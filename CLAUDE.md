# truetop - Architecture Constraints

This file is the canonical source of architectural truth for this project.
Every design decision below is **non-negotiable**. Do not deviate from these
constraints when generating or reviewing code.

---

## 1. Core Stack

- **Kernel-space**: eBPF compiled to BPF bytecode
- **User-space**: Rust (2024 edition)
- **Frameworks**: `aya` + `aya-bpf`, `ratatui` + `crossterm`, `arc-swap`
- **Minimum kernel**: Linux ≥ 5.10 (BTF + raw tracepoints)
- **CO-RE requirement**: `CONFIG_DEBUG_INFO_BTF=y`. Boot aborts gracefully if
  `/sys/kernel/btf/vmlinux` is missing.

---

## 2. eBPF Kernel-Space Pipeline

**Hooking strategy**: exclusive use of `raw_tracepoint` to bypass argument
allocation overhead.

**Targets**:
- `sched_switch` - CPU utilization and D-state I/O wait per PID (one program,
  fanned out per metric)
- `sched_process_exec` - process identity (`comm`) capture
- `sched_process_fork` - identity for children that never `exec` (they inherit
  the parent's `comm`), so fork-per-connection servers are named without `/proc`
- `sched_process_exit` - PID lifecycle cleanup
- `block_rq_issue` / `block_rq_complete` - block I/O device latency (Phase 2b)

Memory (RSS) is read from `/proc` in user space: since Linux 6.2 it lives in a
`percpu_counter` that eBPF cannot sum exactly (see README).

**Memory primitives**: `PerCpuHashMap` and `PerCpuArray` for hotpath
accumulators. Global hash maps are allowed only where per-CPU storage cannot
express the data, with the rationale documented in the module: `COMM_MAP`
(cold exec-path writes) and `SLEEP_SINCE` (a D-sleep interval spans CPUs;
lock-free RCU lookups on the hotpath, locked updates only on D transitions).

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
  - Mutates the pre-allocated staging buffer in-place - **no allocation per
    tick**.
  - Calls `bpf_map_lookup_batch` to pull all per-CPU data in a single syscall.
  - Computes deltas (current vs previous tick) and aggregates per-CPU values
    in user-space.
  - Executes `ArcSwap::store()` - atomic pointer swap, nanosecond lock
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

**Phase 1 - procfs parity baseline**:
- Implement CPU utilization via `sched_switch` and memory tracking via
  `rss_stat`.
- Validate that eBPF O(1) per-event cost is lower than btop's O(N) procfs
  text parsing under high PID load.
- Public release only after parity is confirmed under stress.

**Phase 2 - killer feature (per-PID I/O wait)**:
- Primary metric: time blocked in uninterruptible (D-state) sleep, measured on
  the existing `sched_switch` hook - stamp on a D switch-out, charge the tgid
  on the next switch-in. The task that actually blocks is the one charged: a
  synchronous read lands on the process that issued it, not on a kworker.
  Deferred writeback is different, and honestly so: the writing task returns
  once its pages are dirtied, and the flush later shows up under the flush
  kworker's own D-state, because that is the task the kernel blocks. Charging
  such writeback back to the originating app needs cgroup or page-owner
  tracking and is future work. No top-like tool shows this per process.
- `prev->state` is probe-read via an injected BTF offset (`__state`; `state`
  before 5.14), one code path for every kernel ≥ 5.10. The ≥ 5.18 `prev_state`
  tracepoint argument is a later optimisation.
- The interval includes post-wakeup runqueue delay; refining with
  `sched_wakeup` (which also yields per-PID runqueue latency) is planned.
- **Phase 2b (optional detail layer)**: block device latency per PID via
  `block_rq_issue`/`block_rq_complete` keyed by `(dev, sector)`, with the
  writeback-attribution caveat documented. Per-PID cache-hit ratio stays out
  of the critical path.

---

## 6. Overhead Disclosure

`sched_switch` fires on every context switch. On busy systems this can reach
hundreds of thousands of events per second. The per-event cost is O(1) and
sub-microsecond - measured at a median ~335 ns under a `hackbench` context-switch
storm (hotpath benchmark, turbo off, `tsc` clocksource), not assumed - so total
overhead scales with the context-switch rate, not the process count: single-digit
percent under that storm, well under 1% in normal use, but **not zero**. The cost
also tracks the clocksource, since `bpf_ktime_get_ns` runs per event: on `hpet` it
roughly triples. The README must document these trade-offs explicitly to avoid
claims that will be challenged and disproven.

The I/O-wait extension reads `prev`'s state (the tracepoint arg from 5.18, else a
probe-read) and does one lock-free map lookup per event; perf prices that lookup at
a fraction of a percent of storm cycles. Any change to the hotpath must be
re-measured with the hotpath benchmark before the numbers above are quoted.

---

## Workspace Structure

```
truetop/            # user-space binary (ratatui UI + aya loader)
truetop-ebpf/       # eBPF programs compiled to BPF bytecode
truetop-common/     # #[repr(C)] structs shared across the FFI boundary
```
