//! Collector (backend) — producer side of the double-buffer (CLAUDE.md §3). It
//! orchestrates the per-metric collectors in `metrics`: one batched map read
//! drives CPU% (which establishes the row set), then each visible row is
//! enriched with name and RSS. Publishes an immutable snapshot via an atomic
//! pointer swap.

use std::{
    os::fd::{AsFd, AsRawFd},
    sync::Arc,
    time::Duration,
};

use arc_swap::ArcSwap;
use aya::maps::{HashMap as BpfHashMap, MapData};
use tokio::time::{MissedTickBehavior, interval};
use truetop_common::COMM_LEN;

use crate::{
    batch::BatchReader,
    metrics::{CpuCollector, MemReader, ProcessMetrics, Resolver, Snapshot},
};

#[derive(Debug, Clone, Default)]
pub struct SystemState {
    pub processes: Vec<ProcessMetrics>,
}

/// Owns the per-metric collectors; each [`Collector::tick`] reads the maps and
/// produces the next snapshot.
pub struct Collector {
    cpu_ns: MapData,
    reader: BatchReader,
    cpu: CpuCollector,
    names: Resolver,
    mem: MemReader,
}

impl Collector {
    pub fn new(
        cpu_ns: MapData,
        comm: BpfHashMap<MapData, u32, [u8; COMM_LEN]>,
        ncpus: usize,
    ) -> Self {
        let ncpus = ncpus.max(1);
        Self {
            cpu_ns,
            reader: BatchReader::new(ncpus),
            cpu: CpuCollector::new(ncpus as f64),
            names: Resolver::new(comm),
            mem: MemReader::new(),
        }
    }

    pub fn tick(&mut self) -> SystemState {
        let fd = self.cpu_ns.fd().as_fd().as_raw_fd();
        let cpu_snapshot = Snapshot::new(self.reader.sum_per_cpu(fd));

        let mut processes = self.cpu.collect(cpu_snapshot);
        for p in &mut processes {
            p.name = self.names.resolve(p.pid);
            p.mem = self.mem.for_pid(p.pid);
        }
        SystemState { processes }
    }
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
        println!("tick {i}/{ticks}: {} processes", snapshot.processes.len());
    }
}
