# truetop - Architecture Constraints

This file is the canonical source of architectural truth for this project.
Every design decision below is **non-negotiable**. Do not deviate from these
constraints when generating or reviewing code.

> The constraints (the rules) are binding. The descriptions of how the code
> currently satisfies them - a function name, a map type, a figure - are not
> guaranteed current, only accurate as of the commit that last touched this
> file. This file has drifted from the code before. Before relying on or
> repeating a specific claim, check it against the source it names.

---

## 1. Core Stack

- **Kernel-space**: eBPF compiled to BPF bytecode
- **User-space**: Rust (2024 edition), on a multi-threaded `tokio` runtime
  (`#[tokio::main]`, default flavor)
- **Frameworks**: `aya` + `aya-ebpf`, `ratatui` + `crossterm`, `arc-swap`
- **Minimum kernel**: the code targets Linux ≥ 5.10 (BTF + raw tracepoints, and
  the pre-5.14 `state`-field path), but the tested and supported floor is 5.15 -
  the oldest kernel CI runs. 5.10-5.14 may work, untested.
- **CO-RE requirement**: `CONFIG_DEBUG_INFO_BTF=y`. Boot aborts gracefully if
  `/sys/kernel/btf/vmlinux` is missing.

---

## 2. eBPF Kernel-Space Pipeline

**Hooking strategy**: `raw_tracepoint` everywhere the kernel offers one, to
bypass argument allocation overhead.

**The one exception** is `commit_creds` (`creds`), attached with `fentry`. The
kernel exposes no tracepoint for credential changes, and without it a process
that drops privileges after `exec` - an nginx worker, an sshd session - reports
the uid it was born with for the rest of its life. The rule exists to protect a
path that fires hundreds of thousands of times a second; this one fires about
once per process lifetime, so the trampoline is not a hotpath cost. Any further
exception needs the same argument made explicitly.

**Targets**:
- `sched_switch` - CPU utilization and D-state I/O wait per PID (one program,
  fanned out per metric)
