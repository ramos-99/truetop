//! The `sched_switch` hotpath. One program, fanned out to every metric that
//! feeds off scheduler edges (`cpu`, `iowait`), so adding a metric never adds
//! a second program to the same tracepoint.

use aya_ebpf::{
    Global,
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns},
    macros::raw_tracepoint,
    programs::RawTracePointContext,
};

use crate::{cpu, iowait, task::Task};

// Set at load time (see user-space btf.rs) when the sched_switch tracepoint
// carries prev_state as its 4th arg, which it does from 5.18.
#[unsafe(no_mangle)]
static HAS_PREV_STATE: Global<u32> = Global::new(0);

#[raw_tracepoint(tracepoint = "sched_switch")]
pub fn sched_switch(ctx: RawTracePointContext) -> i32 {
    // args: (bool preempt, *prev, *next, [u32 prev_state on >= 5.18]).
    let now = unsafe { bpf_ktime_get_ns() };

    // The tracepoint fires before the switch, so `current` is still `prev`: this
    // helper returns its tgid:pid with no probe read into the task.
    let prev = bpf_get_current_pid_tgid();
    let prev_tid = prev as u32;
    let prev_tgid = (prev >> 32) as u32;

    let next = Task::arg(&ctx, 2);
    let (next_tid, next_tgid) = next.pid_and_tgid();

    // From the tracepoint arg where the kernel provides it (a register), else
    // probe-read prev's state. One binary covers 5.10+.
    let prev_state = if HAS_PREV_STATE.load() != 0 {
        ctx.arg::<u32>(3)
    } else {
        Task::arg(&ctx, 1).state()
    };

    cpu::charge_out(prev_tid, prev_tgid, now);
    cpu::mark_in(next_tid, next_tgid, now);
    iowait::sleep_out(prev_state, prev_tid, now);
    iowait::wake_in(next_tgid, next_tid, now);
    0
}
