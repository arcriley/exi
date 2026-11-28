// benches/esp32c6.rs
//
// Host runner for the XIAO ESP32-C6 on-device benchmark
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Builds, flashes, and runs the XIAO ESP32-C6 benchmark firmware in
//! `benches/esp32c6/`, writing `target/bench/esp32c6.csv` and `.meta`.
//!
//!     cargo bench --bench esp32c6
//!     cargo bench --bench esp32c6 -- --port /dev/ttyACM0 --profile minimal
//!     cargo bench --bench esp32c6 -- --build-only
//!     cargo bench --bench esp32c6 -- --no-flash

mod common;

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Profile {
    Minimal,
    Extended,
}

impl Profile {
    fn name(self) -> &'static str {
        match self {
            Profile::Minimal => "minimal",
            Profile::Extended => "extended",
        }
    }
    fn features(self) -> &'static [&'static str] {
        match self {
            Profile::Minimal => &[],
            Profile::Extended => &["--features", "extended"],
        }
    }
    fn label(self) -> &'static str {
        match self {
            Profile::Minimal => "bare-header-only",
            Profile::Extended => "bare-header-only,bit-packed,exig,xml",
        }
    }
}

struct Cli {
    port: String,
    profiles: Vec<Profile>,
    build_only: bool,
    no_flash: bool,
}

fn parse_args() -> Cli {
    let mut port = std::env::var("EXI_ESP32C6_PORT")
        .unwrap_or_else(|_| "/dev/ttyACM0".to_string());
    let mut profiles = vec![Profile::Minimal, Profile::Extended];
    let mut build_only = false;
    let mut no_flash = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bench" => {}
            "--build-only" => build_only = true,
            "--no-flash" => no_flash = true,
            "--port" => {
                port = args.next().expect("--port needs a device path");
            }
            "--profile" => {
                let v = args
                    .next()
                    .expect("--profile needs minimal, extended, or all");
                profiles = match v.as_str() {
                    "minimal" => vec![Profile::Minimal],
                    "extended" => vec![Profile::Extended],
                    "all" => vec![Profile::Minimal, Profile::Extended],
                    other => panic!("unknown profile: {other}"),
                };
            }
            "--help" | "-h" => {
                println!(
                    "Usage: cargo bench --bench esp32c6 -- [OPTIONS]\n\
                     \n\
                     Options:\n\
                       --port <PATH>          Serial device (default: $EXI_ESP32C6_PORT or /dev/ttyACM0)\n\
                       --profile <NAME>       minimal, extended, or all (default: all)\n\
                       --build-only           Build firmware profiles and report ELF/image sizes only\n\
                       --no-flash             Skip espflash and run suite on already-flashed board"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("ignoring unknown argument: {other}");
            }
        }
    }
    if no_flash && profiles.len() > 1 {
        // Without flashing, only one firmware image is on the board.
        profiles.truncate(1);
    }
    Cli { port, profiles, build_only, no_flash }
}

/// Parses an ELF32 little-endian firmware binary and returns
/// `(flash_bytes, static_ram_bytes)`:
/// - `flash_bytes`: total `PT_LOAD` `p_filesz` (code, rodata, initialized data).
/// - `static_ram_bytes`: `PT_LOAD` `p_memsz` in SRAM (`p_vaddr < 0x4200_0000`)
///   excluding the pure-NOBITS `.stack` segment (`p_filesz == 0`).
fn elf32_load_sizes(path: &Path) -> Option<(u64, u64)> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 52 || &bytes[0..4] != b"\x7fELF" || bytes[4] != 1 {
        return None;
    }
    let u16le = |off: usize| -> u16 {
        u16::from_le_bytes(bytes[off..off + 2].try_into().unwrap())
    };
    let u32le = |off: usize| -> u32 {
        u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
    };
    let phoff = u32le(28) as usize;
    let phentsize = u16le(42) as usize;
    let phnum = u16le(44) as usize;
    if phentsize < 32 || phoff + phnum * phentsize > bytes.len() {
        return None;
    }
    let mut flash_bytes = 0u64;
    let mut static_ram_bytes = 0u64;
    for i in 0..phnum {
        let off = phoff + i * phentsize;
        let p_type = u32le(off);
        if p_type == 1 {
            // PT_LOAD
            let p_vaddr = u32le(off + 8);
            let p_filesz = u32le(off + 16) as u64;
            let p_memsz = u32le(off + 20) as u64;
            flash_bytes += p_filesz;
            if p_vaddr < 0x4200_0000 && p_filesz > 0 {
                static_ram_bytes += p_memsz;
            }
        }
    }
    Some((flash_bytes, static_ram_bytes))
}

