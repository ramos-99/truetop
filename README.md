<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/truetop-logo.png">
  <source media="(prefers-color-scheme: light)" srcset="assets/truetop-logo-light.png">
  <img alt="truetop" src="assets/truetop-logo-light.png" width="380">
</picture>

**Per-process Linux monitor built on eBPF.**

CPU, memory, and the one column `top` doesn't have: per-process I/O wait.

[![CI](https://github.com/ramos-99/truetop/actions/workflows/ci.yml/badge.svg)](https://github.com/ramos-99/truetop/actions/workflows/ci.yml)
[![kernel](https://img.shields.io/badge/Linux-%E2%89%A5%205.10-F76800?style=flat-square&logo=linux&logoColor=white)](#requirements)
[![collection](https://img.shields.io/badge/collection-eBPF%20CO--RE-F76800?style=flat-square)](#how-it-works)
[![Rust](https://img.shields.io/badge/Rust-2024-DEA584?style=flat-square&logo=rust&logoColor=white)](Cargo.toml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-2EA043?style=flat-square)](#license)

</div>

---

`top`, `htop`, and `btop` parse `/proc` — one directory per process, every refresh —
so their cost grows with the process count. truetop collects the same per-process
CPU **in the kernel** with O(1)-per-event eBPF, drains it in a single batched
syscall per refresh, and adds a metric none of them show: **how long each process
was blocked on I/O** — the thing you want when the box feels stuck but the CPU
looks idle.

## Contents

[Features](#features) · [Requirements](#requirements) · [Install](#install) · [Usage](#usage) · [How it works](#how-it-works) · [Benchmarks](#benchmarks) · [Overhead](#overhead) · [Memory](#memory) · [Roadmap](#roadmap) · [License](#license)

## Features

- **Per-process I/O wait** — time each process spent in uninterruptible (D-state)
  sleep, charged to the task that actually blocked. `top`/`htop`/`btop` don't show
  it per process; `iotop` gets a related number from `delayacct` over netlink.
  truetop reads it straight from the scheduler, in-kernel, alongside everything else.
- **O(1) collection** — CPU and I/O wait accrue on `sched_switch`; one
  `bpf_map_lookup_batch` per refresh drains it all, flat whether you have 50
  processes or 50,000.
- **CO-RE across kernels** — `task_struct` offsets are read from the kernel's own
  BTF at load (no libbpf), so one binary spans releases. CI runs the programs on a
  live-kernel matrix, **Arch and Fedora across 5.15 → 6.18**.
- **Honest about cost** — the `sched_switch` hook runs on every context switch, so
  overhead tracks the switch rate, not the process count. It is measured, not
  hand-waved: see [Overhead](#overhead) and [the benchmarks](bench/BENCHMARKS.md).

## Requirements

- Linux **≥ 5.10**, x86-64 (CI-verified on 5.15 → 6.18; aarch64 builds, runtime untested)
- Kernel built with `CONFIG_DEBUG_INFO_BTF=y` — `/sys/kernel/btf/vmlinux` present at
  runtime. Every mainstream distro ships this; truetop aborts with a clear message if
  it is missing.
- `CAP_BPF` + `CAP_PERFMON` (or root) to load the programs.

## Install

No published binary yet. Build from source — the build script compiles the eBPF
object, so it needs the nightly toolchain and `bpf-linker`:

```sh
rustup toolchain install stable
rustup toolchain install nightly --component rust-src
cargo install bpf-linker

git clone https://github.com/ramos-99/truetop
cd truetop
cargo build --release
```

The binary lands at `target/release/truetop`. Prebuilt releases and an AUR package
are on the [roadmap](#roadmap).

## Usage

Loading eBPF needs privileges, so run as root (or grant the binary `CAP_BPF` and
`CAP_PERFMON`):

```sh
sudo ./target/release/truetop
cargo xtask run                 # build + run, elevating with sudo
sudo ./target/release/truetop --bench 5   # headless: 5 collector ticks, no UI
```

| key            | action                 |
| -------------- | ---------------------- |
| `q` / `Esc`    | quit                   |
| `↑` `↓` / `k` `j` | move selection      |
| `Home` / `End` | first / last row       |
| `c`            | sort by CPU            |
| `i`            | sort by I/O wait       |

Columns: `Pid`, `User`, `Program`, `Cpu%`, `Mem`, `IO Wait`.

## How it works

Three raw tracepoints, nothing on the hotpath from `/proc`:

- `sched_switch` — per-process on-CPU time and I/O wait (stamp the timestamp on a
  D-state switch-out, charge the interval to the task on its next switch-in).
- `sched_process_exec` — process identity (`comm`).
- `sched_process_exit` — deletes the PID's map entries, so nothing stale accumulates.

`task_struct` field offsets come from the kernel's own BTF, injected as load-time
constants (hand-rolled CO-RE — aya emits no relocations), which is what lets one
binary run across kernels. Userspace is two threads sharing an
[`arc-swap`](https://docs.rs/arc-swap): a **collector** wakes every second, drains
every per-CPU map in one `bpf_map_lookup_batch`, computes deltas against the last
tick, and publishes an immutable snapshot; the **ratatui** renderer loads that
snapshot lock-free and formats only the visible rows. Kernel work is strictly O(1)
per event — counters and timestamps only; all aggregation happens in userspace.

## Benchmarks

Per-process CPU is collected in-kernel and pulled in one batched syscall, instead
of parsing `/proc/<pid>/stat` once per process — O(1) syscalls per refresh instead
of O(N). Reproduce everything with `cargo xtask bench`.

| metric                                | truetop                    | htop / btop           |
| ------------------------------------- | -------------------------- | --------------------- |
| CPU at ~9,000 processes               | **1.7%** of a core, flat   | 33% / 22%, climbing   |
| data syscalls per refresh (~5,000)    | **~780**                   | ~12,000 (htop)        |
| per-process collection work           | **~30 ns**                 | ~12 µs (procfs)       |

[![Read the benchmarks](https://img.shields.io/badge/read_the_benchmarks-method_%C2%B7_results_%C2%B7_kernel_cost-8250DF?style=for-the-badge)](bench/BENCHMARKS.md)

## Overhead

`sched_switch` fires on every context switch, so truetop's cost scales with the
context-switch rate, not the process count. The per-event program is O(1) —
median ~335 ns on the reference machine — but not zero: under `hackbench` (a
context-switch storm) it adds single-digit percent wall-clock, while a normal
system does orders of magnitude fewer switches and pays well under 1%. Concretely,
truetop is cheaper than htop only while switches/s stay below ~80,000 + 130 ×
processes; a switch-heavy box with few processes costs more, not less. The cost
also tracks the clocksource (`bpf_ktime_get_ns` runs per event; on `hpet` it roughly
triples). Benchmark on latency-sensitive hosts before deploying — the numbers and
method are in [bench/BENCHMARKS.md](bench/BENCHMARKS.md).

## Memory

RSS is read from `/proc/<pid>/statm` in userspace, for the visible rows only, so
the cost is bounded regardless of process count. This is a fallback, not a design
choice: since Linux 6.2 a process's RSS lives in a `percpu_counter`, and no eBPF
interface exposes the accurate summed value — the global count alone drifts from
`top` by megabytes on busy multi-threaded processes. When the kernel provides a
usable interface (a helper or a tracepoint carrying the summed value), memory will
move in-kernel like the rest. Until then it is the one metric not collected via eBPF.

## Roadmap

- Block-device latency per PID via `block_rq_issue` / `block_rq_complete`.
- `bpf_iter` task walk to drop the single `/proc` backfill of pre-existing process
  names at startup, making truetop fully `/proc`-independent.
- Memory in-kernel once the kernel exposes an accurate per-process RSS interface.
- Widen the CI floor to the 5.10–5.13 `state`-field path, and aarch64 at runtime.

## License

User-space: **MIT OR Apache-2.0**. eBPF code (`truetop-ebpf/`): **GPL-2.0 OR MIT**.
