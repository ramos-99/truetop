//! Process lifecycle as truetop sees it. Run: `cargo xtask test`.

use std::{ptr, thread::sleep, time::Duration};

use anyhow::{Result, bail};
use truetop::attach;

const SAMPLES: usize = 8;
const INTERVAL: Duration = Duration::from_millis(200);

/// `sched_process_exit` fires for every task, so a leader that exits while its
/// threads run reaches the hook with `pid == tgid` though the process is alive.
/// Announcing a departure there deletes the process's counters and its name, and
/// nothing rewrites the name: it will never `exec` again.
#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn a_group_that_outlives_its_leader_keeps_its_name() -> Result<()> {
    let (_ebpf, mut collector) = attach()?;
    let child = Child::with_leader_exited()?;

    // It spins for the whole window, so every sample must find it, named. Each
    // sample carries the kernel's view beside truetop's: if the group died, the
    // fault is this test's, not the collector's.
    let seen: Vec<(String, Option<String>)> = (0..SAMPLES)
        .map(|_| {
            sleep(INTERVAL);
            let snapshot = collector.tick();
            let row = snapshot
                .processes
                .iter()
                .find(|p| p.pid == child.tgid())
                .map(|p| p.name.clone());
            (child.kernel_state(), row)
        })
        .collect();

    assert!(
        seen.iter()
            .all(|(_, name)| name.as_deref().is_some_and(|name| name != "<unknown>")),
        "pid {}: {seen:#?}",
        child.tgid()
    );
    Ok(())
}

/// `clone(2)` flags for a thread: same address space, same thread group.
const AS_THREAD: libc::c_int =
    libc::CLONE_VM | libc::CLONE_FS | libc::CLONE_FILES | libc::CLONE_SIGHAND | libc::CLONE_THREAD;
const WORKER_STACK: usize = 128 << 10;

/// A child whose leader exits while a worker spins on: the leader is a zombie,
/// the thread group runs. Cloned rather than `pthread_create`d, which is not
/// usable after forking a threaded process. Killed on drop.
struct Child(libc::pid_t);

impl Child {
    fn with_leader_exited() -> Result<Self> {
        // Mapped before the fork: the child must not allocate.
        // SAFETY: a fresh anonymous mapping.
        let stack = unsafe {
            libc::mmap(
                ptr::null_mut(),
                WORKER_STACK,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_STACK,
                -1,
                0,
            )
        };
        if stack == libc::MAP_FAILED {
            bail!("mmap: {}", std::io::Error::last_os_error());
        }

        // SAFETY: the child makes only raw syscalls, on a stack it owns; `alarm`
        // caps it if this process dies first.
        match unsafe { libc::fork() } {
            -1 => bail!("fork: {}", std::io::Error::last_os_error()),
            0 => unsafe {
                libc::alarm(20);
                let top = (stack as *mut u8).add(WORKER_STACK).cast();
                libc::clone(spin, top, AS_THREAD, ptr::null_mut());
                // The leader alone, as `pthread_exit` does.
                libc::syscall(libc::SYS_exit, 0);
                unreachable!()
            },
            pid => Ok(Self(pid)),
        }
    }

    fn tgid(&self) -> u32 {
        self.0 as u32
    }

    /// `State` and `Threads` from `/proc`: a group that died reads `Z` with one
    /// thread, a group that outlived its leader reads `Z` with two.
    fn kernel_state(&self) -> String {
        let status = std::fs::read_to_string(format!("/proc/{}/status", self.0))
            .unwrap_or_else(|_| "gone".into());
        status
            .lines()
            .filter(|line| line.starts_with("State:") || line.starts_with("Threads:"))
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: our own child; the group still answers to its tgid.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            libc::waitpid(self.0, ptr::null_mut(), 0);
        }
    }
}

extern "C" fn spin(_: *mut libc::c_void) -> libc::c_int {
    loop {
        std::hint::spin_loop();
    }
}
