//! Announcing departed processes to user space.
//!
//! The counters a process leaves behind sit in per-CPU maps, and an eBPF program
//! can read no slot but the one belonging to the CPU it runs on. A departing
//! process's totals are therefore unknowable here: only user space, which
//! already sums every slot in one batched read, can close the books. So the exit
//! path publishes identity rather than numbers, and deliberately leaves the
//! accounting maps standing. The next collector tick reads the complete totals
//! and only then removes the entries (see `truetop/src/reaper.rs`).
//!
//! A ring buffer rather than a map, following the kernel's own exit-tracing
//! idiom: user space drains it through shared memory, paying no syscall per
//! record, and records arrive in the order they were written.

use aya_ebpf::{macros::map, maps::RingBuf};
use truetop_common::ExitEvent;

/// Room for far more departures than a second of heavy churn produces. A full
/// ring drops records, which delays a reap rather than losing accounting: the
/// tick still reads the totals, and an entry nobody reaps is one nothing charges
/// either, so it goes cold and the LRU evicts it (see `cpu`).
#[map]
static EXITS: RingBuf = RingBuf::with_byte_size(256 * 1024, 0);

/// Announce that `tgid` has ended. Callers filter to the thread-group leader:
/// a thread's death does not end the process the counters are keyed by.
#[inline(always)]
pub(crate) fn announce(tgid: u32) {
    let _ = EXITS.output::<ExitEvent>(&ExitEvent { tgid }, 0);
}
