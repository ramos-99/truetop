//! Process identity as truetop reports it. Run: `cargo xtask test`.

use std::{thread::sleep, time::Duration};

use anyhow::{Result, bail};
use truetop::attach;

const NOBODY: u32 = 65534;
const SAMPLES: usize = 6;
const INTERVAL: Duration = Duration::from_millis(200);

/// The uid is captured on `exec` and `fork`, so a process that calls `setuid`
/// afterwards reports the user it was born with - here root, since the suite
/// runs as root - unless the credential change is hooked too.
#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn a_process_that_drops_privileges_reports_its_new_user() -> Result<()> {
    let (_ebpf, mut collector) = attach()?;
    let child = Child::dropping_to(NOBODY)?;

    let mut users = Vec::new();
    for _ in 0..SAMPLES {
        sleep(INTERVAL);
        let snapshot = collector.tick();
        let row = snapshot.processes.iter().find(|p| p.pid == child.tgid());
        users.push(row.and_then(|p| p.user.clone()));
    }

    let dropped = users.iter().flatten().any(|user| user != "root");
    assert!(
        dropped,
        "pid {} still reads as root: {users:?}",
        child.tgid()
    );
    Ok(())
}

/// A child that drops to `uid` and spins. Raw syscalls: glibc's `setuid`
/// broadcasts to sibling threads, which a forked child should not be doing.
struct Child(libc::pid_t);

impl Child {
    fn dropping_to(uid: u32) -> Result<Self> {
        // SAFETY: the child makes only raw syscalls; `alarm` caps it if this
        // process dies first.
        match unsafe { libc::fork() } {
            -1 => bail!("fork: {}", std::io::Error::last_os_error()),
            0 => unsafe {
                libc::alarm(20);
                libc::syscall(libc::SYS_setgid, uid);
                libc::syscall(libc::SYS_setuid, uid);
                loop {
                    std::hint::spin_loop();
                }
            },
            pid => Ok(Self(pid)),
        }
    }

    fn tgid(&self) -> u32 {
        self.0 as u32
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: our own child, and we are still root.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            libc::waitpid(self.0, std::ptr::null_mut(), 0);
        }
    }
}
