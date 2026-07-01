#!/usr/bin/env bash
#
# Hotpath benchmark: the sched_switch per-event kernel cost, and the whole-system
# overhead it imposes. This is the cost the syscall (macro) benchmark deliberately
# excludes - truetop's program runs in the kernel on every context switch, which
# top/htop have no equivalent of. See ../BENCHMARKS.md.
#
# Two numbers:
#   - per-event ns  - run_time_ns/run_cnt from BPF run statistics (bpftool).
#   - overhead %    - hackbench wall-clock without vs with truetop attached.
#
# Quiet machine, on AC: sudo ./run.sh
set -euo pipefail
cd "$(dirname "$0")"

TRUETOP=../../target/release/truetop
TICKS=60            # keep truetop attached long enough to span the load window
RESULTS="$(cd "$(dirname "$0")/.." && pwd)/results"
mkdir -p "$RESULTS"
RESULTS="$(cd "$(dirname "$0")/.." && pwd)/results"
mkdir -p "$RESULTS"

for c in bpftool jq hackbench; do
    command -v "$c" >/dev/null || { echo "missing dependency: $c" >&2; exit 1; }
done
[[ -x $TRUETOP ]] || { echo "build first: cargo build --release" >&2; exit 1; }
[[ $EUID -eq 0 ]] || { echo "run as root (truetop loads eBPF): sudo $0" >&2; exit 1; }

# run_cnt and run_time_ns for the sched_switch program; "0 0" if not loaded yet.
prog_stats() {
    bpftool prog show --json 2>/dev/null \
        | jq -r 'first(.[] | select(.name == "sched_switch"))
                 | "\(.run_cnt // 0) \(.run_time_ns // 0)"' 2>/dev/null \
        || echo "0 0"
}

# Fastest of three runs of a context-switch-heavy workload, in seconds. Uses the
# getopt flags of the rt-tests hackbench (not the old positional form) and enough
# loops to run ~1-2s, so a few-percent overhead clears the run-to-run noise that a
# 60ms workload buries. Fastest-of-three drops upward scheduling jitter.
hackbench_secs() {
    local best="" t
    for _ in 1 2 3; do
        t=$(hackbench --pipe --groups 16 --loops 5000 2>/dev/null | awk '/Time/ {print $2}')
        [[ -n $t ]] || continue
        if [[ -z $best ]] || awk "BEGIN { exit !($t < $best) }"; then
            best=$t
        fi
    done
    echo "$best"
}

# BPF run stats add a small per-run cost, so enabling them slightly inflates the
# "with truetop" figure - conservative, and disclosed in BENCHMARKS.md.
prev_stats=$(cat /proc/sys/kernel/bpf_stats_enabled 2>/dev/null || echo 0)
sysctl -qw kernel.bpf_stats_enabled=1
trap 'sysctl -qw kernel.bpf_stats_enabled="$prev_stats" 2>/dev/null || true' EXIT

echo "== baseline: hackbench, truetop not running ==" >&2
baseline=$(hackbench_secs)

echo "== hackbench with truetop attached ==" >&2
"$TRUETOP" --bench "$TICKS" >/dev/null 2>&1 &
truetop_pid=$!
sleep 2                                          # let it load and attach

read -r cnt0 ns0 <<<"$(prog_stats)"
with=$(hackbench_secs)
read -r cnt1 ns1 <<<"$(prog_stats)"

kill "$truetop_pid" 2>/dev/null || true
wait "$truetop_pid" 2>/dev/null || true

events=$((cnt1 - cnt0))
elapsed_ns=$((ns1 - ns0))
per_event="n/a"
((events > 0)) && per_event=$(awk "BEGIN { printf \"%.1f\", $elapsed_ns / $events }")
overhead=$(awk "BEGIN { printf \"%+.1f\", ($with - $baseline) / $baseline * 100 }")

printf '\n'
{
    printf 'sched_switch events sampled : %d\n' "$events"
    printf 'per-event kernel cost       : %s ns\n' "$per_event"
    printf 'hackbench baseline          : %s s\n' "$baseline"
    printf 'hackbench with truetop      : %s s (%s%%)\n' "$with" "$overhead"
} | tee "$RESULTS/hotpath.txt"
