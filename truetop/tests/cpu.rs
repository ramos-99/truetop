//! End-to-end CPU accounting: truetop's CPU% must track `/proc` for a known
//! busy process. Run: `cargo xtask test`.

mod common;

use std::{
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use common::{clk_tck, proc_cpu_ticks, spawn};
use truetop::attach;

#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn cpu_percent_matches_proc() -> Result<()> {
    let (_ebpf, mut collector) = attach()?;

    let busy = spawn("sh", &["-c", "while :; do :; done"])?;
    let idle = spawn("sleep", &["10"])?;
    let busy_pid = busy.0.id();
    let idle_pid = idle.0.id();

    sleep(Duration::from_millis(200));
    collector.tick();
    let ticks0 = proc_cpu_ticks(busy_pid)?;
    let t0 = Instant::now();

    sleep(Duration::from_secs(2));

    let ticks1 = proc_cpu_ticks(busy_pid)?;
    let elapsed = t0.elapsed().as_secs_f64();
    let snapshot = collector.tick();

    // Ground truth: fraction of one core the busy process used.
    let proc_fraction = (ticks1 - ticks0) as f64 / clk_tck() / elapsed;
    assert!(
        proc_fraction > 0.5,
        "spinner not CPU-bound: {proc_fraction:.2}"
    );

    let busy_row = snapshot
        .processes
        .iter()
        .find(|p| p.pid == busy_pid)
        .context("busy process not seen by truetop")?;
    // truetop reports top's %: 100.0 is one core, so scale to a one-core fraction.
    let truetop_fraction = busy_row.cpu.cpu_percent / 100.0;
    assert!(
        (truetop_fraction - proc_fraction).abs() < 0.3,
        "truetop {truetop_fraction:.2} vs proc {proc_fraction:.2}"
    );

    if let Some(idle_row) = snapshot.processes.iter().find(|p| p.pid == idle_pid) {
        assert!(
            idle_row.cpu.cpu_percent < 1.0,
            "idle at {}%",
            idle_row.cpu.cpu_percent
        );
    }

    Ok(())
}
