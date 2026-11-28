// benches/esp32c6/src/main.rs
//
// ESP32-C6 EXI on-device benchmark firmware
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! USB Serial/JTAG benchmark harness for the XIAO ESP32-C6.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::hint::black_box;

use esp_hal::Blocking;
use esp_hal::clock::{Clocks, CpuClock};
use esp_hal::time::Instant;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use exi::grammar::Grammars;
use exi::strings::{GlobalConfig, GlobalStrings};
use exi::{
    Coding, Header, Limits, Options, Reader, ReaderConfig, Writer,
    WriterConfig,
};
#[cfg(feature = "extended")]
use exi::{BinaryKind, Value, grammar::GrammarSet};
use exi_esp32c6::{
    MAX_DOCUMENT, SENSOR_TEMPERATURE, decode, decode_reading_with,
    decode_sensor_with, encode, encode_sensor, fill_samples, from_hex,
    grammar, options, sensor_grammar, sensor_options, write_reading,
    write_sensor,
};
#[cfg(feature = "extended")]
use exi_esp32c6::{
    decode_reading_xml, decode_sensor_xml, encode_reading_xml,
    encode_sensor_xml, sensor_exig_bytes,
};

esp_bootloader_esp_idf::esp_app_desc!();

#[cfg(feature = "extended")]
const PROFILE: &str = "extended";
#[cfg(not(feature = "extended"))]
const PROFILE: &str = "minimal";

#[cfg(feature = "extended")]
const HEAP_SIZE: usize = 128 * 1024;
#[cfg(not(feature = "extended"))]
const HEAP_SIZE: usize = 32 * 1024;

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

/// Runs `work` for a brief warmup, then collects 5 samples of `runs`
/// iterations each and emits one structured `CASE` line.
fn bench_case(
    usb: &mut Console,
    name: &str,
    wire_bytes: usize,
    payload_bytes: usize,
    mut work: impl FnMut(),
) {
    // Warm up and choose iteration count targeting ~25 ms per sample.
    let warm = Instant::now();
    let mut warm_runs = 0u32;
    while warm.elapsed().as_micros() < 15_000 {
        work();
        warm_runs += 1;
    }
    let warm_us = warm.elapsed().as_micros().max(1);
    let runs = ((25_000u64 * warm_runs as u64) / warm_us).clamp(1, 2000) as u32;

    let mut samples_ns = [0u64; 5];
    for slot in &mut samples_ns {
        let start = Instant::now();
        for _ in 0..runs {
            work();
        }
        let elapsed_us = start.elapsed().as_micros();
        *slot = (elapsed_us * 1_000) / runs as u64;
    }
    samples_ns.sort_unstable();

    // Measure single-iteration heap allocation delta separately from timing.
    let before = esp_alloc::HEAP.stats();
    work();
    let after = esp_alloc::HEAP.stats();
    let alloc_bytes =
        after.total_allocated.saturating_sub(before.total_allocated);

    writeln!(
        usb,
        "CASE name={name} wire_bytes={wire_bytes} payload_bytes={payload_bytes} runs={runs} min_ns={} median_ns={} max_ns={} alloc_bytes={alloc_bytes} heap_live={} heap_peak={}\r",
        samples_ns[0],
        samples_ns[2],
        samples_ns[4],
        after.current_usage,
        after.max_usage,
    )
    .unwrap();
}

