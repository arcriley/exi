# XIAO ESP32-C6 EXI Benchmark

On-device firmware benchmark for the Seeed Studio XIAO ESP32-C6 (RISC-V
`riscv32imac-unknown-none-elf`, 160 MHz), paired with the host runner
[`benches/esp32c6.rs`](../esp32c6.rs).

## Profiles

Two firmware profiles measure both minimum footprint and full device features:

| Profile | Cargo features | Allocator heap | What it measures |
|---|---|---:|---|
| `minimal` | default (`bare-header-only`) | 32 KiB | Smallest flash/RAM build; `<reading>` fresh and reused codecs; byte-aligned sensor (64 B & 2 KiB binary, whole / 32 B / 1 B streaming feeds) |
| `extended` | `--features extended` (`bit-packed`, `exig`, `xml`) | 128 KiB | All `minimal` cases plus `.exig` load/attach, bit-packed sensor coding, and UTF-8 XML text (`XmlWriter` / `XmlReader`) on the same MCU |

## Running from the repository root

With the XIAO ESP32-C6 connected (default `/dev/ttyACM0`), `espflash` in
`PATH`, and `riscv32imac-unknown-none-elf` installed on nightly:

```sh
cargo bench --bench esp32c6                           # build, flash, and run both profiles
cargo bench --bench esp32c6 -- --port /dev/ttyACM0    # explicit serial port
cargo bench --bench esp32c6 -- --profile minimal      # one profile only
cargo bench --bench esp32c6 -- --build-only           # build both profiles and report ELF/image sizes
cargo bench --bench esp32c6 -- --no-flash             # run suite against already-flashed firmware
```

Results are written to `target/bench/esp32c6.csv` and
`target/bench/esp32c6.meta`.

### Linux serial permissions

Native USB Serial/JTAG (`303a:1001`) enumerates as `/dev/ttyACM0` owned by
`root:dialout`. Either add your user to `dialout` (`sudo usermod -aG dialout
$USER` and re-login) or install a udev rule granting `plugdev` access:

```text
SUBSYSTEM=="tty", ATTRS{idVendor}=="303a", ATTRS{idProduct}=="1001", GROUP="plugdev", MODE="0660"
```

## Direct firmware build and host tests

From this directory:

```sh
cargo +nightly build --release --locked
cargo +nightly build --release --locked --features extended
cargo +nightly test --locked --lib --target x86_64-unknown-linux-gnu
cargo +nightly test --locked --lib --features extended --target x86_64-unknown-linux-gnu
```

## Serial console protocol

The firmware prints `EXI READY profile=… cpu_mhz=160 heap_size=…` on boot and
accepts LF- or CRLF-terminated commands:

- `INFO` — profile, CPU clock, heap size/usage, and inline `Reader` sizes.
- `BENCH SUITE` — runs all benchmark cases (5 samples each) and prints `SUITE START`, one `CASE …` line per workload, and `SUITE END`.
- `BENCH` — runs 1,000 fresh `<reading>` round trips (compatible with the original serial demo).
- `<hex>` — decodes one bare-header byte-aligned `<reading>` document, increments its integer value, and replies `EXI <hex>`.

For example, send `80000015` (21) and the console replies `EXI 80000016`
(22). The browser suite also tests those readings with bit-packed coding,
where they occupy three bytes each, `800540` and `800580`.
