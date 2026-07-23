//! Machine-wide facts for the header. The specs never change, so they are read
//! once at startup; memory and load are one small `/proc` file each per tick,
//! which is a constant cost rather than a per-process one.

use std::fs;

/// Constant properties of the host, read once.
pub struct Machine {
    pub hostname: String,
    pub cpu_model: String,
    pub cores: usize,
    pub memory_total_bytes: u64,
}

impl Machine {
    pub fn detect(cores: usize) -> Self {
        Self {
            hostname: first_line("/proc/sys/kernel/hostname").unwrap_or_else(|| "localhost".into()),
            cpu_model: cpu_model().unwrap_or_else(|| "unknown cpu".into()),
            cores,
            memory_total_bytes: memory_total_bytes(),
        }
    }
}

pub fn memory_total_bytes() -> u64 {
    meminfo_field("MemTotal:").unwrap_or(0)
}

/// Memory in use: total minus available, the figure `free` reports as used.
pub fn memory_used_bytes(total: u64) -> u64 {
    total.saturating_sub(meminfo_field("MemAvailable:").unwrap_or(total))
}

/// The one-minute load average.
pub fn load_average() -> f64 {
    first_line("/proc/loadavg")
        .and_then(|line| line.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

fn first_line(path: &str) -> Option<String> {
    Some(fs::read_to_string(path).ok()?.lines().next()?.trim().into())
}

fn cpu_model() -> Option<String> {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").ok()?;
    let line = cpuinfo
        .lines()
        .find(|line| line.starts_with("model name"))?;
    Some(line.split_once(':')?.1.trim().into())
}

/// A `/proc/meminfo` row, in bytes; the file reports kibibytes.
fn meminfo_field(key: &str) -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find(|line| line.starts_with(key))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}
