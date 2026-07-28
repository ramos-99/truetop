//! Collector (backend) - producer side of the double-buffer (CLAUDE.md §3). Each
//! [`Collector::tick`] diffs the CPU and I/O-wait counters into per-pid maps,
//! enriches the union of the top rows of each with name/user/RSS, and publishes
//! an immutable snapshot of raw values. Sorting and formatting are the
//! renderer's job.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use aya::maps::{HashMap as BpfHashMap, MapData, RingBuf};
use tokio::time::{MissedTickBehavior, interval};
use truetop_common::Identity;

use crate::{
    batch::BatchReader,
    cpu_maps::CpuMaps,
    metrics::{
        CpuCollector, CpuMetrics, IoMetrics, IoWaitCollector, MemReader, ProcessMetrics, Resolver,
    },
    reaper::Reaper,
    system::CpuCounts,
};

/// Rows enriched (name/user/RSS via `/proc`) per sortable metric. The renderer
/// truncates further to the viewport; enriching only the top of each metric
/// keeps `/proc` reads bounded, not O(process count).
const MAX_ROWS: usize = 256;

/// Chart history depth, in ticks.
const HISTORY: usize = 120;

/// Per-pid CPU% and I/O-wait% for one tick.
type Deltas = HashMap<u32, f64>;

/// One tick's snapshot, published whole and never mutated. Raw values only; the
/// renderer formats them, and only for the rows it draws.
#[derive(Debug, Clone, Default)]
pub struct SystemState {
    /// The rows worth enriching - the top of each metric, not every process.
    pub processes: Vec<ProcessMetrics>,
    /// Machine-wide CPU over the last `HISTORY` ticks, oldest first.
    pub cpu_history: VecDeque<f64>,
    /// The same for I/O wait.
    pub io_history: VecDeque<f64>,
    /// CPU used this tick, as a share of the online cores.
    pub total_cpu_percent: f64,
    /// I/O wait this tick, summed across processes rather than normalised.
    pub total_io_percent: f64,
    /// Memory in use: total minus available, the figure `free` reports as used.
    pub memory_used_bytes: u64,
    /// Installed memory.
    pub memory_total_bytes: u64,
    /// The one-minute load average.
    pub load_average: f64,
    /// Processes the kernel is accounting for, which is all of them that have
    /// run, not just the rows above.
    pub tracked: usize,
    /// The counter map is full, so it is evicting to make room. Accounting stays
    /// honest, but a process that has been idle a while may report 0% for one
    /// tick after it runs again.
    pub at_capacity: bool,
}

/// Owns the per-metric collectors; each [`Collector::tick`] reads the maps and
/// produces the next snapshot.
pub struct Collector {
    cpu_maps: CpuMaps,
    iowait_ns: MapData,
    reader: BatchReader,
    cpu: CpuCollector,
    iowait: IoWaitCollector,
    names: Resolver,
    mem: MemReader,
    reaper: Reaper,
    online_cpus: usize,
    capacity: usize,
    memory_total_bytes: u64,
    cpu_history: VecDeque<f64>,
    io_history: VecDeque<f64>,
}

impl Collector {
    pub(crate) fn new(
        cpu_maps: CpuMaps,
        iowait_ns: MapData,
        comm: BpfHashMap<MapData, u32, Identity>,
        exits: RingBuf<MapData>,
        cpus: CpuCounts,
        capacity: u32,
    ) -> Self {
        Self {
            cpu_maps,
            iowait_ns,
            reader: BatchReader::new(cpus.possible),
            cpu: CpuCollector::new(),
            iowait: IoWaitCollector::new(),
            names: Resolver::new(comm),
            mem: MemReader::new(),
            reaper: Reaper::new(exits),
            online_cpus: cpus.online,
            capacity: capacity as usize,
            memory_total_bytes: crate::system::memory_total_bytes(),
            // Filled with a flat baseline so the charts span their panel from the
            // first tick instead of drawing into a mostly empty axis.
            cpu_history: VecDeque::from(vec![0.0; HISTORY]),
            io_history: VecDeque::from(vec![0.0; HISTORY]),
        }
    }

    /// The counter map's type and capacity as the kernel reports them, so a
    /// caller can check the map it got is the map it asked for.
    #[doc(hidden)]
    pub fn cpu_map_kind_and_capacity(&self) -> anyhow::Result<(aya::maps::MapType, u32)> {
        self.cpu_maps.kind_and_capacity()
    }

