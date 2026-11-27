# EXI over USB on the XIAO ESP32-C6

This Rust firmware uses EXI with a fixed schema and a USB Serial/JTAG console.
The schema has one element, `<reading>`, with signed integer content. The
device decodes an EXI document and returns a fresh document with the reading
incremented. By default both directions use strict, byte-aligned documents
with bare headers. The schema and options are agreed out of band.

## Build and flash

From this directory, with `espflash` in PATH and the nightly
`riscv32imac-unknown-none-elf` target installed:

```sh
cargo +nightly build --release --locked
cargo +nightly run --release --locked
```

The second command flashes the firmware and opens the serial monitor. Use a
USB data cable and make sure your account can open the board's serial port.
On Linux, native USB serial requires the `cdc_acm` driver.

## Exchange and benchmark

Send one line of ASCII hex, terminated by LF or CRLF. The hex is a transport
wrapper around a complete EXI document. Each request starts a fresh codec
session. The firmware prints `EXI READY` at startup. A reply starts with
`EXI ` or `ERR `.

```text
> 80000015
< EXI 80000016
> BENCH
< BENCH docs=1000 bytes=4872 us=… heap_live=… heap_peak=…
```

The example sends 21 and receives 22. `BENCH` measures 1,000 independent
encode/decode round trips and verifies each result. Timing excludes USB I/O
and grammar construction, and includes per-document codec construction.
Heap figures are the allocator's live and high-water estimates since boot,
including grammar setup. The firmware reserves 32 KiB for the allocator.

On a XIAO ESP32-C6 (chip revision 0.2), three byte-aligned runs took 2.527–2.530 s per
1,000 round trips, with 9,504 bytes live and a 10,236-byte heap high-water
mark. The release application image was 201,488 bytes.

Build with `--features bit-packed` at both ends to pack the event code and
integer sign into bits. The same exchange then sends `800540` and receives
`800580`. The bare header carries no coding options.

To run the shared codec test from this directory on an x86-64 Linux host:

```sh
cargo +nightly test --locked --lib --target x86_64-unknown-linux-gnu
cargo +nightly test --locked --lib --target x86_64-unknown-linux-gnu --features bit-packed
```