struct BuildInfo {
    elf_path: PathBuf,
    flash_bytes: u64,
    static_ram_bytes: u64,
    app_image_bytes: Option<u64>,
}

fn has_espflash() -> bool {
    Command::new("espflash")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn build_profile(crate_dir: &Path, profile: Profile) -> BuildInfo {
    let mut cmd = Command::new("cargo");
    cmd.arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--offline")
        .args(profile.features())
        .current_dir(crate_dir);
    eprintln!(
        "Building esp32c6 profile={} ({})...",
        profile.name(),
        profile.label()
    );
    let status = cmd.status().expect("failed to invoke cargo +nightly");
    assert!(
        status.success(),
        "firmware build failed for profile {}",
        profile.name()
    );

    let elf_path = crate_dir
        .join("target/riscv32imac-unknown-none-elf/release/exi-esp32c6");
    let (flash_bytes, static_ram_bytes) =
        elf32_load_sizes(&elf_path).unwrap_or((0, 0));

    let app_image_bytes = if has_espflash() {
        let bin = common::out_dir()
            .join(format!("esp32c6-{}.bin", profile.name()));
        let ok = Command::new("espflash")
            .args(["save-image", "--chip", "esp32c6"])
            .arg(&elf_path)
            .arg(&bin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok { fs::metadata(&bin).ok().map(|m| m.len()) } else { None }
    } else {
        None
    };

    BuildInfo { elf_path, flash_bytes, static_ram_bytes, app_image_bytes }
}

fn flash_profile(port: &str, elf_path: &Path) {
    if !has_espflash() {
        eprintln!(
            "error: `espflash` was not found in PATH.\n\
             Install it with `cargo install espflash --locked` (or run with `--build-only` / `--no-flash`)."
        );
        std::process::exit(1);
    }
    eprintln!("Flashing {} to {port}...", elf_path.display());
    let status = Command::new("espflash")
        .args(["flash", "--chip", "esp32c6", "--port", port, "--non-interactive"])
        .arg(elf_path)
        .status()
        .expect("failed to run espflash");
    if !status.success() {
        eprintln!(
            "error: `espflash flash` failed on {port}. Check that the XIAO ESP32-C6 is connected \
             and that your user has read/write permission on {port} (dialout group or udev rule)."
        );
        std::process::exit(1);
    }
    // Wait briefly for the USB Serial/JTAG peripheral to re-enumerate after reset.
    thread::sleep(Duration::from_millis(800));
}

fn configure_serial(port: &str) {
    let status = Command::new("stty")
        .args([
            "-F",
            port,
            "115200",
            "raw",
            "-echo",
            "-echoe",
            "-echok",
            "-onlcr",
            "-icrnl",
            "-ixon",
            "-ixoff",
            "-crtscts",
            "-hupcl",
        ])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("stty failed on {port} with status {s}"),
        Err(e) => panic!("failed to run stty on {port}: {e}"),
    }
}

fn parse_kv(line: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for tok in line.split_whitespace() {
        if let Some((k, v)) = tok.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

#[derive(Debug)]
struct CaseRow {
    profile: String,
    name: String,
    wire_bytes: usize,
    payload_bytes: usize,
    runs: u32,
    min_ns: u64,
    median_ns: u64,
    max_ns: u64,
    alloc_bytes: usize,
    heap_live: usize,
    heap_peak: usize,
}

struct SuiteResult {
    profile: String,
    cpu_mhz: u32,
    heap_size: usize,
    reader64_bytes: usize,
    reader256_bytes: usize,
    heap_live_end: usize,
    heap_peak_end: usize,
    cases: Vec<CaseRow>,
}

fn wait_for_port(port: &str, timeout: Duration) -> std::fs::File {
    let deadline = Instant::now() + timeout;
    loop {
        if Path::new(port).exists() {
            match OpenOptions::new().read(true).write(true).open(port) {
                Ok(f) => return f,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!(
                        "error: permission denied opening {port}.\n\
                         Add your user to the `dialout` group or install a udev rule for Espressif USB Serial/JTAG (303a:1001)."
                    );
                    std::process::exit(1);
                }
                Err(_) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(150));
                }
                Err(e) => panic!("failed to open {port}: {e}"),
            }
        } else if Instant::now() < deadline {
            thread::sleep(Duration::from_millis(150));
        } else {
            panic!("serial device {port} did not appear within {timeout:?}");
        }
    }
}

