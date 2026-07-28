//! Integration: what happens when the accounting maps run out of room. LRU maps
//! evict their coldest entry rather than refusing the write, so a busy process
//! is never locked out of a full map the way a plain hash locked it out for
//! life. Run: `cargo xtask test`.

mod common;

use std::{collections::HashSet, thread::sleep, time::Duration};

use anyhow::Result;
use aya::maps::MapType;
use common::{ChildGuard, spawn};
use truetop::attach_with_capacity;

/// Far below any machine's working set, so the map is full within a tick.
const CAPACITY: u32 = 16;
const BUSY: usize = 32;
const SAMPLES: usize = 5;
const INTERVAL: Duration = Duration::from_millis(200);

#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn the_counter_map_is_lru_and_sized_by_the_flag() -> Result<()> {
    let (_ebpf, collector) = attach_with_capacity(CAPACITY)?;

    assert_eq!(
        collector.cpu_map_kind_and_capacity()?,
        (MapType::LruPerCpuHash, CAPACITY)
    );
    Ok(())
}

/// The map is filled by the machine's own processes first, so every process this
/// test starts arrives at a map with no room. Under a plain hash none of them
/// could ever be recorded; under LRU they displace whatever is coldest.
#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn a_full_map_admits_busy_newcomers() -> Result<()> {
    let (_ebpf, mut collector) = attach_with_capacity(CAPACITY)?;
    sleep(INTERVAL);
    collector.tick();

    let busy: Vec<ChildGuard> = (0..BUSY)
        .map(|_| spawn("sh", &["-c", "while :; do :; done"]))
        .collect::<Result<_>>()?;
    let ours: HashSet<u32> = busy.iter().map(|child| child.0.id()).collect();

    let mut seen = HashSet::new();
    for _ in 0..SAMPLES {
        sleep(INTERVAL);
        seen.extend(collector.tick().processes.iter().map(|p| p.pid));
    }

    let admitted = ours.intersection(&seen).count();
    assert!(
        admitted >= BUSY / 2,
        "{admitted} of {BUSY} busy processes reached a full {CAPACITY}-entry map"
    );
    Ok(())
}
