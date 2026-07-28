//! Everything CPU-side that touches a kernel map: reading the accumulated
//! `CPU_NS` counter, correcting it for the thread still running when the tick
//! reads it, and deleting a departed process's entry once reaped. The
//! per-interval percentage math stays in `metrics::cpu` - this module is the
//! I/O boundary that math runs on top of.
//!
//! The `sched_switch` hotpath only bills a thread's on-CPU slice when it
//! switches out (`cpu::charge_out`), so a thread that runs a whole tick
//! without ever being preempted is never billed - it would read 0% forever.
//! `START_TIME` and `CURRENT_TGID` (one slot per CPU) record who is running
//! and since when; [`CpuMaps::read`] adds that in-flight slice to the
//! accumulated counter before the backend diffs it, so the correction lives
//! entirely on the read path and the hotpath stays untouched.

use std::collections::HashMap;

use aya::maps::{MapData, MapType, PerCpuArray};

use crate::{batch::BatchReader, metrics::Snapshot};

pub(crate) struct CpuMaps {
    cpu_ns: MapData,
    start_time: PerCpuArray<MapData, u64>,
    current_tgid: PerCpuArray<MapData, u32>,
}

impl CpuMaps {
    pub(crate) fn new(
        cpu_ns: MapData,
        start_time: PerCpuArray<MapData, u64>,
        current_tgid: PerCpuArray<MapData, u32>,
    ) -> Self {
        Self {
            cpu_ns,
            start_time,
            current_tgid,
        }
    }

    /// The accumulated `CPU_NS` counter, corrected with each CPU's in-flight
    /// slice for the thread currently running on it.
    pub(crate) fn read(&self, reader: &mut BatchReader) -> Snapshot {
        let mut snapshot = reader.read_counter(&self.cpu_ns);
        self.add_inflight(&mut snapshot.by_pid);
        snapshot
    }

    /// The counter map, for the reaper to delete a departed process's entry
    /// once the tick has read its final total.
    pub(crate) fn map_mut(&mut self) -> &mut MapData {
        &mut self.cpu_ns
    }

    /// The kernel's own view of the counter map: its type and its capacity.
    pub(crate) fn kind_and_capacity(&self) -> anyhow::Result<(MapType, u32)> {
        let info = self.cpu_ns.info()?;
        Ok((info.map_type()?, info.max_entries()))
    }

    fn add_inflight(&self, by_pid: &mut HashMap<u32, u64>) {
        let (Ok(tgids), Ok(starts)) = (self.current_tgid.get(&0, 0), self.start_time.get(&0, 0))
        else {
            return;
        };
        let now = monotonic_ns();
        for (&tgid, &start) in tgids.iter().zip(starts.iter()) {
            if tgid != 0 && start != 0 && now > start {
                *by_pid.entry(tgid).or_insert(0) += now - start;
            }
        }
    }
}

/// `now` in the clock `bpf_ktime_get_ns` reads: `CLOCK_MONOTONIC`, time since
/// boot excluding suspend - so it is directly comparable to `START_TIME`.
fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: CLOCK_MONOTONIC and a valid out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}