fn run_suite(
    usb: &mut Console,
    cpu_mhz: u32,
    pool: &GlobalStrings,
    reading_g: &Grammars<'_>,
    reading_opts: &Options,
) {
    let heap0 = esp_alloc::HEAP.stats();
    writeln!(
        usb,
        "SUITE START profile={PROFILE} cpu_mhz={cpu_mhz} heap_size={} reader64_bytes={} reader256_bytes={}\r",
        heap0.size,
        core::mem::size_of::<Reader<'_, 64>>(),
        core::mem::size_of::<Reader<'_, 256>>(),
    )
    .unwrap();

    // 1. Setup: build and attach <reading> grammar to a fresh device pool.
    bench_case(usb, "reading_setup", 0, 0, || {
        let set = grammar().unwrap();
        let pool = GlobalStrings::new(GlobalConfig::device()).unwrap();
        let attached = set.attach(&pool).unwrap();
        black_box(&attached);
    });

    let mut rbuf = [0u8; MAX_DOCUMENT];
    let rlen = encode(reading_g, reading_opts, 215, &mut rbuf).unwrap();

    // 2. <reading> fresh codecs (comparable to original esp32c6 demo).
    bench_case(usb, "reading_encode_fresh", rlen, 8, || {
        let mut out = [0u8; MAX_DOCUMENT];
        let n = encode(reading_g, reading_opts, black_box(215), &mut out)
            .unwrap();
        black_box(n);
    });
    bench_case(usb, "reading_decode_fresh", rlen, 8, || {
        let v = decode(reading_g, reading_opts, black_box(&rbuf[..rlen]))
            .unwrap();
        assert_eq!(v, 215);
    });
    bench_case(usb, "reading_roundtrip_fresh", rlen, 8, || {
        let mut out = [0u8; MAX_DOCUMENT];
        let n = encode(reading_g, reading_opts, black_box(215), &mut out)
            .unwrap();
        let v = decode(reading_g, reading_opts, &out[..n]).unwrap();
        assert_eq!(v, 215);
    });

    // 3. <reading> reused codecs via Writer::reset / Reader::reset.
    {
        let wcfg = WriterConfig::new(reading_opts.clone(), reading_g)
            .header(Header::Bare { cookie: false });
        let mut spare = Vec::with_capacity(MAX_DOCUMENT);
        let mut writer =
            Writer::new(Vec::with_capacity(MAX_DOCUMENT), wcfg).unwrap();
        bench_case(usb, "reading_encode_reused", rlen, 8, || {
            write_reading(&mut writer, black_box(215)).unwrap();
            spare.clear();
            spare = writer.reset(core::mem::take(&mut spare));
            black_box(spare.len());
        });

        let mut reader = Reader::<64>::new(
            ReaderConfig::new(reading_g).options(reading_opts.clone()),
        )
        .unwrap();
        bench_case(usb, "reading_decode_reused", rlen, 8, || {
            let v = decode_reading_with(&mut reader, black_box(&rbuf[..rlen]))
                .unwrap();
            assert_eq!(v, 215);
            reader.reset();
        });

        bench_case(usb, "reading_roundtrip_reused", rlen, 8, || {
            write_reading(&mut writer, black_box(215)).unwrap();
            spare.clear();
            spare = writer.reset(core::mem::take(&mut spare));
            let v = decode_reading_with(&mut reader, &spare).unwrap();
            assert_eq!(v, 215);
            reader.reset();
        });
    }

    // 4. Sensor schema: byte-aligned, 64 B and 2048 B binary payloads.
    let sensor_set = sensor_grammar().unwrap();
    let sensor_g = sensor_set.attach(pool).unwrap();
    let mut samples64 = [0u8; 64];
    fill_samples(&mut samples64);
    let mut samples2k = alloc::vec![0u8; 2048];
    fill_samples(&mut samples2k);

    {
        let sopts = sensor_options(Coding::ByteAligned).unwrap();
        let mut wire64 = [0u8; 128];
        let wlen64 = encode_sensor(
            &sensor_g,
            &sopts,
            SENSOR_TEMPERATURE,
            &samples64,
            &mut wire64,
        )
        .unwrap();

        let wcfg = WriterConfig::new(sopts.clone(), &sensor_g)
            .header(Header::Bare { cookie: false })
            .limits(Limits::device());
        let mut spare = Vec::with_capacity(2112);
        let mut writer = Writer::new(Vec::with_capacity(2112), wcfg).unwrap();
        bench_case(usb, "sensor_byte_64_encode", wlen64, 64, || {
            write_sensor(&mut writer, SENSOR_TEMPERATURE, &samples64).unwrap();
            spare.clear();
            spare = writer.reset(core::mem::take(&mut spare));
            black_box(spare.len());
        });

        let mut reader = Reader::<256>::new(
            ReaderConfig::new(&sensor_g)
                .options(sopts.clone())
                .limits(Limits::device()),
        )
        .unwrap();
        bench_case(usb, "sensor_byte_64_decode_whole", wlen64, 64, || {
            decode_sensor_with(
                &mut reader,
                black_box(&wire64[..wlen64]),
                wlen64,
                SENSOR_TEMPERATURE,
                &samples64,
            )
            .unwrap();
            reader.reset();
        });
        bench_case(usb, "sensor_byte_64_decode_chunk32", wlen64, 64, || {
            decode_sensor_with(
                &mut reader,
                black_box(&wire64[..wlen64]),
                32,
                SENSOR_TEMPERATURE,
                &samples64,
            )
            .unwrap();
            reader.reset();
        });

        let mut wire2k = alloc::vec![0u8; 2112];
        let wlen2k = encode_sensor(
            &sensor_g,
            &sopts,
            SENSOR_TEMPERATURE,
            &samples2k,
            &mut wire2k,
        )
        .unwrap();
        bench_case(usb, "sensor_byte_2k_encode", wlen2k, 2048, || {
            write_sensor(&mut writer, SENSOR_TEMPERATURE, &samples2k).unwrap();
            spare.clear();
            spare = writer.reset(core::mem::take(&mut spare));
            black_box(spare.len());
        });
        bench_case(usb, "sensor_byte_2k_decode_chunk32", wlen2k, 2048, || {
            decode_sensor_with(
                &mut reader,
                black_box(&wire2k[..wlen2k]),
                32,
                SENSOR_TEMPERATURE,
                &samples2k,
            )
            .unwrap();
            reader.reset();
        });
        bench_case(usb, "sensor_byte_2k_decode_chunk1", wlen2k, 2048, || {
            decode_sensor_with(
                &mut reader,
                black_box(&wire2k[..wlen2k]),
                1,
                SENSOR_TEMPERATURE,
                &samples2k,
            )
            .unwrap();
            reader.reset();
        });
    }

    // 5. Extended profile: .exig load/attach, bit-packed coding, and XML text.
    #[cfg(feature = "extended")]
    {
        let exig = sensor_exig_bytes().unwrap();
        bench_case(usb, "sensor_exig_load", exig.len(), 0, || {
            let set = GrammarSet::from_exig_with(
                black_box(&exig),
                Limits::device(),
            )
            .unwrap();
            black_box(&set);
        });
        let loaded =
            GrammarSet::from_exig_with(&exig, Limits::device()).unwrap();
        bench_case(usb, "sensor_exig_attach", exig.len(), 0, || {
            let fresh = GlobalStrings::new(GlobalConfig::device()).unwrap();
            let attached = loaded.attach(&fresh).unwrap();
            black_box(&attached);
        });

        let bopts = sensor_options(Coding::BitPacked).unwrap();
        let mut wire64 = [0u8; 128];
        let wlen64 = encode_sensor(
            &sensor_g,
            &bopts,
            SENSOR_TEMPERATURE,
            &samples64,
            &mut wire64,
        )
        .unwrap();
        let wcfg = WriterConfig::new(bopts.clone(), &sensor_g)
            .header(Header::Bare { cookie: false })
            .limits(Limits::device());
        let mut spare = Vec::with_capacity(2112);
        let mut writer = Writer::new(Vec::with_capacity(2112), wcfg).unwrap();
        bench_case(usb, "sensor_bit_64_encode", wlen64, 64, || {
            write_sensor(&mut writer, SENSOR_TEMPERATURE, &samples64).unwrap();
            spare.clear();
            spare = writer.reset(core::mem::take(&mut spare));
            black_box(spare.len());
        });
        let mut reader = Reader::<256>::new(
            ReaderConfig::new(&sensor_g)
                .options(bopts.clone())
                .limits(Limits::device()),
        )
        .unwrap();
        bench_case(usb, "sensor_bit_64_decode_whole", wlen64, 64, || {
            decode_sensor_with(
                &mut reader,
                black_box(&wire64[..wlen64]),
                wlen64,
                SENSOR_TEMPERATURE,
                &samples64,
            )
            .unwrap();
            reader.reset();
        });
        bench_case(usb, "sensor_bit_64_decode_chunk32", wlen64, 64, || {
            decode_sensor_with(
                &mut reader,
                black_box(&wire64[..wlen64]),
                32,
                SENSOR_TEMPERATURE,
                &samples64,
            )
            .unwrap();
            reader.reset();
        });

        let mut wire2k = alloc::vec![0u8; 2112];
        let wlen2k = encode_sensor(
            &sensor_g,
            &bopts,
            SENSOR_TEMPERATURE,
            &samples2k,
            &mut wire2k,
        )
        .unwrap();
        bench_case(usb, "sensor_bit_2k_encode", wlen2k, 2048, || {
            write_sensor(&mut writer, SENSOR_TEMPERATURE, &samples2k).unwrap();
            spare.clear();
            spare = writer.reset(core::mem::take(&mut spare));
            black_box(spare.len());
        });
        bench_case(usb, "sensor_bit_2k_decode_chunk32", wlen2k, 2048, || {
            decode_sensor_with(
                &mut reader,
                black_box(&wire2k[..wlen2k]),
                32,
                SENSOR_TEMPERATURE,
                &samples2k,
            )
            .unwrap();
            reader.reset();
        });

        // XML text comparison on the same ESP32-C6 hardware.
        let mut xrbuf = [0u8; 64];
        let xrlen = encode_reading_xml(215, &mut xrbuf).unwrap();
        bench_case(usb, "reading_xml_encode", xrlen, 8, || {
            let mut out = [0u8; 64];
            let n = encode_reading_xml(black_box(215), &mut out).unwrap();
            black_box(n);
        });
        bench_case(usb, "reading_xml_decode", xrlen, 8, || {
            let v =
                decode_reading_xml(&reading_g, black_box(&xrbuf[..xrlen]))
                    .unwrap();
            assert_eq!(v, 215);
        });

        let mut b64_buf = [0u8; 128];
        let expected_b64 = Value::Binary(&samples64, BinaryKind::Base64)
            .write_lexical(&mut b64_buf)
            .unwrap();
        let mut xsbuf = [0u8; 256];
        let xslen = encode_sensor_xml(
            SENSOR_TEMPERATURE,
            &samples64,
            &mut xsbuf,
        )
        .unwrap();
        bench_case(usb, "sensor_xml_64_encode", xslen, 64, || {
            let mut out = [0u8; 256];
            let n = encode_sensor_xml(
                SENSOR_TEMPERATURE,
                &samples64,
                &mut out,
            )
            .unwrap();
            black_box(n);
        });
        bench_case(usb, "sensor_xml_64_decode", xslen, 64, || {
            decode_sensor_xml(
                &sensor_g,
                black_box(&xsbuf[..xslen]),
                SENSOR_TEMPERATURE,
                expected_b64,
            )
            .unwrap();
        });
    }

    let heap_end = esp_alloc::HEAP.stats();
    writeln!(
        usb,
        "SUITE END profile={PROFILE} heap_live={} heap_peak={}\r",
        heap_end.current_usage, heap_end.max_usage,
    )
    .unwrap();
}

