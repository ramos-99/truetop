//! Collection-cost models for the `scaling` benchmark — see BENCHMARKS.md.

/// Per-CPU on-CPU nanosecond slots for one pid, as a batched map read yields it.
pub type PerCpuSample = Vec<u64>;

/// eBPF path: sum the per-CPU slots already pulled in one batched read — zero
/// per-process syscalls.
pub fn collect_batched(samples: &[(u32, PerCpuSample)]) -> u64 {
    samples
        .iter()
        .map(|(_pid, per_cpu)| per_cpu.iter().sum::<u64>())
        .sum()
}

/// procfs path: read and parse one `/proc/<pid>/stat` per process — O(N) syscalls.
pub fn collect_procfs(count: usize) -> u64 {
    (0..count).map(|_| read_self_cpu_ns()).sum()
}

fn read_self_cpu_ns() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // `comm` may hold spaces/parens; after the final ')', utime and stime are
    // the 12th and 13th whitespace fields.
    let mut fields = stat
        .rsplit(')')
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let utime: u64 = fields.nth(11).and_then(|f| f.parse().ok()).unwrap_or(0);
    let stime: u64 = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
    utime + stime
}