fn run_on_device(port: &str) -> SuiteResult {
    let file = wait_for_port(port, Duration::from_secs(5));
    configure_serial(port);
    let mut writer = file.try_clone().expect("clone serial fd");
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if !trimmed.is_empty() && tx.send(trimmed).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Probe the firmware with `INFO` until it responds.
    let probe_deadline = Instant::now() + Duration::from_secs(8);
    let mut info_kv = None;
    while Instant::now() < probe_deadline {
        let _ = writer.write_all(b"\nINFO\n");
        let _ = writer.flush();
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(line) if line.starts_with("INFO ") => {
                info_kv = Some(parse_kv(&line));
                break;
            }
            Ok(line) if line.starts_with("PANIC ") => {
                panic!("device panicked: {line}");
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("serial reader disconnected while probing {port}");
            }
        }
    }
    let info = info_kv.expect("ESP32-C6 did not answer INFO on serial port");
    let profile = info.get("profile").cloned().unwrap_or_default();
    let cpu_mhz = info
        .get("cpu_mhz")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let heap_size = info
        .get("heap_size")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let reader64_bytes = info
        .get("reader64_bytes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let reader256_bytes = info
        .get("reader256_bytes")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Drain any stray queued lines before starting the suite.
    while rx.try_recv().is_ok() {}

    writer
        .write_all(b"BENCH SUITE\n")
        .expect("failed to send BENCH SUITE");
    writer.flush().expect("failed to flush BENCH SUITE");

    let mut cases: Vec<CaseRow> = Vec::new();
    let heap_live_end;
    let heap_peak_end;
    let suite_deadline = Instant::now() + Duration::from_secs(90);

    loop {
        let remain = suite_deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            panic!("timed out waiting for SUITE END from {port}");
        }
        let line = rx
            .recv_timeout(remain.min(Duration::from_secs(15)))
            .expect("timed out reading benchmark output from ESP32-C6");
        if line.starts_with("PANIC ") {
            for c in &cases {
                eprintln!(
                    "  ok: {} (alloc={} live={} peak={})",
                    c.name, c.alloc_bytes, c.heap_live, c.heap_peak
                );
            }
            panic!("device panicked during benchmark: {line}");
        }
        if line.starts_with("CASE ") {
            let kv = parse_kv(&line);
            let get_u64 = |k: &str| -> u64 {
                kv.get(k).and_then(|v| v.parse().ok()).unwrap_or(0)
            };
            let row = CaseRow {
                profile: profile.clone(),
                name: kv.get("name").cloned().unwrap_or_default(),
                wire_bytes: get_u64("wire_bytes") as usize,
                payload_bytes: get_u64("payload_bytes") as usize,
                runs: get_u64("runs") as u32,
                min_ns: get_u64("min_ns"),
                median_ns: get_u64("median_ns"),
                max_ns: get_u64("max_ns"),
                alloc_bytes: get_u64("alloc_bytes") as usize,
                heap_live: get_u64("heap_live") as usize,
                heap_peak: get_u64("heap_peak") as usize,
            };
            cases.push(row);
        } else if line.starts_with("SUITE END ") {
            let kv = parse_kv(&line);
            heap_live_end = kv
                .get("heap_live")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            heap_peak_end = kv
                .get("heap_peak")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            break;
        }
    }

    SuiteResult {
        profile,
        cpu_mhz,
        heap_size,
        reader64_bytes,
        reader256_bytes,
        heap_live_end,
        heap_peak_end,
        cases,
    }
}