- `sched_process_exec` - process identity (`comm`) capture
- `sched_process_fork` - identity for children that never `exec` (they inherit
  the parent's `comm`), so fork-per-connection servers are named without `/proc`
- `sched_process_exit` - PID lifecycle cleanup
- `commit_creds` (fentry) - the uid after a process changes credentials
- `block_rq_issue` / `block_rq_complete` - block I/O device latency (Phase 2b)

Memory (RSS) is read from `/proc` in user space: since Linux 6.2 it lives in a
`percpu_counter` that eBPF cannot sum exactly (see README).

**Memory primitives**: `CPU_NS`, `IOWAIT_NS` and `COMM_MAP` are LRU
(`LruPerCpuHashMap`, `LruPerCpuHashMap`, `LruHashMap`), sized by
`--max-processes`. A plain hash map fails a write at capacity, and the hotpath
silently drops it - the process then reads 0% forever with no error. LRU evicts
the coldest entry instead, so a process arriving at a full map is accounted for
rather than lied about. `SLEEP_SINCE` stays a plain global `HashMap`: it is
keyed by tid, an entry lives only from a D-state switch-out to the matching
switch-in, and it is bounded by concurrently-sleeping threads, not by process
count - LRU would only add cost there for nothing.

**PID cleanup**: a process's final CPU and I/O-wait totals must be read before
their entries are dropped, and those totals live in per-CPU maps the exit hook
cannot sum. `sched_process_exit` announces a departure only when the thread
group is actually dead - `task::group_is_dead()` probe-reads
`signal->live == 0`, not `tid == tgid`: a leader that calls `pthread_exit` is a
zombie while its threads keep running, and reaping on `tid == tgid` would strip
a live process of its counters and its name. On a real departure, user space
reads the final totals on its next tick - charging a process that lived and
died between two ticks its full time - and only then deletes the entries,
bounded to `Reaper::BUDGET` (4096) per tick so an exit storm cannot serialise an
unbounded run of syscalls ahead of the snapshot publish. Reaping is the fast
path, reclaiming within about one tick; LRU eviction is the backstop if a ring
record is ever dropped or the budget is exceeded, so the maps never grow past
their configured capacity even when reaping falls behind.

**CO-RE enforcement**: direct pointer dereferencing is **prohibited**. Kernel
struct fields are read at BTF-resolved offsets injected as load-time constants
(`btf::field_byte_offset`, `EbpfLoader::override_global`), via
`bpf_probe_read_kernel` at that offset - never by dereferencing a raw pointer
into the struct.

**Execution constraint**: strictly O(1) per event. No loops, no aggregation,
no delta calculations in kernel space. Counters and timestamps only.

---

## 3. User-Space Concurrency (Snapshot-per-Tick + Arc-Swap)

One `tokio` runtime, three concurrent units, sharing state through
`Arc<ArcSwap<SystemState>>`:

- **Collector task** (`tokio::spawn`, `backend::collector_loop`): wakes on a
  1000 ms interval, calls the synchronous `Collector::tick()` (batched map
  reads, `/proc` reads for the visible rows, `bpf(2)` deletes for reaped
  exits), and publishes a fresh `SystemState` with `shared.store(Arc::new(_))`.
  `tick()` is **not** wrapped in `spawn_blocking`; it runs to completion on
  whichever worker thread picked up the task, which only works because
  `rt-multi-thread` has spare workers to schedule the rest of the runtime on
  meanwhile.
- **Signal task** (`tokio::spawn`): awaits `Ctrl-C` / SIGTERM and flips an
  `AtomicBool` the other two units poll.
- **Renderer** (`ui::render_app`): called directly, not spawned, from inside
  the async `run_ui`. It is a synchronous loop polling
  `crossterm::event::poll` on a 16 ms budget, so it blocks whichever worker
  thread is running it for the entire session - the same load-bearing
  assumption that `tick()`'s blocking I/O depends on.

`SystemState` is **not** a reused buffer. Each tick builds one from scratch -
a fresh `Vec<ProcessMetrics>`, cloned history `VecDeque`s, a new struct - and
publishes it whole. That allocation is deliberate: it is what makes
`ArcSwap::load()` a lock-free read of a complete, self-consistent snapshot: the
atomicity guarantee is on the pointer swap, not on avoiding allocation.
Formatting (numbers to human-readable strings) happens lazily in the render
phase, only for the rows actually drawn.

---

## 4. Data Pipeline & ABI Safety

- All structs shared between eBPF and user-space enforce `#[repr(C)]` for
  identical memory layouts across the FFI boundary.
- **Memory lock**: `setrlimit(RLIMIT_MEMLOCK, RLIM_INFINITY)` enforced at
  initialisation before Aya instantiation to accommodate BPF map allocations.
- **Teardown**: the signal task (§3) flips a flag so the renderer and
  collector stop; `run()` then drops the `aya::Ebpf`, releasing its owned
  `bpf_link`s. This is a convenience for a clean exit, not a correctness
  requirement: `bpf_link` file descriptors are refcounted by the kernel and
  released on close regardless of cause, including an uncatchable `SIGKILL`.
  There is no leaked-attachment failure mode to guard against.

---

## 5. I/O-Wait Attribution

Time blocked in uninterruptible (D-state) sleep is measured on the existing
`sched_switch` hook: stamp on a D switch-out, charge the tgid on the next
switch-in. The task that actually blocks is the one charged - a synchronous
read lands on the process that issued it, not on a kworker. Deferred writeback
is different, and honestly so: the writing task returns once its pages are
dirtied, and the flush later shows up under the flush kworker's own D-state,
because that is the task the kernel puts to sleep. Charging such writeback
back to the originating app needs cgroup or page-owner tracking and is not
implemented. No other `top`-like tool shows this metric per process at all.

`prev->state` is read from the ≥5.18 tracepoint argument where the kernel
provides it (`HAS_PREV_STATE`), else probe-read via an injected BTF offset
(`__state`; `state` before 5.14) - one code path covers every kernel ≥ 5.10.

**Not implemented**: refining the interval with `sched_wakeup` to exclude
post-wakeup runqueue delay (which also yields per-PID runqueue latency as a
side effect); block device latency per PID via `block_rq_issue` /
`block_rq_complete`, keyed by `(dev, sector)`, subject to the same
writeback-attribution caveat above.

---

## 6. Overhead Disclosure

`sched_switch` fires on every context switch. On busy systems this can reach
hundreds of thousands of events per second. The per-event cost is O(1) and
sub-microsecond, so total overhead scales with the context-switch rate, not the
process count - but it is **not zero**, and it tracks both the clocksource
(`bpf_ktime_get_ns` runs per event; `hpet` roughly triples it versus `tsc`) and
CPU turbo/mitigation state, which can move the figure with no code change at
all.

**The current number lives in one place**: [`bench/BENCHMARKS.md`](bench/BENCHMARKS.md),
not here. Any change to `sched_switch`, `iowait`, or anything else on the
per-event path must be re-measured with `cargo xtask bench hotpath` - baseline
and after, same session, same machine - before a figure is quoted anywhere,
`BENCHMARKS.md` included. Do not copy a number from there into this file: it
will drift the moment the hotpath changes again, and a stale number here is
exactly the failure this rule exists to prevent.

---

## Workspace Structure

```
truetop/            # user-space binary (ratatui UI + aya loader)
truetop-ebpf/       # eBPF programs compiled to BPF bytecode
truetop-common/     # #[repr(C)] structs shared across the FFI boundary
```

**What each file does is not repeated here.** Every module has its own doc
comment stating its purpose, and a crate-root index of one-liners pointing at
them: [`truetop-ebpf/src/main.rs`](truetop-ebpf/src/main.rs) for the kernel
side, [`truetop/src/lib.rs`](truetop/src/lib.rs) for user space. Read those
first when looking for where something lives; a copy here would drift the
first time either changes and this file did not.
