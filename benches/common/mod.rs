// benches/common/mod.rs
//
// Shared parts of the benchmark harnesses
// Copyright © 2026 Arc Riley
//
// This file is free software and available for distribution by
// the terms of the Apache 2.0 or MIT licenses. Code contrubutions
// are welcome only if this notice remains unchanged clearly
// communicating that changes are made under these same conditions.
//
//! Sample statistics, the output directory, and the record of the
//! machine and build each run's figures came from.
//!
//! Cargo makes bench targets only from `benches/*.rs` and
//! `benches/*/main.rs`, so this module is shared with `mod common;` and
//! is not a bench of its own.

#![allow(dead_code)] // each bench uses some of this

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// The median, fastest and slowest of a set of samples.
#[derive(Clone, Copy)]
pub struct Stats {
    pub median: Duration,
    pub min: Duration,
    pub max: Duration,
    pub samples: usize,
}

impl Stats {
    pub fn of(mut v: Vec<Duration>) -> Stats {
        assert!(!v.is_empty(), "no samples");
        v.sort();
        Stats {
            median: v[v.len() / 2],
            min: v[0],
            max: v[v.len() - 1],
            samples: v.len(),
        }
    }

    /// The spread, slowest over fastest less one: 0.05 is 5 %.
    pub fn spread(&self) -> f64 {
        self.max.as_secs_f64() / self.min.as_secs_f64().max(1e-12) - 1.0
    }
}

/// `target/bench` in the package, made if needed.
pub fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/bench");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A path in the package.
pub fn package(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// Writes `target/bench/<bench>.meta`: what a reader needs to reproduce
/// the figures. Lines are `key: value`; anything that cannot be read is
/// `unknown`.
pub fn write_meta(bench: &str) {
    let mut s = String::new();
    let mut put = |k: &str, v: String| {
        let v = v.trim();
        s += &format!("{k}: {}\n", if v.is_empty() { "unknown" } else { v });
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    put("bench", bench.into());
    put("unix_time", secs.to_string());
    put("args", std::env::args().skip(1).collect::<Vec<_>>().join(" "));
    put("git_commit", run("git", &["rev-parse", "HEAD"]));
    // Uncommitted changes mean the commit alone does not describe the
    // code timed.
    let dirty = run("git", &["status", "--porcelain", "--untracked-files=no"]);
    put("git_dirty", (!dirty.trim().is_empty()).to_string());
    // The toolchain on PATH; under rustup it is the one cargo used for
    // this build.
    for line in run("rustc", &["-vV"]).lines() {
        if let Some((k, v)) = line.split_once(": ") {
            put(
                &format!(
                    "rustc_{}",
                    k.replace(['-', ' '], "_").to_lowercase()
                ),
                v.into(),
            );
        }
    }
    put("debug_assertions", cfg!(debug_assertions).to_string());
    put("features", features().join(","));
    // Cargo.lock is not in git, so record the versions that set the
    // numbers.
    put(
        "dependencies",
        locked(&["flate2", "lz4_flex", "zstd", "html5ever", "sha2"]),
    );
    put("cpu", first_match("/proc/cpuinfo", "model name"));
    put(
        "cpus",
        std::thread::available_parallelism()
            .map_or(0, |n| n.get())
            .to_string(),
    );
    put("kernel", read("/proc/sys/kernel/osrelease"));
    put(
        "governor",
        read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
    );
    // 1 when turbo or boost is on: AMD and acpi-cpufreq use `boost`,
    // Intel `no_turbo`.
    let boost = read("/sys/devices/system/cpu/cpufreq/boost");
    let no_turbo = read("/sys/devices/system/cpu/intel_pstate/no_turbo");
    put(
        "boost",
        match (boost.trim(), no_turbo.trim()) {
            (b, _) if !b.is_empty() => b.into(),
            ("", "0") => "1".into(),
            ("", "1") => "0".into(),
            _ => String::new(),
        },
    );
    let path = out_dir().join(format!("{bench}.meta"));
    std::fs::write(&path, s).unwrap();
    println!("wrote {}", path.display());
}

/// The crate features this bench was built with.
fn features() -> Vec<&'static str> {
    let all = [
        ("std", cfg!(feature = "std")),
        ("xml", cfg!(feature = "xml")),
        ("bit-packed", cfg!(feature = "bit-packed")),
        ("compression", cfg!(feature = "compression")),
        ("exig", cfg!(feature = "exig")),
        ("header-options", cfg!(feature = "header-options")),
        ("bare-header-only", cfg!(feature = "bare-header-only")),
        ("xsd", cfg!(feature = "xsd")),
        ("infer", cfg!(feature = "infer")),
        ("dom", cfg!(feature = "dom")),
        ("html", cfg!(feature = "html")),
        ("html5ever", cfg!(feature = "html5ever")),
        ("xhtml5", cfg!(feature = "xhtml5")),
        ("capi", cfg!(feature = "capi")),
    ];
    all.iter().filter(|(_, on)| *on).map(|(n, _)| *n).collect()
}

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn first_match(path: &str, key: &str) -> String {
    read(path)
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

/// `name version` pairs from Cargo.lock for these packages.
fn locked(names: &[&str]) -> String {
    let lock = read(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock"));
    let mut out = Vec::new();
    let mut lines = lock.lines();
    while let Some(l) = lines.next() {
        let Some(n) =
            l.strip_prefix("name = \"").and_then(|n| n.strip_suffix('"'))
        else {
            continue;
        };
        if !names.contains(&n) {
            continue;
        }
        if let Some(v) = lines
            .next()
            .and_then(|v| v.strip_prefix("version = \""))
            .and_then(|v| v.strip_suffix('"'))
        {
            out.push(format!("{n} {v}"));
        }
    }
    out.join(", ")
}
