# truetop — Architecture Constraints

This file is the canonical source of architectural truth for this project.
Every design decision below is **non-negotiable**. Do not deviate from these
constraints when generating or reviewing code.

---

## 1. Core Stack

- **Kernel-space**: eBPF compiled to BPF bytecode
- **User-space**: Rust (2024 edition)
- **Frameworks**: `aya` + `aya-bpf`, `ratatui` + `crossterm`, `arc-swap`
- **Minimum kernel**: Linux ≥ 5.10 (hard requirement for `rss_stat`)
- **CO-RE requirement**: `CONFIG_DEBUG_INFO_BTF=y`. Boot aborts gracefully if
  `/sys/kernel/btf/vmlinux` is missing.

---

## 2. eBPF Kernel-Space Pipeline

**Hooking strategy**: exclusive use of `raw_tracepoint` to bypass argument
allocation overhead.

**Targets**:
- `sched_switch` — CPU utilization per PID
- `rss_stat` — memory RSS per PID
- `sched_process_exit` — PID lifecycle cleanup
- `block_rq_issue` / `block_rq_complete` — block I/O latency

**Memory primitives**: `PerCpuHashMap` and `PerCpuArray` exclusively. Global
hash maps are **prohibited** to eliminate spinlock contention on hotpaths.

**PID cleanup**: `sched_process_exit` hook calls `bpf_map_delete_elem()`
immediately on process termination. No stale entries accumulate.

**CO-RE enforcement**: direct pointer dereferencing is **prohibited**. All
kernel struct accesses use `bpf_core_read!` macros for cross-kernel ABI
stability.

**Execution constraint**: strictly O(1) per event. No loops, no aggregation,
no delta calculations in kernel space. Counters and timestamps only.

---

## 3. User-Space Concurrency (Double-Buffer + Arc-Swap)

Dual-thread model with atomic pointer swap for zero-lock reads.

- **Shared state**: `ArcSwap<SystemState>` where `SystemState` is a
  pre-allocated, reusable staging buffer.
- **Collector thread (backend)**:
  - Wakes on 1000 ms interval.
  - Mutates the pre-allocated staging buffer in-place — **no allocation per
    tick**.
  - Calls `bpf_map_lookup_batch` to pull all per-CPU data in a single syscall.
  - Computes deltas (current vs previous tick) and aggregates per-CPU values
    in user-space.
  - Executes `ArcSwap::store()` — atomic pointer swap, nanosecond lock
    duration. Drops previous snapshot.
- **Renderer thread (frontend)**:
  - Event-driven via `crossterm::event::poll`.
  - Calls `ArcSwap::load()` for an atomic read of the current immutable
    snapshot.
  - Feeds data references directly into ratatui draw routines.
  - Formatting (integers to human-readable strings) occurs lazily within the
    draw phase, only for visible data.
  - Guarantees sub-16 ms UI responsiveness regardless of PID count or backend
    batching latency.

---

## 4. Data Pipeline & ABI Safety

- All structs shared between eBPF and user-space enforce `#[repr(C)]` for
  identical memory layouts across the FFI boundary.
- **Memory lock**: `setrlimit(RLIMIT_MEMLOCK, RLIM_INFINITY)` enforced at
  initialisation before Aya instantiation to accommodate BPF map allocations.
- **Teardown**: signal handler intercepts `SIGINT`/`SIGTERM` to gracefully
  detach all `bpf_link` descriptors. Unhandled teardown leaks tracepoint
  attachments until reboot.

---

## 5. Implementation Phases (v0.1.0)

**Phase 1 — procfs parity baseline**:
- Implement CPU utilization via `sched_switch` and memory tracking via
  `rss_stat`.
- Validate that eBPF O(1) per-event cost is lower than btop's O(N) procfs
  text parsing under high PID load.
- Public release only after parity is confirmed under stress.

**Phase 2 — killer feature (block I/O latency per PID)**:
- Hook `block_rq_issue` and `block_rq_complete`.
- Correlation uses an intermediate BPF map keyed by `(dev, sector)` — stored
  on issue, looked up and deleted on complete — to compute per-request latency.
- User-space renders real-time latency histograms per PID.
- This metric is structurally impossible to extract from `/proc/diskstats`,
  which only provides aggregate throughput.

---

## 6. Overhead Disclosure

`sched_switch` fires on every context switch. On busy systems this can reach
hundreds of thousands of events per second. The per-event cost is
nanosecond-level O(1) kernel execution, which is lower than procfs text
parsing at scale, but is **not zero**. The README must document this
trade-off explicitly to avoid claims that will be challenged and disproven.

---

## Workspace Structure

```
truetop/            # user-space binary (ratatui UI + aya loader)
truetop-ebpf/       # eBPF programs compiled to BPF bytecode
truetop-common/     # #[repr(C)] structs shared across the FFI boundary
```
