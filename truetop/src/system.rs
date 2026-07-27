//! Machine-wide facts: the CPU topology both halves of truetop are sized by, and
//! the host specs the header shows. All fixed, so they are read once at startup;
//! memory and load are one small `/proc` file each per tick, which is a constant
//! cost rather than a per-process one.

use std::fs;

use anyhow::{Context as _, ensure};
use aya::util::nr_cpus;

/// The two CPU counts truetop needs, resolved together once at startup and
/// logged there. `possible` is the per-CPU map stride, from the same sysfs file
/// the kernel allocates by; `online` is what the header calls cores and what
/// utilisation is a share of. They agree on a desktop and diverge on a VM that
/// reserves more CPU slots than it boots, so they are one type rather than two
/// interchangeable `usize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuCounts {
    pub possible: usize,
    pub online: usize,
}

impl CpuCounts {
    pub fn detect() -> anyhow::Result<Self> {
        let possible = nr_cpus()
            .map_err(|(source, err)| anyhow::anyhow!("{source}: {err}"))
            .context("reading the possible CPU count")?;
        let online = online_cpus()?;
        // A topology we cannot read makes every number downstream wrong, so this
        // fails rather than clamping into a plausible-looking lie.
        ensure!(
            (1..=possible).contains(&online),
            "implausible CPU topology: {online} online, {possible} possible"
        );
        Ok(Self { possible, online })
    }
}

fn online_cpus() -> anyhow::Result<usize> {
    // SAFETY: sysconf takes a name and returns a long; no pointers are involved.
    let count = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    usize::try_from(count).context("reading the online CPU count")
}

/// Constant properties of the host, read once.
pub struct Machine {
    pub hostname: String,
    pub cpu_model: String,
    pub cores: usize,
    pub memory_total_bytes: u64,
}

impl Machine {
    /// Takes [`CpuCounts`] rather than a bare number so the core count cannot be
    /// sourced from anywhere else.
    pub fn detect(cpus: CpuCounts) -> Self {
        Self {
            hostname: first_line("/proc/sys/kernel/hostname").unwrap_or_else(|| "localhost".into()),
            cpu_model: cpu_model().unwrap_or_else(|| "unknown cpu".into()),
            cores: cpus.online,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One `processor` block per online CPU: an independent path to the number
    /// sysconf reports.
    fn cpuinfo_processors() -> usize {
        fs::read_to_string("/proc/cpuinfo")
            .expect("/proc/cpuinfo")
            .lines()
            .filter(|line| line.starts_with("processor"))
            .count()
    }

    #[test]
    fn the_online_count_agrees_with_proc_cpuinfo() {
        let cpus = CpuCounts::detect().expect("CPU topology");
        assert_eq!(cpus.online, cpuinfo_processors());
    }

    #[test]
    fn the_host_reports_its_installed_memory() {
        assert!(Machine::detect(CpuCounts::detect().expect("CPU topology")).memory_total_bytes > 0);
    }
}
