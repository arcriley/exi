// examples/esp32c6/src/main.rs
//
// ESP32-C6 EXI serial demo
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! USB Serial/JTAG service for the XIAO ESP32-C6.

#![no_std]
#![no_main]

use core::fmt::{self, Write};
use esp_hal::Blocking;
use esp_hal::time::Instant;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi_esp32c6::{MAX_DOCUMENT, decode, encode, from_hex, grammar, options};

esp_bootloader_esp_idf::esp_app_desc!();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    // No interrupt handler uses USB and panic never returns to the
    // console.
    let peripheral = unsafe { esp_hal::peripherals::USB_DEVICE::steal() };
    let mut usb = Console(UsbSerialJtag::new(peripheral));
    let _ = writeln!(usb, "PANIC {info}\r");
    loop {
        core::hint::spin_loop();
    }
}

struct Console(UsbSerialJtag<'static, Blocking>);

impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0.write(text.as_bytes()).unwrap();
        Ok(())
    }
}

impl Console {
    fn line(&mut self, out: &mut [u8]) -> Option<usize> {
        let mut len = 0;
        let mut overflow = false;
        loop {
            if let Ok(byte) = self.0.read_byte() {
                match byte {
                    b'\n' => return (!overflow).then_some(len),
                    b'\r' => {}
                    _ if len < out.len() => {
                        out[len] = byte;
                        len += 1;
                    }
                    _ => overflow = true,
                }
            }
        }
    }
}

#[esp_hal::main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 32 * 1024);
    let mut usb = Console(UsbSerialJtag::new(p.USB_DEVICE));
    let strings = GlobalStrings::new(GlobalConfig::device()).unwrap();
    let grammar = grammar().unwrap();
    let attached = grammar.attach(&strings).unwrap();
    let opts = options().unwrap();
    let mut line = [0; 2 * MAX_DOCUMENT];
    let mut input = [0; MAX_DOCUMENT];
    let mut output = [0; MAX_DOCUMENT];
    writeln!(usb, "EXI READY\r").unwrap();

    loop {
        let Some(len) = usb.line(&mut line) else {
            writeln!(usb, "ERR line too long\r").unwrap();
            continue;
        };
        let line = &line[..len];
        if line == b"BENCH" {
            const RUNS: i64 = 1_000;
            let start = Instant::now();
            let mut bytes = 0;
            for value in 0..RUNS {
                let len =
                    encode(&attached, &opts, value, &mut output).unwrap();
                let decoded =
                    decode(&attached, &opts, &output[..len]).unwrap();
                assert_eq!(decoded, value);
                bytes += len;
            }
            let micros = start.elapsed().as_micros();
            let heap = esp_alloc::HEAP.stats();
            writeln!(
                usb,
                "BENCH docs={RUNS} bytes={bytes} us={micros} heap_live={} heap_peak={}\r",
                heap.current_usage, heap.max_usage,
            )
            .unwrap();
            continue;
        }
        let Some(len) = from_hex(line, &mut input) else {
            writeln!(usb, "ERR expected EXI hex or BENCH\r").unwrap();
            continue;
        };
        let result =
            decode(&attached, &opts, &input[..len]).and_then(|value| {
                let next = value.checked_add(1).ok_or_else(|| {
                    exi::Error::new(
                        exi::ErrorKind::Value,
                        "reading cannot be incremented",
                    )
                })?;
                encode(&attached, &opts, next, &mut output)
            });
        match result {
            Ok(len) => {
                write!(usb, "EXI ").unwrap();
                for byte in &output[..len] {
                    write!(usb, "{byte:02x}").unwrap();
                }
                writeln!(usb, "\r").unwrap();
            }
            Err(error) => writeln!(usb, "ERR {error}\r").unwrap(),
        }
    }
}
