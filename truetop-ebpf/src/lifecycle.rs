//! Process lifecycle. A single `exit` hook fans cleanup out to every subsystem
//! so no map accumulates dead pids (CLAUDE.md §2).
//!
//! `sched_process_exit` fires per *thread*: drop the thread's stopwatch always,
//! but only reap the process-keyed maps when the group leader (`tid == tgid`)
//! goes — otherwise one dying thread would wipe a live process.

use aya_ebpf::{macros::raw_tracepoint, programs::RawTracePointContext};

use crate::{comm, cpu, task::Task};

#[raw_tracepoint(tracepoint = "sched_process_exit")]
pub fn sched_process_exit(ctx: RawTracePointContext) -> i32 {
    // args: (*p).
    let task = Task::arg(&ctx, 0);
    let tid = task.pid();
    if tid == 0 {
        return 0;
    }
    cpu::forget_thread(tid);

    let tgid = task.tgid();
    if tid == tgid {
        cpu::forget_process(tgid);
        comm::forget(tgid);
    }
    0
}
