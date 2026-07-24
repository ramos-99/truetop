//! Process churn for the self-cpu benchmark: a bounded set of workers each fork a
//! short-lived child and reap it in a tight loop, sustaining a high fork+exit
//! rate at a low live count - the turnover a parallel build produces. This is the
//! load that exercises truetop's exit-reaping path, which the steady `load`
//! process count never does. `churn [WORKERS]` (default 4); workers die with the
//! parent via `PR_SET_PDEATHSIG`.

fn main() {
    let workers: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(4);

    for _ in 0..workers {
        // SAFETY: single-threaded program; the child only calls async-signal-safe
        // libc functions (no allocation) and never returns.
        if unsafe { libc::fork() } == 0 {
            unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
            churn();
        }
    }

    // SAFETY: blocks until run.sh signals us; the workers exit with us.
    unsafe { libc::pause() };
}

/// Fork a short-lived child and reap it, forever. The child naps ~1 ms so it is
/// scheduled (and thus seen) before exiting, without pegging a core.
fn churn() -> ! {
    let nap = libc::timespec {
        tv_sec: 0,
        tv_nsec: 1_000_000,
    };
    loop {
        // SAFETY: single-threaded; the child path is async-signal-safe and exits.
        match unsafe { libc::fork() } {
            0 => unsafe {
                libc::nanosleep(&nap, std::ptr::null_mut());
                libc::_exit(0);
            },
            // A limit (e.g. RLIMIT_NPROC); back off briefly and retry.
            -1 => unsafe {
                libc::nanosleep(&nap, std::ptr::null_mut());
            },
            pid => unsafe {
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            },
        }
    }
}
