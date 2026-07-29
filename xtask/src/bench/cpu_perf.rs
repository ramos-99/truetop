//! Best-effort CPU tuning for stable benchmark numbers, reverted on drop.

use std::process::Command;

const GOVERNOR0: &str = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
const GOVERNOR_GLOB: &str = "/sys/devices/system/cpu/cpu*/cpufreq/scaling_governor";
const TURBO_KNOBS: [(&str, &str); 2] = [
    ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
    ("/sys/devices/system/cpu/cpufreq/boost", "0"),
];

/// Tuning that reverts on drop. Every field is `Some` only if we changed it, so
/// restore touches exactly what we set and nothing else.
pub struct CpuPerf {
    governor: Option<String>,
    turbo: Option<(&'static str, String)>,
}

impl CpuPerf {
    /// Returns `None` if nothing could be tuned (no knobs, or no privileges), so
    /// the run proceeds untuned rather than failing.
    pub fn engage() -> Option<Self> {
        let mut perf = Self {
            governor: None,
            turbo: None,
        };

        let mut any_knob = false;

        // Governor: read cpu0's current value, switch every CPU to performance.
        if let Some(orig) = read_trim(GOVERNOR0) {
            any_knob = true;
            if orig == "performance" {
                eprintln!("cpu: governor already performance");
            } else if write_glob(GOVERNOR_GLOB, "performance") {
                eprintln!("cpu: governor {orig} -> performance");
                perf.governor = Some(orig);
            } else {
                eprintln!("cpu: could not set governor (need sudo?)");
            }
        }

        // Turbo: whichever knob this kernel exposes.
        for (path, off) in TURBO_KNOBS {
            let Some(orig) = read_trim(path) else {
                continue;
            };
            any_knob = true;
            if orig == off {
                eprintln!("cpu: turbo already off");
            } else if write_one(path, off) {
                eprintln!("cpu: turbo off ({path}: {orig} -> {off})");
                perf.turbo = Some((path, orig));
            } else {
                eprintln!("cpu: could not disable turbo (need sudo?)");
            }
            break;
        }

        if !any_knob {
            eprintln!("cpu: no cpufreq knobs found; numbers may be noisy");
        }

        // Restore only runs for what we actually changed.
        if perf.governor.is_none() && perf.turbo.is_none() {
            return None;
        }
        Some(perf)
    }
}

impl Drop for CpuPerf {
    fn drop(&mut self) {
        if let Some(orig) = &self.governor {
            write_glob(GOVERNOR_GLOB, orig);
            eprintln!("cpu: governor restored to {orig}");
        }
        if let Some((path, orig)) = &self.turbo {
            write_one(path, orig);
            eprintln!("cpu: turbo restored ({path}: {orig})");
        }
    }
}

/// The current scaling governor of cpu0, if the knob exists.
pub fn current_governor() -> Option<String> {
    read_trim(GOVERNOR0)
}

/// Whichever turbo knob this kernel exposes, and its current value - not
/// whether `CpuPerf` asked to disable it, but what the kernel reports now.
pub fn current_turbo() -> Option<(&'static str, String)> {
    TURBO_KNOBS
        .iter()
        .find_map(|&(path, _)| read_trim(path).map(|value| (path, value)))
}

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
}

/// Write one sysfs file as root. Values here are our own constants, never input.
fn write_one(path: &str, val: &str) -> bool {
    sudo_sh(&format!("echo {val} > {path}"))
}

/// Write a value to every file matching a glob (the shell expands it; `tee`
/// fans out where a single `>` redirect cannot).
fn write_glob(glob: &str, val: &str) -> bool {
    sudo_sh(&format!("echo {val} | tee {glob} >/dev/null"))
}

fn sudo_sh(script: &str) -> bool {
    Command::new("sudo")
        .args(["sh", "-c", script])
        .status()
        .is_ok_and(|s| s.success())
}
