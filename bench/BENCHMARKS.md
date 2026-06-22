# Benchmarks

Extracting per-process CPU is O(1) in syscalls with eBPF vs O(N) with procfs.
Both are O(N) in time; the difference is a constant factor — the eBPF path makes
no per-process syscall, avoiding the VFS open/read and the text formatting of
`/proc/<pid>/stat`.

## scaling

```sh
cargo bench -p truetop-bench
```

Sweeps N and compares two ways to get the same result (total on-CPU ns over N
processes):

| path           | source                                       | syscalls |
| -------------- | -------------------------------------------- | -------- |
| ebpf_batched   | one `bpf_map_lookup_batch` (modelled in mem) | O(1)     |
| procfs_per_pid | open/read/parse `/proc/<pid>/stat` per pid   | O(N)     |

Criterion writes a log-Y HTML report to `target/criterion/`. Both lines rise
linearly; the eBPF one sits ~3500x lower (~4 ns vs ~14 µs per process).

`ebpf_batched` is modelled in memory, so it makes no real `bpf()` call — it
measures the per-process work, not the syscall count. The syscall count is the
collector itself (`batch::BatchReader`, one `BPF_MAP_LOOKUP_BATCH` per tick),
verifiable with `strace -c` in the macro benchmark.

`procfs_per_pid` re-reads `/proc/self/stat` (one cached file), the best case for
procfs; btop reads N distinct files, so the real gap is wider.

Not measured here: the `sched_switch` program runs on every context switch (O(1)
per event, not zero); `bpftool prog show` reports that cost.

Results compare against the last run's baseline, so a busy machine prints
spurious regressed/improved lines — `rm -rf target/criterion` and pin the
governor to `performance` for clean numbers.
