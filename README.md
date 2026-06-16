# truetop

A Linux system monitor built on eBPF + Rust. Replaces procfs polling with
kernel-space tracepoint hooks, delivering per-PID CPU, memory, and block I/O
latency metrics at O(1) per-event cost.

---

## Why

`/proc` is a text interface designed for human inspection, not machine
consumption at scale. Under high PID load, tools like `top` and `btop` pay
O(N) parsing cost every refresh cycle. truetop moves instrumentation into the
kernel via eBPF tracepoints — each event is handled in nanoseconds with no
per-event allocation — and batches map reads into user-space with a single
`bpf_map_lookup_batch` syscall per tick.

The second motivation is metric availability. `/proc/diskstats` provides
aggregate block device throughput only. Per-PID block I/O latency — actual
request latency histograms per process — is structurally impossible to derive
from procfs. truetop computes it by correlating `block_rq_issue` and
`block_rq_complete` tracepoints in kernel space.

---

## Requirements

### Kernel

- Linux **≥ 5.10** (hard minimum; `rss_stat` tracepoint required)
- `CONFIG_DEBUG_INFO_BTF=y` compiled into the running kernel
- `/sys/kernel/btf/vmlinux` must be present at runtime; the program aborts
  with a diagnostic message if it is missing

### Toolchain

```
rustup toolchain install stable
rustup toolchain install nightly --component rust-src
cargo install bpf-linker
```

Cross-compilation from macOS requires LLVM:

```
brew install llvm
cargo install bpf-linker --no-default-features
```

---

## Build

eBPF programs must be compiled before the user-space binary links against
them. The workspace `build.rs` handles this automatically via `cargo build`,
but the eBPF step can also be invoked explicitly:

```sh
# Build eBPF programs (nightly, BPF target)
cargo xtask build-ebpf

# Build the full workspace (debug)
cargo build

# Build release
cargo build --release

# Run directly
cargo run --release
```

### Cross-compiling from macOS

```sh
cargo build --package truetop --release \
  --target=${ARCH}-unknown-linux-musl \
  --config=target.${ARCH}-unknown-linux-musl.linker=\"rust-lld\"
```

Copy the resulting `target/${ARCH}-unknown-linux-musl/release/truetop` to a
Linux host and run it there.

---

## Architecture

### eBPF kernel-space pipeline

All kernel instrumentation uses `raw_tracepoint` exclusively — not kprobes,
not fentry — to avoid the argument struct allocation that kprobes impose.

| Tracepoint | Purpose |
|---|---|
| `sched_switch` | CPU time accounting per PID |
| `rss_stat` | Memory RSS tracking per PID |
| `sched_process_exit` | Immediate map cleanup on process exit |
| `block_rq_issue` | Record block request timestamp |
| `block_rq_complete` | Compute and emit per-request I/O latency |

Map types are `PerCpuHashMap` and `PerCpuArray` only. Global (shared) hash
maps are prohibited — they introduce spinlock contention on context-switch
hotpaths. All per-CPU values are aggregated in user-space after batched
extraction.

Each eBPF handler is strictly O(1): no loops, no delta calculations, no
string formatting. Kernel space stores raw counters and timestamps only.

All kernel struct field accesses use `bpf_core_read!` macros. Direct pointer
dereferencing is prohibited. This provides CO-RE (Compile Once, Run
Everywhere) compatibility across kernel versions without recompilation.

### User-space concurrency (double-buffer + arc-swap)

Two threads, no locks on the read path:

**Collector thread** (1000 ms interval):
1. Calls `bpf_map_lookup_batch` — one syscall drains all per-CPU map entries.
2. Computes deltas against the previous tick; aggregates per-CPU values.
3. Writes results into a pre-allocated `SystemState` staging buffer (no heap
   allocation per tick).
4. Calls `ArcSwap::store()` — nanosecond atomic pointer swap. Previous
   snapshot is dropped.

**Renderer thread** (event-driven, `crossterm::event::poll`):
1. Calls `ArcSwap::load()` — atomic snapshot read, no lock.
2. Passes immutable data references directly to ratatui draw routines.
3. Number formatting occurs lazily in the draw phase, only for rows visible
   in the current terminal frame.

UI responsiveness is bounded by the renderer event loop, not by collector
batch duration. Sub-16 ms frame time is maintained regardless of PID count.

### ABI safety

All structs crossing the eBPF↔user-space boundary are defined in
`truetop-common` and annotated `#[repr(C)]`. The user-space binary and the
eBPF object share this crate, ensuring layout identity across the FFI
boundary.

`setrlimit(RLIMIT_MEMLOCK, RLIM_INFINITY)` is called before Aya initialises
BPF maps. Without this, large map allocations fail silently on default
systems.

### Teardown

A signal handler intercepts `SIGINT` and `SIGTERM` and calls detach on all
`bpf_link` file descriptors before exit. Without explicit detach, tracepoint
attachments persist until reboot.

---

## Overhead trade-off

`sched_switch` fires on every CPU context switch. On a busy system with many
threads this can reach hundreds of thousands of events per second.

The per-event eBPF handler cost is O(1) and measured in nanoseconds — lower
than the O(N) procfs text parsing that conventional tools perform every
refresh. However, it is not zero. On systems with pathologically high
context-switch rates, the aggregate eBPF overhead is measurable.

If you are running truetop on a latency-sensitive production host, benchmark
first with `perf stat` to confirm that context-switch rates are within
acceptable bounds for your workload.

---

## Implementation status

| Feature | Status |
|---|---|
| CPU utilization via `sched_switch` | Phase 1 |
| Memory RSS via `rss_stat` | Phase 1 |
| Block I/O latency histograms per PID | Phase 2 |

Phase 1 is released only after benchmarked parity with btop under high PID
load is confirmed.

---

## License

With the exception of eBPF code, truetop is distributed under the terms of
either the [MIT license](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE),
at your option.

All eBPF code (code in `truetop-ebpf/`) is distributed under either the
[GNU General Public License, Version 2](LICENSE-GPL2) or the [MIT license](LICENSE-MIT),
at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this project by you shall be dual-licensed as
above, without any additional terms or conditions.
