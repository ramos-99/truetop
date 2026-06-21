//! Process identity capture. `comm` is written only on the cold `exec` path
//! (and dropped on `exit`), never on `sched_switch`, so the hotpath stays
//! string-free (CLAUDE.md §2/§6).
//!
//! This map is intentionally a global `HashMap`, not per-CPU: the §2 per-CPU
//! rule targets hotpath spinlock contention, which a cold-path write does not
//! incur, and a per-process name is not per-CPU data — only the core that ran
//! the `exec` would hold a per-CPU copy.

use aya_ebpf::{
    helpers::{TASK_COMM_LEN, bpf_get_current_comm},
    macros::{map, raw_tracepoint},
    maps::HashMap,
    programs::RawTracePointContext,
};
use truetop_common::COMM_LEN;

use crate::task::Task;

// The shared wire width must match the kernel's; lock it at compile time.
const _: () = assert!(COMM_LEN == TASK_COMM_LEN);

// tgid → comm (NUL-padded).
#[map]
static COMM_MAP: HashMap<u32, [u8; COMM_LEN]> = HashMap::with_max_entries(16384, 0);

#[raw_tracepoint(tracepoint = "sched_process_exec")]
pub fn sched_process_exec(ctx: RawTracePointContext) -> i32 {
    // args: (*p, old_pid, *bprm); `comm` already holds the new program name.
    let tgid = Task::arg(&ctx, 0).tgid();
    if tgid != 0
        && let Ok(comm) = bpf_get_current_comm()
    {
        let _ = COMM_MAP.insert(tgid, comm, 0);
    }
    0
}

/// Drop a dead process's name; called by the shared exit hook on the leader.
#[inline(always)]
pub(crate) fn forget(tid: u32, tgid: u32) {
    if tid == tgid {
        let _ = COMM_MAP.remove(tgid);
    }
}
