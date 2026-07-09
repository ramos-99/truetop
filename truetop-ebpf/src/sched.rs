//! The `sched_switch` hotpath. One program, fanned out to every metric that
//! feeds off scheduler edges (`cpu`, `iowait`), so adding a metric never adds
//! a second program to the same tracepoint.

use aya_ebpf::{
    Global, helpers::bpf_ktime_get_ns, macros::raw_tracepoint, programs::RawTracePointContext,
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
    let prev = Task::arg(&ctx, 1);
    let next = Task::arg(&ctx, 2);
    let prev_tid = prev.pid();
    let next_tid = next.pid();

    // From the tracepoint arg where the kernel provides it (a register), else
    // probe-read from the task. One binary covers 5.10+.
    let prev_state = if HAS_PREV_STATE.load() != 0 {
        ctx.arg::<u32>(3)
    } else {
        prev.state()
    };

    cpu::charge_out(prev_tid, prev.tgid(), now);
    cpu::mark_in(next_tid, now);
    iowait::sleep_out(prev_state, prev_tid, now);
    iowait::wake_in(&next, next_tid, now);
    0
}