fn print_suite(suite: &SuiteResult, build: Option<&BuildInfo>) {
    println!(
        "\n== XIAO ESP32-C6 ({}, {} MHz, heap {} KiB, Reader<64> {} B, Reader<256> {} B) ==",
        suite.profile,
        suite.cpu_mhz,
        suite.heap_size / 1024,
        suite.reader64_bytes,
        suite.reader256_bytes,
    );
    if let Some(b) = build {
        match b.app_image_bytes {
            Some(img) => println!(
                "Image: {img} B app binary (ELF flash: {} B, static RAM incl. heap: {} B)",
                b.flash_bytes, b.static_ram_bytes
            ),
            None => println!(
                "Image: ELF flash {} B, static RAM incl. heap {} B (install espflash for app .bin size)",
                b.flash_bytes, b.static_ram_bytes
            ),
        }
    }
    println!(
        "{:<32} {:>6} {:>10} {:>7} {:>10} {:>8} {:>9} {:>9}",
        "Case",
        "Wire B",
        "Median",
        "Spread",
        "Throughput",
        "Alloc/op",
        "Heap live",
        "Heap peak"
    );
    for c in &suite.cases {
        let med_us = c.median_ns as f64 / 1_000.0;
        let spread = if c.min_ns > 0 {
            (c.max_ns as f64 / c.min_ns as f64 - 1.0) * 100.0
        } else {
            0.0
        };
        let speed = if c.wire_bytes > 0 && c.median_ns > 0 {
            let kb_s = (c.wire_bytes as f64 / (c.median_ns as f64 * 1e-9))
                / 1024.0;
            if kb_s >= 1024.0 {
                format!("{:>6.2} MB/s", kb_s / 1024.0)
            } else {
                format!("{:>6.1} KB/s", kb_s)
            }
        } else {
            format!("{:>10}", "-")
        };
        println!(
            "{:<32} {:>6} {:>7.1} µs {:>6.1}% {:>10} {:>6} B {:>7} B {:>7} B",
            c.name,
            c.wire_bytes,
            med_us,
            spread,
            speed,
            c.alloc_bytes,
            c.heap_live,
            c.heap_peak,
        );
    }
    println!(
        "End heap: live {} B, peak {} B",
        suite.heap_live_end, suite.heap_peak_end
    );
}

fn main() {
    let cli = parse_args();
    let crate_dir = common::package("benches/esp32c6");
    let mut csv = String::from(
        "profile,case,wire_bytes,payload_bytes,runs,min_ns,median_ns,max_ns,alloc_bytes,heap_live,heap_peak,flash_bytes,static_ram_bytes,app_image_bytes,cpu_mhz\n",
    );
    let mut meta_extra = String::new();

    for &profile in &cli.profiles {
        let build = if cli.no_flash {
            None
        } else {
            let info = build_profile(&crate_dir, profile);
            meta_extra += &format!(
                "esp32c6_{}_flash_bytes: {}\nesp32c6_{}_static_ram_bytes: {}\n",
                profile.name(),
                info.flash_bytes,
                profile.name(),
                info.static_ram_bytes,
            );
            if let Some(img) = info.app_image_bytes {
                meta_extra += &format!(
                    "esp32c6_{}_app_image_bytes: {img}\n",
                    profile.name()
                );
            }
            if cli.build_only {
                println!(
                    "Built profile={:<8} ({:<36}) flash: {:>7} B, static RAM: {:>6} B{}",
                    profile.name(),
                    profile.label(),
                    info.flash_bytes,
                    info.static_ram_bytes,
                    info.app_image_bytes
                        .map(|n| format!(", app image: {n} B"))
                        .unwrap_or_default()
                );
                continue;
            }
            flash_profile(&cli.port, &info.elf_path);
            Some(info)
        };

        let suite = run_on_device(&cli.port);
        print_suite(&suite, build.as_ref());
        meta_extra += &format!(
            "esp32c6_{}_cpu_mhz: {}\nesp32c6_{}_heap_size: {}\nesp32c6_{}_heap_peak: {}\n",
            suite.profile,
            suite.cpu_mhz,
            suite.profile,
            suite.heap_size,
            suite.profile,
            suite.heap_peak_end,
        );
        let (ef, em, img) = build
            .as_ref()
            .map(|b| {
                (b.flash_bytes, b.static_ram_bytes, b.app_image_bytes.unwrap_or(0))
            })
            .unwrap_or((0, 0, 0));
        for c in &suite.cases {
            csv += &format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                c.profile,
                c.name,
                c.wire_bytes,
                c.payload_bytes,
                c.runs,
                c.min_ns,
                c.median_ns,
                c.max_ns,
                c.alloc_bytes,
                c.heap_live,
                c.heap_peak,
                ef,
                em,
                img,
                suite.cpu_mhz,
            );
        }
    }

    if !cli.build_only {
        let csv_path = common::out_dir().join("esp32c6.csv");
        fs::write(&csv_path, &csv).unwrap();
        common::write_meta("esp32c6");
        let meta_path = common::out_dir().join("esp32c6.meta");
        let mut meta = fs::read_to_string(&meta_path).unwrap_or_default();
        meta.push_str(&meta_extra);
        fs::write(&meta_path, meta).unwrap();
        println!("\nwrote {}", csv_path.display());
        println!("wrote {}", meta_path.display());
    }
}
