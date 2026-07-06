//! The `sched_switch` hotpath. One program, fanned out to every metric that
//! feeds off scheduler edges (`cpu`, `iowait`), so adding a metric never adds
//! a second program to the same tracepoint.

use aya_ebpf::{helpers::bpf_ktime_get_ns, macros::raw_tracepoint, programs::RawTracePointContext};

use crate::{cpu, iowait, task::Task};

#[raw_tracepoint(tracepoint = "sched_switch")]
pub fn sched_switch(ctx: RawTracePointContext) -> i32 {
    // args: (bool preempt, *prev, *next, [prev_state on >= 5.18]). prev's state
    // is probe-read from the task instead so one code path covers 5.10+.
    let now = unsafe { bpf_ktime_get_ns() };
    let prev = Task::arg(&ctx, 1);
    let next = Task::arg(&ctx, 2);
    let prev_tid = prev.pid();
    let next_tid = next.pid();

    cpu::charge_out(prev_tid, prev.tgid(), now);
    cpu::mark_in(next_tid, now);
    iowait::sleep_out(&prev, prev_tid, now);
    iowait::wake_in(&next, next_tid, now);
    0
}
