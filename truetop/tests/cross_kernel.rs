//! Cross-kernel smoke tests, run inside the vmtest matrix on every kernel and
//! distro (see `.github/workflows/vm-matrix.yml`), via `cargo xtask test cross_kernel`.
//!
//! The guest is deliberately minimal: a 9p/virtiofs rootfs (no block device) and
//! virtualized, oversubscribed timing. So these assert only what is robust in any
//! environment - that truetop loads, attaches, and collects on this kernel,
//! exercising the CO-RE offsets, the verifier, and the tracepoint attach.
//!
//! The magnitude oracles (CPU vs `/proc`, I/O wait vs `delayacct`) need real
//! hardware - a block device and faithful timing - so they live in `cpu.rs` and
//! `iowait.rs` and run on the host, never here.

use anyhow::Result;
use truetop::attach;

/// truetop loads, attaches, and collects on this kernel: after a scheduling round
/// it sees its own process in a snapshot. Needs no block device or faithful timing.
#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test cross_kernel"]
fn attaches_and_collects() -> Result<()> {
    let (_ebpf, mut collector) = attach()?;
    let me = std::process::id();

    // Seed deltas, burn a little CPU so this process is scheduled with recorded
    // time, then sample - it must then see itself.
    collector.tick();
    let mut x = 0u64;
    for i in 0..100_000_000u64 {
        x = x.wrapping_add(i.rotate_left(3));
    }
    std::hint::black_box(x);
    let snapshot = collector.tick();

    assert!(
        snapshot.processes.iter().any(|p| p.pid == me),
        "truetop did not see its own process ({me}) after attaching"
    );
    Ok(())
}