    pub fn tick(&mut self) -> SystemState {
        let cpu_snap = self.cpu_maps.read(&mut self.reader);
        // Reads a handful high: the in-flight correction can add a running tgid
        // the map has not seen. Noise against a five-figure capacity.
        let tracked = cpu_snap.by_pid.len();
        let cpu = self.cpu.deltas(cpu_snap);
        let io_snap = self.reader.read_counter(&self.iowait_ns);
        let io = self.iowait.deltas(io_snap);

        let processes = select_candidates(&cpu, &io, MAX_ROWS)
            .into_iter()
            .map(|pid| self.enrich(pid, &cpu, &io))
            .collect();

        let total_cpu_percent = cpu.values().sum::<f64>() / self.online_cpus as f64;
        let total_io_percent = io.values().sum();
        push_capped(&mut self.cpu_history, total_cpu_percent);
        push_capped(&mut self.io_history, total_io_percent);

        // The reads above charged every process that ended this interval its full
        // time, including the short-lived ones; only now are those entries dropped.
        self.reaper.reap(
            self.cpu_maps.map_mut(),
            &mut self.iowait_ns,
            &mut self.names,
        );

        SystemState {
            processes,
            cpu_history: self.cpu_history.clone(),
            io_history: self.io_history.clone(),
            total_cpu_percent,
            total_io_percent,
            memory_used_bytes: crate::system::memory_used_bytes(self.memory_total_bytes),
            memory_total_bytes: self.memory_total_bytes,
            load_average: crate::system::load_average(),
            tracked,
            at_capacity: tracked >= self.capacity,
        }
    }

    /// One candidate pid to a full row: its counter values plus name, user, RSS.
    fn enrich(&self, pid: u32, cpu: &Deltas, io: &Deltas) -> ProcessMetrics {
        ProcessMetrics {
            pid,
            name: self.names.resolve(pid),
            user: self.names.user(pid),
            cpu: CpuMetrics {
                cpu_percent: cpu.get(&pid).copied().unwrap_or(0.0),
            },
            mem: self.mem.for_pid(pid),
            io: io.get(&pid).map(|&percent| IoMetrics {
                io_wait_percent: percent,
            }),
        }
    }
}

/// The union of the top-`cap` pids by CPU and by I/O wait: whatever the renderer
/// sorts by, the true top of that metric is present and enriched.
fn select_candidates(cpu: &Deltas, io: &Deltas, cap: usize) -> Vec<u32> {
    let mut pids: HashSet<u32> = HashSet::with_capacity(cap * 2);
    pids.extend(top_pids(cpu, cap));
    pids.extend(top_pids(io, cap));
    pids.into_iter().collect()
}

/// The `cap` highest-valued pids, unordered: the caller unions them into a set,
/// so partitioning around the cut is enough and sorting the rest is waste.
fn top_pids(values: &Deltas, cap: usize) -> Vec<u32> {
    let mut ranked: Vec<(u32, f64)> = values.iter().map(|(&pid, &v)| (pid, v)).collect();
    if ranked.len() > cap {
        ranked.select_nth_unstable_by(cap, |a, b| b.1.total_cmp(&a.1));
        ranked.truncate(cap);
    }
    ranked.into_iter().map(|(pid, _)| pid).collect()
}

fn push_capped(buf: &mut VecDeque<f64>, value: f64) {
    if buf.len() >= HISTORY {
        buf.pop_front();
    }
    buf.push_back(value);
}

/// Drive the double-buffer at 1 Hz until the task is dropped at shutdown.
pub async fn collector_loop(shared: Arc<ArcSwap<SystemState>>, mut collector: Collector) {
    let mut ticker = interval(Duration::from_millis(1000));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        shared.store(Arc::new(collector.tick()));
    }
}

/// Run `ticks` collector iterations at 1 Hz with no UI, for tracing the
/// collection path without the render loop's syscalls.
pub fn run_headless(mut collector: Collector, ticks: u32) {
    for i in 1..=ticks {
        std::thread::sleep(Duration::from_secs(1));
        let snapshot = collector.tick();
        // `tracked`, not the row count: the benchmarks log this, and the rows
        // stop at MAX_ROWS however many processes are running.
        println!("tick {i}/{ticks}: {} processes", snapshot.tracked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_pids_selects_by_value_not_by_order() {
        let cpu = Deltas::from([(1, 1.0), (2, 5.0), (3, 3.0), (4, 4.0), (5, 2.0)]);
        let top: HashSet<u32> = top_pids(&cpu, 3).into_iter().collect();
        assert_eq!(top, HashSet::from([2, 4, 3]));
    }

    #[test]
    fn candidates_union_top_cpu_and_top_io() {
        let cpu = Deltas::from([(1, 90.0), (2, 10.0), (3, 1.0)]);
        // pid 4 has high I/O but no CPU rank: it must still make the cut.
        let io = Deltas::from([(3, 80.0), (4, 70.0), (5, 1.0)]);
        let got: HashSet<u32> = select_candidates(&cpu, &io, 2).into_iter().collect();
        assert_eq!(got, HashSet::from([1, 2, 3, 4]));
    }
}
