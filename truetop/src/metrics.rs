//! Per-process render aggregates, split by subsystem so each phase plugs in
//! cleanly. Derived values (percentages, units) live here, not in the FFI layer.

/// CPU share over the last interval, normalised to whole-system capacity
/// (100.0 = every logical core saturated).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuMetrics {
    pub cpu_percent: f64,
}

/// RSS memory. Phase 2 (`rss_stat`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[allow(dead_code)]
pub struct MemMetrics {
    pub rss_bytes: u64,
}

/// Per-PID block-I/O latency. Phase 2 (`block_rq_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[allow(dead_code)]
pub struct IoMetrics {
    pub mean_latency_ns: u64,
}

/// One process as the renderer sees it: identity plus one slot per subsystem.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessMetrics {
    pub pid: u32,
    pub cpu: CpuMetrics,
    pub mem: Option<MemMetrics>,
    pub io: Option<IoMetrics>,
}
