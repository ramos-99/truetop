//! CPU utilisation via `sched_switch` (CLAUDE.md §2). Per-CPU, add-only
//! counters; user space derives percentages from cross-tick deltas.
//!
//! Time is charged per thread but accumulated per process: each thread keeps
//! its own schedule-in stopwatch (`START_TIME`, keyed by tid), and its slice is
//! added to the process total (`CPU_NS`, keyed by tgid) — matching top/btop.

use aya_ebpf::{
    helpers::bpf_ktime_get_ns,
    macros::{map, raw_tracepoint},
    maps::PerCpuHashMap,
    programs::RawTracePointContext,
};

use crate::task::Task;

// tid → schedule-in timestamp (per-thread stopwatch).
#[map]
static START_TIME: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(16384, 0);
// tgid → accumulated on-CPU nanoseconds (the counter user space diffs).
#[map]
static CPU_NS: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(16384, 0);

#[raw_tracepoint(tracepoint = "sched_switch")]
pub fn sched_switch(ctx: RawTracePointContext) -> i32 {
    // args: (bool preempt, *prev, *next, [prev_state]).
    let now = unsafe { bpf_ktime_get_ns() };
    let prev = Task::arg(&ctx, 1);
    charge_out(prev.pid(), prev.tgid(), now);
    mark_in(Task::arg(&ctx, 2).pid(), now);
    0
}

/// Drop a dead task's CPU state; called by the shared exit hook. The stopwatch
/// is per-thread; the accumulator is per-process, so reap it on the leader only.
#[inline(always)]
pub(crate) fn forget(tid: u32, tgid: u32) {
    let _ = START_TIME.remove(tid);
    if tid == tgid {
        let _ = CPU_NS.remove(tgid);
    }
}

/// Add the slice the outgoing thread just ran to its process total.
#[inline(always)]
fn charge_out(tid: u32, tgid: u32, now: u64) {
    if tid == 0 {
        return;
    }
    let Some(start) = (unsafe { START_TIME.get(tid) }).copied() else {
        return;
    };
    let total = (unsafe { CPU_NS.get(tgid) }).copied().unwrap_or(0);
    let _ = CPU_NS.insert(tgid, total.saturating_add(now.saturating_sub(start)), 0);
}

/// Stamp the incoming thread's schedule-in time.
#[inline(always)]
fn mark_in(tid: u32, now: u64) {
    if tid == 0 {
        return;
    }
    let _ = START_TIME.insert(tid, now, 0);
}
