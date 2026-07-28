//! Process lifecycle.
//!
//! On exit two things must happen, and they belong to different owners. The
//! transient sleep timestamp is the kernel's to drop: it is scratch state, not
//! accounting, and keeping it would leak. The accumulated CPU and I/O-wait
//! counters are user space's to close: they live in per-CPU maps this program
//! cannot sum, so the exit only announces the departure and leaves the totals
//! standing for the next collector tick to read and then remove (see
//! `exit_notify`).

use aya_ebpf::{macros::raw_tracepoint, programs::RawTracePointContext};

use crate::{exit_notify, iowait, task::Task};

#[raw_tracepoint(tracepoint = "sched_process_exit")]
pub fn sched_process_exit(ctx: RawTracePointContext) -> i32 {
    // args: (*p).
    let task = Task::arg(&ctx, 0);
    let tid = task.pid();
    if tid == 0 {
        return 0;
    }
    let tgid = task.tgid();

    iowait::forget_thread(tid);
    // Not `tid == tgid`: a leader that calls `pthread_exit` becomes a zombie
    // while its threads run on, and announcing there strips a live process of
    // its counters and its name.
    if task.group_is_dead() {
        exit_notify::announce(tgid);
    }
    0
}
