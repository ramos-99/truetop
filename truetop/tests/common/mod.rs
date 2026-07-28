//! Shared harness for the eBPF integration tests - they need root and a live
//! kernel, so are `#[ignore]`d. Run with `cargo xtask test`.
//!
//! Each test binary compiles this module separately, so a helper another binary
//! uses reads as dead here.
#![allow(dead_code)]

use std::process::{Child, Command};

use anyhow::{Context as _, Result};

pub struct ChildGuard(pub Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn spawn(program: &str, args: &[&str]) -> Result<ChildGuard> {
    Command::new(program)
        .args(args)
        .spawn()
        .map(ChildGuard)
        .with_context(|| format!("spawning {program}"))
}

/// User+system CPU ticks from `/proc/<pid>/stat`, parsed after the last `)`
/// since comm may contain spaces (utime/stime are fields 14/15).
pub fn proc_cpu_ticks(pid: u32) -> Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let after_comm = stat.rsplit_once(')').context("malformed stat")?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime: u64 = fields[11].parse()?;
    let stime: u64 = fields[12].parse()?;
    Ok(utime + stime)
}

pub fn clk_tck() -> f64 {
    // SAFETY: sysconf with a valid name is always safe to call.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz > 0 { hz as f64 } else { 100.0 }
}