#[esp_hal::main]
fn main() -> ! {
    let cfg = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(cfg);
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    let cpu_mhz = Clocks::get().cpu_clock.as_mhz();
    let mut usb = Console(UsbSerialJtag::new(p.USB_DEVICE));
    let strings = GlobalStrings::new(GlobalConfig::device()).unwrap();
    let grammar = grammar().unwrap();
    let attached = grammar.attach(&strings).unwrap();
    let opts = options().unwrap();
    let mut line = [0; 2 * MAX_DOCUMENT];
    let mut input = [0; MAX_DOCUMENT];
    let mut output = [0; MAX_DOCUMENT];
    let heap = esp_alloc::HEAP.stats();
    writeln!(
        usb,
        "EXI READY profile={PROFILE} cpu_mhz={cpu_mhz} heap_size={}\r",
        heap.size,
    )
    .unwrap();

    loop {
        let Some(len) = usb.line(&mut line) else {
            writeln!(usb, "ERR line too long\r").unwrap();
            continue;
        };
        let line = &line[..len];
        if line.is_empty() {
            continue;
        }
        if line == b"INFO" {
            let h = esp_alloc::HEAP.stats();
            writeln!(
                usb,
                "INFO profile={PROFILE} cpu_mhz={cpu_mhz} heap_size={} heap_live={} heap_peak={} reader64_bytes={} reader256_bytes={}\r",
                h.size,
                h.current_usage,
                h.max_usage,
                core::mem::size_of::<Reader<'_, 64>>(),
                core::mem::size_of::<Reader<'_, 256>>(),
            )
            .unwrap();
            continue;
        }
        if line == b"BENCH SUITE" {
            run_suite(&mut usb, cpu_mhz, &strings, &attached, &opts);
            continue;
        }
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
            writeln!(
                usb,
                "ERR expected INFO, BENCH, BENCH SUITE, or EXI hex\r"
            )
            .unwrap();
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
