//! Cross-kernel smoke tests, run inside the vmtest matrix on every kernel and
//! distro (see `.github/workflows/vm-matrix.yml`), via `cargo xtask test cross_kernel`.
//!
//! The guest is deliberately minimal: a 9p/virtiofs rootfs (no block device) and
//! virtualized, oversubscribed timing. So these assert only what is robust in any
//! environment - that truetop loads, attaches, collects, and names processes on
//! this kernel, exercising the CO-RE offsets, the verifier, and the tracepoint
//! attach.
//!
//! The magnitude oracles (CPU vs `/proc`, I/O wait vs `delayacct`) need real
//! hardware - a block device and faithful timing - so they live in `cpu.rs` and
//! `iowait.rs` and run on the host, never here.

use std::{fs, thread, time::Duration};

use anyhow::{Result, bail};
use truetop::attach;

/// A child forked from this process that never calls `execve`, spinning so the
/// collector ranks it into the snapshot. Killed and reaped on drop.
struct ForkedChild(libc::pid_t);

impl ForkedChild {
    fn spawn() -> Result<Self> {
        // SAFETY: the child makes only async-signal-safe calls, and `alarm` caps it
        // if the parent dies before reaping.
        match unsafe { libc::fork() } {
            -1 => bail!("fork: {}", std::io::Error::last_os_error()),
            0 => unsafe {
                libc::alarm(10);
                let mut spin = 0u64;
                loop {
                    spin = spin.wrapping_add(1);
                    std::hint::black_box(spin);
                }
            },
            pid => Ok(Self(pid)),
        }
    }

    fn pid(&self) -> u32 {
        self.0 as u32
    }
}

impl Drop for ForkedChild {
    fn drop(&mut self) {
        // SAFETY: signalling and reaping our own child.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            libc::waitpid(self.0, std::ptr::null_mut(), 0);
        }
    }
}

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

/// A child that forks and never execs inherits the parent's `comm`, and the fork
/// hook must record it - this is how PostgreSQL backends and nginx workers are
/// spawned, and the `exec` hook alone would leave them unnamed.
#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test cross_kernel"]
fn names_a_child_that_never_execs() -> Result<()> {
    // The child inherits the comm of the forking *thread*, so that is the oracle.
    let expected = fs::read_to_string("/proc/thread-self/comm")?
        .trim()
        .to_owned();

    let (_ebpf, mut collector) = attach()?;

    // Fork before the baseline tick: the counting tick needs a prior sample to
    // diff against, or the child reads 0% and can fall outside the row cap on a
    // busy machine - which is what made this flaky in CI.
    let child = ForkedChild::spawn()?;
    thread::sleep(Duration::from_millis(300));
    collector.tick();
    thread::sleep(Duration::from_millis(300));
    let snapshot = collector.tick();

    let named = snapshot
        .processes
        .iter()
        .find(|p| p.pid == child.pid())
        .map(|p| p.name.as_str());
    assert_eq!(
        named,
        Some(expected.as_str()),
        "fork-only child {} should inherit the parent's comm",
        child.pid()
    );
    Ok(())
}

/// A process that exits between two collector reads must still be charged its
/// time on the read that follows, then be gone on the next. This is the whole
/// point of leaving the counters standing for user space to reap: the exit hook
/// used to delete them in the kernel, so a short-lived process vanished with its
/// CPU time uncounted - exactly the work `make -j` is made of.
#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test cross_kernel"]
fn accounts_for_a_process_that_exits_between_reads() -> Result<()> {
    let (_ebpf, mut collector) = attach()?;

    let child = ForkedChild::spawn()?;
    let pid = child.pid();

    // Let the child accrue CPU, then take the baseline the delta is measured from.
    thread::sleep(Duration::from_millis(300));
    collector.tick();

    // More CPU, then end it: SIGKILL and reap, so the exit tracepoint has fired
    // by the time the next read runs.
    thread::sleep(Duration::from_millis(300));
    drop(child);

    let after_exit = collector.tick();
    assert!(
        after_exit.processes.iter().any(|p| p.pid == pid),
        "a process that exited this interval must still be accounted for ({pid})"
    );

    let next = collector.tick();
    assert!(
        !next.processes.iter().any(|p| p.pid == pid),
        "the accounted process must be reaped the following tick ({pid})"
    );
    Ok(())
}
