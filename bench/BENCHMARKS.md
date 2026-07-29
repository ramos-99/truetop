# Benchmarks

![collection](https://img.shields.io/badge/collection-eBPF-F76800?style=flat-square&logo=linux&logoColor=white)
![syscalls](https://img.shields.io/badge/syscalls-O%281%29-2EA043?style=flat-square)
![vs procfs](https://img.shields.io/badge/vs_procfs-O%28N%29-C74634?style=flat-square)

![harness](https://img.shields.io/badge/harness-criterion_%C2%B7_strace_%C2%B7_bpftool-8250DF?style=flat-square)
![built with](https://img.shields.io/badge/Rust-2024-DEA584?style=flat-square&logo=rust&logoColor=white)

Reference machine: AMD Ryzen 7 5800HS, 8 cores / 16 threads. Re-measured
2026-07-29 against the code in this tree; details and caveats below.

Two things moved since this page was first written:

- The batched read stopped at the first partially-filled batch instead of at
  `ENOENT`, so above roughly 4,000 live processes the collector folded ~4,095
  entries and silently dropped the rest (fixed in `2ad0ecf`). The
  syscalls-per-refresh figure below is higher than previously published because
  it now correctly counts the batch calls that cover the full process count,
  not because the collector got slower.
- The accounting maps moved from a fixed-size hash to LRU, which costs 20-45
  ns/event on the hotpath in exchange for never silently reporting 0% at
  capacity. Priced in [What capacity costs](#what-capacity-costs).

| metric                              | truetop               | htop / btop           |
| ----------------------------------- | --------------------- | --------------------- |
| CPU at ~7,050 processes             | **1.3%** of a core, flat | 33.6% / 23.6%, climbing |
| CPU under ~3,260 forks/s churn      | **1.3%** of a core    | 2.7% (htop)            |
| data syscalls per refresh at ~5,350 | **~3,970**             | ~12,900 (htop)         |
| per-process collection work         | **~26 ns**             | ~12.3 µs (procfs)      |

*Exclusive:* per-process D-state I/O wait, which none of them show. *Cost:* the
kernel `sched_switch` hook runs on every context switch (~500 ns on this
machine, see [hotpath](#hotpath-kernel-cost-per-context-switch)), so truetop is
cheaper than htop only below some switch rate that grows with process count -
see [switch](#switch-cost-under-a-context-switch-storm), not re-measured this
round. RSS is a higher, flat ~33 MiB floor - see
[selfcpu](#selfcpu-the-tools-own-cost).

Per-process CPU is collected in-kernel and pulled in one batched syscall, instead
of parsing `/proc/<pid>/stat` once per process. That makes CPU collection O(1) in
syscalls per refresh instead of O(N). The price is an O(1) eBPF program on every
context switch, measured in the hotpath benchmark below.

## Result

![Monitor CPU% versus process count](results/selfcpu-cpu.svg)

CPU% of one core, median, htop/btop at a 1s refresh. truetop is measured twice:
`truetop-ui` is the normal interactive program, `truetop-collector` is the same
collection loop run headless (`--bench`, no terminal, nothing drawn). The gap
between the two columns is what the renderer itself costs - here, close to
nothing, since it only repaints when the snapshot actually changes:

| processes | htop | btop | truetop-ui | truetop-collector |
| --------: | ---: | ---: | ---------: | -----------------: |
|       398 |  3.0 |  0.7 |         0.7 |                 0.0 |
|     1,382 |  7.0 |  2.0 |         1.0 |                 0.7 |
|     2,875 | 12.0 |  4.3 |         0.7 |                 0.7 |
|     5,357 | 21.6 |  9.3 |         1.0 |                 1.0 |
|     7,052 | 33.6 | 23.6 |         1.3 |                 1.3 |

htop and btop read one `/proc` entry per process each refresh (O(N)); truetop
batch-reads the CPU map and touches `/proc` only for the visible viewport, so it
stays flat. At ~7,050 processes that is ~26x less CPU than htop and ~18x less
than btop, and the gap widens with N.

The cost is flat with process turnover too, not just count. Under 3,261 forks
per second at 371 live processes - the churn of a heavy parallel build - truetop
reads every fork and exit in the kernel and reaps each departed process's
counters, yet collection still sits at 1.3% of a core, below htop's 2.7%. The
per-exit bookkeeping does not show up.

This is the user-space cost; the kernel `sched_switch` price and the RSS trade-off
are in the sections below.

## At a glance

|             | Measures                          | Method                            | Run                         |
| ----------- | --------------------------------- | --------------------------------- | --------------------------- |
| **micro**   | per-process collection work       | in-memory model, no `bpf()` call  | `cargo xtask bench micro`   |
| **macro**   | data syscalls per refresh at scale | `strace` vs top / htop           | `cargo xtask bench macro`   |
| **hotpath** | kernel cost per context switch    | `bpftool` run stats + `hackbench` | `cargo xtask bench hotpath` |
| **selfcpu** | the tool's own CPU% and RSS, by process count and by churn | sample `/proc` vs btop / htop | `cargo xtask bench selfcpu` |
| **switch**  | that cost under a switch storm    | `stress-ng --switch` vs btop / htop | `cargo xtask bench switch` |

## Running

```sh
cargo xtask bench                  # micro, macro, hotpath
cargo xtask bench micro macro      # or any subset, in any order
cargo xtask bench selfcpu switch   # opt-in: need btop/htop/stress-ng installed
```

One command builds what it needs, runs the benchmarks, and for stable numbers
pins the CPU governor to `performance`, disables turbo, and pins the micro
benchmark to one core. It restores the original CPU settings on exit, including on
error, so the machine is never left tuned. Pass `--no-prep` to skip the tuning (on
a VM or in CI). Everything but micro needs root, so it elevates with `sudo`.

Every benchmark writes to `bench/results/`: the criterion report under
`criterion/`, plus `macro.csv`, `hotpath.txt`, `selfcpu.csv`, `switch.csv`, the
`scaling.svg` / `selfcpu-cpu.svg` / `selfcpu-rss.svg` plots, and `env.txt` (commit,
kernel, CPU, governor, clocksource). Only the `.svg` plots are tracked in git; the
rest is regenerated per run.

### Requirements

micro needs only a Rust toolchain; the rest shell out to standard tools and need
root. Each script names its own missing dependencies before doing any work, so a
partial install fails on the benchmark that needs the tool, not halfway through it.

| benchmark | tools it calls                                                            |
| --------- | ------------------------------------------------------------------------- |
| micro     | `cargo` (criterion is a dev-dependency); `taskset` used for pinning if present |
| macro     | `strace`, `script`, `top`, `htop`; `python3` + `matplotlib` for the plot  |
| hotpath   | `bpftool`, `jq`, `hackbench`, `perf`                                       |
| selfcpu   | `script`, `btop`, `htop`; `python3` + `matplotlib` for the plot           |
| switch    | `script`, `btop`, `htop`, `stress-ng`, `bpftool`, `jq`                     |

`script`, `taskset`, and `timeout` come from util-linux/coreutils and are almost
always already installed. The rest, by distro:

```sh
# Arch (hackbench is in the AUR via rt-tests)
sudo pacman -S --needed strace htop btop bpf jq perf stress-ng python-matplotlib

# Debian / Ubuntu
sudo apt install strace htop btop jq rt-tests stress-ng python3-matplotlib linux-tools-$(uname -r)
```

Package names vary; the table above is the source of truth for what must be on
`PATH`. `bpftool` ships with the kernel tools (`bpf` on Arch,
`linux-tools-$(uname -r)` on Debian), and `hackbench` ships with `rt-tests`.

## micro: per-process work

```sh
cargo xtask bench micro            # wraps: cargo bench -p truetop-bench
```

Sweeps N and compares the two collection paths. Both compute the same thing, the
total on-CPU nanoseconds over N processes:

| path             | source                                         | syscalls |
| ---------------- | ---------------------------------------------- | -------- |
| `ebpf_batched`   | fold per-CPU slots into the `tgid → ns` map    | O(1)     |
| `procfs_per_pid` | open / read / parse `/proc/<pid>/stat` per pid | O(N)     |

The eBPF arm builds a `HashMap` the same way `batch::fold` does: it sums each
key's per-CPU slots and inserts the result. A bare summation would leave out the
insert and undercount what the collector actually does per process.

Criterion writes a log-Y HTML report to `bench/results/criterion/`. Both lines
rise linearly, and the eBPF one sits orders of magnitude lower. At 10,000
processes: ~26 ns per process for eBPF against ~12.3 µs for procfs, ~480x. The
figures are machine-specific, so regenerate them for yours; the shape holds.

`procfs_per_pid` re-reads `/proc/self/stat`, one cached file, which is the best
case for procfs. The macro benchmark reads N distinct files, so the real gap is
wider. What the collector actually issues per tick (`batch::BatchReader`) is
measured in the macro benchmark below.

## macro: versus top and htop

![CPU collection cost versus process count](results/scaling.svg)

Per-process data syscalls per refresh, traced with `strace` against the real
tools. One trace per row, not a repeated median like the other benchmarks, so
read the low end as noisy rather than exact:

| Processes | top    | htop   | truetop | truetop's edge |
| --------: | -----: | -----: | ------: | -------------: |
|       343 |    688 |    266 |     561 |           0.5x |
|       592 |  1,186 |    757 |   1,404 |           0.5x |
|       845 |  1,640 |  1,381 |   1,543 |           0.9x |
|     1,347 |  2,697 |  2,511 | **1,815** |         1.4x |
|     2,348 |  4,699 |  4,279 | **2,355** |         1.8x |
|     5,349 | 10,030 | 12,897 | **3,968** |         3.2x |

Below ~850 processes htop costs fewer syscalls than truetop: with almost every
process visible at once, truetop's per-row `statm` and name-lookup reads track
active count directly, and procfs's single flat read per process wins at that
scale. Past ~1,300 truetop pulls ahead and the gap widens with N. Edge is the
fewest-syscall competitor divided by truetop; lower than 1 means truetop lost.

This is higher than previously published (~780, flat) because that figure was
measuring a bug: the batched read stopped at the first partially-filled 4,096-key
batch, so above that count the collector silently read less than the true map.
Above ~4,096 tracked processes a second `BPF_MAP_LOOKUP_BATCH` call is now
correctly issued per map, which is real, counted work that the old number
omitted.

```sh
cargo xtask bench macro            # wraps: sudo bench/macro/run.sh + plot.py
```

Counts per-process data syscalls per refresh against the real tools under
`strace -fy`: `read` / `pread64` on `/proc/<pid>/{stat,statm,status,cmdline}`
(`-y` resolves cached fds back to paths, so reads off a held fd still count) plus
`bpf`, divided by refreshes. A load generator (`src/bin/load.rs`) forks N
processes that reschedule at ~0% CPU. Truly idle ones never reschedule, so no
monitor would see them anyway.

These per-refresh figures are an approximation, not an exact count. Reads are
divided by a refresh count inferred per tool: top and htop by each reopen of the
`/proc` directory, truetop by its fixed tick count. The proxy is close, because
top and htop genuinely reopen `/proc` every scan, but read the numbers as the
scaling shape rather than exact syscall counts.

The collector reads the CPU map with `BPF_MAP_LOOKUP_BATCH` (one call per 4,096
live pids: one below that count, two above it, and so on) and touches `/proc`
only for the visible viewport, up to 512 rows (the union of the top 256 by CPU
and by I/O wait), each a `statm` read plus a `comm` map lookup, itself a `bpf`
syscall. So the per-refresh cost is O(N/4,096) batch calls plus O(viewport)
lookups: flat once the viewport saturates, with a step at each batch boundary.
That step is visible in the table above, between 2,348 and 5,349 processes.

**Why `--bench`.** truetop runs headless (`truetop --bench <ticks>`). `strace`
ptrace-traps every syscall, and the TUI input poll issues enough of them to starve
the collector under tracing, so the traced run never completes a tick. `--bench`
drives the collector directly with no terminal, so the trace covers the collection
path only. The tick count is fixed, so reads divide by it without a per-scan
marker.

**Why btop is omitted.** It reads `/proc/<pid>` per process like htop (same O(N)
class), but its TUI defeats `strace` the same way truetop's does. htop covers the
procfs case.

Not counted on truetop's side: the `sched_switch` program, which runs in the
kernel on every context switch. The count here is the user-space refresh cost,
where the O(1) vs O(N) split is. The kernel cost is the hotpath benchmark.

## hotpath: kernel cost per context switch

```sh
cargo xtask bench hotpath          # wraps: sudo bench/hotpath/run.sh
```

The macro benchmark counts the per-process syscalls truetop avoids in user space.
This one counts what it adds in the kernel. `sched_switch` runs on every context
switch, which is the trade-off the README's Overhead section describes.

With `kernel.bpf_stats_enabled`, `run_time_ns / run_cnt` is the whole invocation,
helper calls included; it is sampled over short windows during a `hackbench` storm
and reported as a median with IQR. `perf` separately attributes cycles to the JIT'd
`bpf_prog_*` symbol, which covers the program's own code only: helpers are kernel
functions and land in their own symbols, so the difference is what they cost.
`hackbench` also runs with and without truetop attached for a coarse wall-clock
comparison, which is the number that survives a change of machine: the per-event
ns below moves with clocksource, CPU frequency and mitigation state (all in
`env.txt`), the percentage is what those cancel out of.

Reference machine (Ryzen 7 5800HS, 8 cores / 16 threads, turbo off, `tsc`
clocksource, mitigations as shipped by the distro), under `hackbench`:
**~+2% wall-clock overhead** (5.448 s -> 5.536 s for the same hackbench run,
with truetop attached). Per-event: **~502.5 ns**, IQR [498.9, 505.4] (n=22).
perf puts ~130 ns of that in the program's own code and ~372 ns in helper calls:
the clock read, the `task_struct` probe reads, and the map operations.

It is not one number. It tracks cache locality, because the probe reads dominate:
hackbench churns 640 distinct tasks, while `stress-ng --switch` alternates
between few hot ones and costs less per event (see
[switch](#switch-cost-under-a-context-switch-storm), not re-measured this
round). It also tracks the clocksource, since `bpf_ktime_get_ns` runs once per
event: a `tsc` read is ~20 ns, an `hpet` one ~1 µs, which roughly triples the
per-event cost - `env.txt` now also records turbo state and CPU mitigations,
since both can move this figure with no code change at all.

Four changes from an earlier optimisation pass, same machine, before any of the
work below:

| stopwatch, state read, prev ids                 | per-event          |
| ----------------------------------------------- | ------------------ |
| tid-keyed per-CPU hashmap, probe-read state     | ~630 ns            |
| single-slot array, in-place counter add         | ~434 ns            |
| `prev_state` from the tracepoint arg (>= 5.18)  | ~391 ns            |
| `bpf_get_current_pid_tgid` for prev's ids       | ~335 ns [333, 337] |

Each step's A/B had non-overlapping IQRs at the time. The 335 ns endpoint does
not reproduce today on identical code (479 ns) - turbo and mitigation state
weren't recorded when this table was made, and one of them has likely moved.
Read the steps as relative costs, not as absolutes to compare against below.

### Capacity is not what these measure

No benchmark here reaches the accounting maps' capacity, and none should: a
saturated map evicts, so the run would measure degraded behaviour rather than the
design. The margin is deliberate - `selfcpu` tops out at 10,000 processes and
`macro` at 5,000, against a default capacity of 65,536. Scaling either past that
means raising `--max-processes` in the same change.

### What capacity costs

The accounting maps were plain per-CPU hashes of a fixed 16,384 entries, and a
full one returns `E2BIG`: the hotpath dropped the write, and that process read
0.0% for the rest of its life. They are LRU now, so a full map evicts its coldest
entry instead and accounting degrades rather than lying. `--max-processes` sets
the size; its default is capped by a memory budget, because an LRU map is
preallocated and a per-CPU one costs `entries x 8 x possible_cpus`.

Measured under `hackbench`, tsc clocksource, turbo off:

| accounting map                     | per-event                |
| ---------------------------------- | ------------------------ |
| plain per-CPU hash, 16,384 entries | 479.0 ns [477.4, 481.3]  |
| LRU, 65,536 entries                | 500.1 - 502.5 ns         |
| LRU, 16,384 entries                | 522.0 ns [513.7, 525.8]  |

**LRU costs 20-45 ns per event.** The kernel sets a reference bit on every
lookup a BPF program makes into an LRU map; the per-tick batch read from user
space does not, since the syscall lookup path deliberately skips marking, so it
cannot disturb eviction order. This is steady-state cost, not eviction -
hackbench's few hundred tasks sit well under either map's capacity. Map size
(16,384 vs 65,536) made no measurable difference; the cost is the map type.

We took the trade. A monitor may run out of capacity; it must not report zero and
say nothing.

One change we did not make: raising the minimum kernel would let the one global map
on this path move to task-local storage, but perf prices that map's lookup at a
fraction of a percent of storm cycles, ~5% of the per-event cost after the
replacement's own overhead. Not worth dropping 5.10 support. A later version can
detect the kernel at load and take the faster path where it exists, giving 5.15+
users the win, once the VM matrix in CI can test both paths. Note the fit is
specific to that map: `SLEEP_SINCE` holds per-thread state that dies with the
thread, which is what task storage is for. The per-process counters are the
opposite case - they have to outlive the task, since a process that lived and
died between two ticks is still charged - so they cannot move there without an
exit-time flush into a map, which is the thing being replaced. aya-ebpf also
exposes no task-storage map type today.

## selfcpu: the tool's own cost

```sh
cargo xtask bench selfcpu          # wraps: sudo bench/selfcpu/run.sh + plot.py
```

The chart and table are the Result at the top. The other benchmarks measure
truetop's collection path; this one measures the whole running process. It samples
`/proc/<pid>/stat` for each monitor under a controlled load (the `load` generator
forks N processes that reschedule at ~0% CPU so every monitor sees them), and
reports CPU% as a median with IQR over windows long enough to average out the
refresh, plus RSS from `statm`. truetop appears twice: with its UI (parity with
btop/htop, which render) and as the headless collector (`--bench`). At idle the UI
and the collector read the same ~0: redraw-on-change means the UI does not repaint
while the snapshot is unchanged.

This is the user-space cost only. truetop's total also carries the kernel
`sched_switch` cost from the hotpath benchmark, which htop and btop do not pay; it
scales with the context-switch rate, so on a switch-heavy machine it can offset the
user-space saving.

![Monitor RSS versus process count](results/selfcpu-rss.svg)

RSS is a trade-off. truetop has a higher fixed floor (~33 MiB) but stays flat;
htop and btop start near 8 MiB and grow with N (19.2 / 16.7 MiB at 7,052
processes), crossing truetop somewhere beyond the range tested here. The floor
moved up from a previously published ~20 MiB - not from the larger accounting
maps, which are kernel memory and do not appear in a process's RSS, but most
likely the extra BTF parsing the `commit_creds` hook does at load.

Caveats: htop and btop CPU% depends on their configuration (columns, tree view,
meters); these ran defaults at a normalised 1s refresh, and the figure is
terminal-geometry dependent (render cost scales with visible rows). The jiffy
resolution (~0.3% per window) separates truetop from the others but is too coarse
to split the UI from the collector; that needs `perf stat`.

## switch: cost under a context-switch storm

Not re-run in the 2026-07-29 pass above; these numbers are older and shown for
the shape of the trade-off, not as current absolutes.

```sh
cargo xtask bench switch           # wraps: sudo bench/switch/run.sh
```

selfcpu loads the machine with processes, which is what htop and btop scale with.
This one loads it with context switches, which is what truetop scales with:
`stress-ng --switch` drives the rate while few processes exist. Each tool's own
CPU% comes from `/proc`; for truetop the kernel `sched_switch` cost is added from
bpf_stats, so its total is user-space plus kernel against htop/btop's user-space.

Reference machine, CPU% of one core:

| ctxt/s | htop | btop | truetop (user + kernel) |
| -----: | ---: | ---: | ----------------------: |
|   661k | 3.80 | 0.90 |  **11.5** (0.6 + 10.9)  |
|  2.09M | 3.90 | 0.60 |  **43.1** (0.6 + 42.5)  |
|  4.84M | 5.10 | 1.00 | **124.4** (0.9 + 123.5) |

truetop loses here, and by a lot. Its user-space stays flat (~0.6%), but the kernel
hook runs on every switch, so its total tracks the switch rate; htop and btop do
not care about the rate at all.

Together with selfcpu that gives the whole trade-off in one line. truetop is
cheaper than htop while

    context switches/s  <  ~80,000 + 130 x (process count)

A desktop at a few hundred processes and tens of thousands of switches per second
sits far inside that. A switch-heavy server with few processes does not: there,
truetop costs more than the tool it replaces.
