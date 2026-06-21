//! Process lifecycle. The exit hook fans cleanup out to every subsystem so no
//! map accumulates dead pids (CLAUDE.md §2); each `forget` owns its own
//! thread- vs process-scoped policy.

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
    let tgid = task.tgid();
    cpu::forget(tid, tgid);
    comm::forget(tid, tgid);
    0
}
