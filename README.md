<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/truetop-logo.png">
  <source media="(prefers-color-scheme: light)" srcset="assets/truetop-logo-light.png">
  <img alt="truetop" src="assets/truetop-logo-light.png" width="380">
</picture>

Per-process Linux monitor built on eBPF.

[![CI](https://github.com/ramos-99/truetop/actions/workflows/ci.yml/badge.svg)](https://github.com/ramos-99/truetop/actions/workflows/ci.yml)
[![kernel](https://img.shields.io/badge/Linux-%E2%89%A5%205.15-F76800?style=flat-square&logo=linux&logoColor=white)](#requirements)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-2EA043?style=flat-square)](#license)

<img src="assets/demo.gif" alt="truetop sorting by I/O wait: fio readers blocked on the disk light up red" width="820">

</div>

---

truetop shows per-process CPU, memory, and one column `top`, `htop` and `btop`
do not have: how long each process sat blocked on storage. Everything except
memory is collected inside the kernel by O(1)-per-event eBPF programs and
drained in a single batched syscall per refresh, so collection cost does not
grow with the process count.

## Contents

[Features](#features) · [Requirements](#requirements) · [Install](#install) · [Usage](#usage) · [How it works](#how-it-works) · [Benchmarks](#benchmarks) · [Overhead](#overhead) · [Memory](#memory) · [Roadmap](#roadmap) · [Security](#security) · [License](#license)

## Features

- **Per-process I/O wait.** The time each process spent in uninterruptible
  (D-state) sleep, charged to the task that actually blocked. `top`, `htop` and
  `btop` do not show it per process; `iotop` derives a related number from
  `delayacct` over netlink. truetop reads it from the scheduler, in-kernel, next
  to everything else.

  It is a diagnostic rather than a dashboard: on an idle machine the column
  stays empty. A kernel worker high in it is deferred writeback being flushed;
  one of your own processes high in it is a synchronous stall, which is the case
  the other monitors leave you to guess at.

- **O(1) collection.** CPU and I/O wait accumulate on `sched_switch`, and one
  `bpf_map_lookup_batch` per refresh drains the lot, whether the machine is
  running 50 processes or 50,000.

- **One binary across kernels.** `task_struct` offsets are resolved from the
  kernel's own BTF at load and injected as constants, without libbpf. CI runs
  the programs on live kernels 5.15 through 6.18, on Arch and Fedora configs.

- **Measured overhead.** The `sched_switch` hook runs on every context switch,
  so the cost tracks the switch rate rather than the process count. The figures,
  and the crossover past which htop is cheaper, are in [Overhead](#overhead).

## Requirements

- Linux 5.15 or newer on x86-64, the range CI runs the programs on (5.15 through
  6.18). The pre-5.14 `state`-field path is still in the code, so 5.10 through
  5.14 may work, but as of today that is neither tested nor guaranteed. aarch64
  compiles but is not tested at runtime.
- A kernel built with `CONFIG_DEBUG_INFO_BTF=y`, so `/sys/kernel/btf/vmlinux`
  exists at runtime. Every mainstream distro ships this, and truetop aborts with
  a message naming the option if it is missing.
- `CAP_BPF` and `CAP_PERFMON`, or root, to load the programs.

## Install

There is no published binary yet. The build compiles the eBPF object, so it
needs the nightly toolchain and `bpf-linker`:

```sh
rustup toolchain install stable
rustup toolchain install nightly --component rust-src
cargo install bpf-linker
```

Then either build the repository:

```sh
git clone https://github.com/ramos-99/truetop
cd truetop
cargo build --release
```

or install straight from git:

```sh
cargo install --git https://github.com/ramos-99/truetop truetop
```

Prebuilt releases and an AUR package are on the [roadmap](#roadmap).

## Usage

Loading eBPF needs privileges, so run as root or grant the binary the two
capabilities:

```sh
sudo ./target/release/truetop
sudo setcap cap_bpf,cap_perfmon+ep ./target/release/truetop   # then run it unprivileged
cargo xtask run                                               # build and run, elevating with sudo
```

| key | action |
| --- | --- |
| `q`, `Esc` | quit |
| `↑` `↓`, `k` `j` | move the selection |
| `PgUp`, `PgDn` | move a page |
| `Home`, `End` | first, last row |
| `c`, `m`, `i` | sort by CPU, memory, I/O wait; press again to reverse |
| `/` | filter by program name or pid |
| `space` | pause |

Columns are `Pid`, `User`, `Program`, `Cpu%`, `Mem` and `IO Wait`. The table
holds the top 256 rows of the current sort, so drawing does not grow with the
process count either.

`truetop --bench <TICKS>` runs the collector headless for a fixed number of
ticks, which is what the benchmarks drive. `--help` and `--version` answer
without privileges.

## How it works

Four raw tracepoints, and nothing reads `/proc` on the hot path:

- `sched_switch` accumulates per-process on-CPU time, and I/O wait by stamping a
  timestamp when a task leaves the CPU in uninterruptible sleep and charging the
  interval when it comes back.
- `sched_process_exec` and `sched_process_fork` capture process names. Recording
  the fork is what names children that never call `execve`, such as PostgreSQL
  backends and nginx workers.
- `sched_process_exit` announces the departure on a ring buffer. User space
  reads the process's final totals on its next refresh, so one that ran and ended
  between two refreshes is still charged its time, then deletes the entry.

A process that both starts and ends within a single refresh is the exception:
with no earlier sample to subtract from, that interval's time is not attributed
to it.

`task_struct` field offsets are read from the kernel's own BTF at load time and
injected as constants, which is what lets one binary span kernel versions
without libbpf.

User space runs two threads over an [`arc-swap`](https://docs.rs/arc-swap). A
collector wakes every second, drains every per-CPU map in one
`bpf_map_lookup_batch`, computes deltas against the previous tick, and publishes
an immutable snapshot. The [ratatui](https://ratatui.rs) renderer loads that
snapshot without locking and formats only the rows it draws. Kernel-side work is
O(1) per event and stores counters and timestamps only; all aggregation happens
in user space.

## Benchmarks

Reproduce these with `cargo xtask bench`.

| metric | truetop | htop / btop |
| --- | --- | --- |
| CPU at ~9,000 processes | 1.7% of a core, flat | 33% / 22%, climbing |
| data syscalls per refresh at ~5,000 | ~780 | ~12,000 (htop) |
| per-process collection work | ~30 ns | ~12 µs (procfs) |

Method, full results and the kernel-side cost are in
[bench/BENCHMARKS.md](bench/BENCHMARKS.md).

## Overhead

`sched_switch` fires on every context switch, so truetop's cost scales with the
context-switch rate rather than the process count. The per-event program is O(1)
and measured at a median of ~335 ns on the reference machine, which is small but
not free: under a `hackbench` context-switch storm it costs single-digit percent
of wall clock, while an ordinary system does orders of magnitude fewer switches
and pays well under 1%.

Concretely, truetop is cheaper than htop only while switches per second stay
below roughly 80,000 + 130 × processes. A switch-heavy machine with few
processes costs more, not less. The figure also tracks the clocksource, since
`bpf_ktime_get_ns` runs per event and roughly triples on `hpet`. Benchmark
before deploying on latency-sensitive hosts.

## Memory

RSS comes from `/proc/<pid>/statm` in user space, read only for the rows on
screen, so the cost stays bounded regardless of process count.

This is a fallback rather than a preference. Since Linux 6.2 a process's RSS
lives in a `percpu_counter`, and no eBPF interface exposes the summed value; the
global count on its own drifts from `top` by megabytes on busy multi-threaded
processes. If the kernel grows a helper or a tracepoint carrying the total,
memory moves in-kernel with everything else.

## Roadmap

- Per-PID block device latency from `block_rq_issue` and `block_rq_complete`.
- A `bpf_iter` task walk to replace the one-time `/proc` sweep that names
  processes which already existed at startup.
- Memory in-kernel, once an accurate per-process interface exists.
- Kernels 5.10 through 5.13 in CI, which take the pre-5.14 `state` field path,
  and aarch64 at runtime.

## Security

> [!IMPORTANT]
> truetop loads eBPF with `CAP_BPF` and `CAP_PERFMON`, or root. Report
> vulnerabilities privately through the [security policy](SECURITY.md), not a
> public issue.

## License

User space is MIT or Apache-2.0. The eBPF code in `truetop-ebpf/` is GPL-2.0 or
MIT, matching the licence it declares to the kernel verifier.
