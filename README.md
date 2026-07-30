<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/truetop-logo.png">
  <source media="(prefers-color-scheme: light)" srcset="assets/truetop-logo-light.png">
  <img alt="truetop" src="assets/truetop-logo-light.png" width="380">
</picture>

**Per-process Linux monitor built on eBPF.**

_The I/O-wait column `top`, `htop` and `btop` don't have._

[![CI](https://img.shields.io/github/actions/workflow/status/ramos-99/truetop/ci.yml?style=for-the-badge&logo=github&label=CI)](https://github.com/ramos-99/truetop/actions/workflows/ci.yml)
[![kernel matrix](https://img.shields.io/github/actions/workflow/status/ramos-99/truetop/vm-matrix.yml?style=for-the-badge&logo=linux&logoColor=white&label=kernels%205.15%E2%80%936.18)](https://github.com/ramos-99/truetop/actions/workflows/vm-matrix.yml)
[![eBPF CO-RE](https://img.shields.io/badge/eBPF-CO--RE-F76800?style=for-the-badge&logoColor=white)](#how-it-works)
[![Rust](https://img.shields.io/badge/Rust-2024-DEA584?style=for-the-badge&logo=rust&logoColor=white)](Cargo.toml)
[![License](https://img.shields.io/badge/MIT%20or%20Apache--2.0-2EA043?style=for-the-badge)](#license)

[![Contributing](https://img.shields.io/badge/Contributing-guide-2B3137?style=for-the-badge&logo=git&logoColor=white)](CONTRIBUTING.md)
[![Security policy](https://img.shields.io/badge/Security-policy-2B3137?style=for-the-badge&logo=letsencrypt&logoColor=white)](SECURITY.md)

<img src="assets/demo.gif" alt="truetop sorting by I/O wait: fio readers blocked on the disk light up red" width="820">

</div>

---

truetop shows per-process CPU, memory, and one column `top`, `htop` and `btop`
do not have: how long each process sat blocked on storage. Everything except
memory is collected inside the kernel by O(1)-per-event eBPF programs and drained
in batched reads rather than one syscall per process, so collection cost tracks
the context-switch rate rather than how many processes are running.

## Contents

[Why](#why) · [Features](#features) · [Requirements](#requirements) · [Install](#install) · [Usage](#usage) · [How it works](#how-it-works) · [Benchmarks](#benchmarks) · [Overhead](#overhead) · [What reads /proc](#what-still-reads-proc) · [Roadmap](#roadmap) · [Contributing](#contributing) · [Security](#security) · [License](#license)

## Why

The premise: build a `top`-class monitor almost entirely in eBPF instead of
procfs, and use that to expose numbers procfs cannot reach at all, not only to
collect the same numbers cheaper. Per-process I/O wait is the first case: `top`,
`htop` and `btop` all read process state from `/proc`, which carries no
per-process uninterruptible-sleep counter to read. eBPF sees the scheduler
directly, so it can show it. Anything else that lives at the scheduler or block
layer and never made it into procfs is a candidate for the same treatment; block
device latency per process is next, see [Roadmap](#roadmap).

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

- **Batched collection.** CPU and I/O wait accumulate on `sched_switch`, and each
  refresh drains the maps with `bpf_map_lookup_batch`, thousands of processes per
  syscall instead of one syscall per process. The maps hold 65,536 processes by
  default on ordinary hardware; past capacity they evict the coldest entry rather
  than refusing new ones, and the status line says so.

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
cargo install --locked --git https://github.com/ramos-99/truetop truetop
```

`--locked` is not optional: without it `cargo install --git` ignores `Cargo.lock`
and resolves the dependency graph afresh, so the build you get is not the build
CI tested.

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

`--max-processes <N>` sizes the kernel accounting maps. The default is derived
from the CPU count rather than fixed, because the maps are preallocated and a
per-CPU one costs `N × 8 bytes × CPUs`: the full 65,536 up to 64 CPUs, tapering
to 16,384 above that. Raise it if the status line reports `at capacity`.

`truetop --bench <TICKS>` runs the collector headless for a fixed number of
ticks, which is what the benchmarks drive. `--help` and `--version` answer
without privileges.

## How it works

Four raw tracepoints and one `fentry` hook, none of which reads `/proc`:

- `sched_switch` accumulates per-process on-CPU time, and I/O wait by stamping a
  timestamp when a task leaves the CPU in uninterruptible sleep and charging the
  interval when it comes back.
- `sched_process_exec` and `sched_process_fork` capture each process's name and
  owning user. Recording the fork is what names children that never call
  `execve`, such as PostgreSQL backends and nginx workers.
- `commit_creds` follows the user when a process changes it, which is how a
  worker that drops privileges after being forked is shown as the user it runs
  as rather than the one it was started by. It is the one hook that is not a
  tracepoint, because the kernel offers none for credential changes.
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

Reproduce these with `cargo xtask bench`. Re-measured 2026-07-29; see
[bench/BENCHMARKS.md](bench/BENCHMARKS.md) for what moved since these were
first published and why.

| metric | truetop | htop / btop |
| --- | --- | --- |
| CPU at ~7,050 processes | 1.3% of a core, flat | 33.6% / 23.6%, climbing |
| CPU under ~3,260 forks/s churn | 1.3% of a core | 2.7% (htop) |
| data syscalls per refresh at ~5,350 | ~3,970 | ~12,900 (htop) |
| per-process collection work | ~26 ns | ~12.3 µs (procfs) |
| RSS, this process, at ~7,050 processes | ~33 MiB, flat | ~19 / ~17 MiB, climbing |

RSS is the one line where truetop starts behind: htop and btop start smaller and
grow with process count, crossing truetop's flat floor somewhere past the range
tested here. Detail in [bench/BENCHMARKS.md](bench/BENCHMARKS.md#selfcpu-the-tools-own-cost).

Method, full results and the kernel-side cost are in
[bench/BENCHMARKS.md](bench/BENCHMARKS.md).

## Overhead

`sched_switch` fires on every context switch, so truetop's cost scales with the
context-switch rate rather than the process count. The per-event program is O(1)
and measured at a median of ~502 ns on the reference machine (turbo off, `tsc`
clocksource - both move this figure, see
[bench/BENCHMARKS.md](bench/BENCHMARKS.md)). Under a `hackbench`
context-switch storm that cost ~+2% wall clock; an ordinary system does orders of
magnitude fewer switches and pays well under 1%.

Roughly 20-45 ns of that is the accounting maps being LRU rather than a fixed
hash, so a full map evicts its coldest entry instead of silently refusing new
processes - see [Features](#features).

truetop is cheaper than htop only below some switch rate that rises with process
count; a switch-heavy machine with few processes costs more, not less. The exact
crossover has not been re-measured against today's per-event cost - see
[bench/BENCHMARKS.md](bench/BENCHMARKS.md#switch-cost-under-a-context-switch-storm).
The figure also tracks the clocksource, since `bpf_ktime_get_ns` runs per event
and roughly triples on `hpet`. Benchmark before deploying on latency-sensitive
hosts.

## What still reads `/proc`

Everything about a process's *behaviour* comes from the kernel: CPU time, I/O
wait, and the name and owning user of anything that starts while truetop runs.
What does not is listed here, rather than left to be found under `strace`:

| what | when | why |
| --- | --- | --- |
| RSS, from `statm` | per visible row, per refresh | no eBPF interface gives an exact figure, see below |
| names and uids of processes that predate truetop | once, at startup | a `bpf_iter` task walk would do it in-kernel, see [Roadmap](#roadmap) |
| `meminfo` and `loadavg`, for the header | two small files, per refresh | machine-wide, so the cost is constant, not per process |
| hostname, CPU model, total memory | once, at startup | the same, and they do not change |

Only the first is permanent. The second is the last O(N) procfs scan in the tool
and is on its way out. The rest is two files a second, whether the machine runs
fifty processes or fifty thousand.

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
  processes which already existed at startup. That sweep is a stopgap: it is the
  last O(N) procfs scan left in truetop, and the only reason startup cost tracks
  the process count rather than being flat like everything else here. A task
  iterator reads the same names and uids from `task_struct` in one kernel-side
  pass. [aya](https://aya-rs.dev) already loads and attaches iterators from user
  space; `aya-ebpf` has no iterator program type yet, so the kernel half would
  have to be hand-rolled against a raw context. **Contributions adding iterator
  support to aya would be very welcome**, and would reduce this to a small change
  here.
- Memory in-kernel, once an accurate per-process interface exists.
- Kernels 5.10 through 5.13 in CI, which take the pre-5.14 `state` field path,
  and aarch64 at runtime.

## Contributing

The toolchain, the three test layers (unit, root-only integration, and the
14-kernel VM matrix) and the benchmark harness are documented in
[CONTRIBUTING.md](CONTRIBUTING.md). Bug reports want `uname -a`, the distro and
the architecture; a CO-RE issue cannot be triaged without them.

## Security

> [!IMPORTANT]
> truetop loads eBPF with `CAP_BPF` and `CAP_PERFMON`, or root. Report
> vulnerabilities privately through the [security policy](SECURITY.md), not a
> public issue.

## License

User space is MIT or Apache-2.0. The eBPF code in `truetop-ebpf/` is GPL-2.0 or
MIT, matching the licence it declares to the kernel verifier.
