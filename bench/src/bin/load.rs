//! Active load for the macro benchmark: `load N` forks N processes that keep
//! rescheduling at ~0% CPU, so they appear to every monitor (idle processes
//! never reschedule, so truetop wouldn't see them). Children die with the
//! parent via `PR_SET_PDEATHSIG`.

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(0);

    for _ in 0..n {
        // SAFETY: single-threaded program; the child only calls
        // async-signal-safe libc functions (no allocation) and never returns.
        if unsafe { libc::fork() } == 0 {
            let delay = libc::timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            };
            unsafe {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                loop {
                    libc::nanosleep(&delay, std::ptr::null_mut());
                }
            }
        }
    }

    // SAFETY: blocks until run.sh signals us; the children exit with us.
    unsafe { libc::pause() };
}
