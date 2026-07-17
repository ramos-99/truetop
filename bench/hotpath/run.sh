#!/usr/bin/env bash
#
# Hotpath benchmark: sched_switch per-event kernel cost under a hackbench storm.
# bpf_stats times the whole invocation; perf's `bpf_prog_*` symbol covers only the
# program's own code, so the difference is what the helper calls cost. The figure
# tracks the machine's clocksource, which env.txt records. See ../BENCHMARKS.md.
#
# Quiet machine, on AC (xtask tunes governor/turbo): sudo ./run.sh
set -euo pipefail
cd "$(dirname "$0")"
source ../lib.sh

TRUETOP=../../target/release/truetop
TICKS=120
GROUPS_N=$(nproc)          # not GROUPS: that is a special bash array (group ids)
LOOPS=5000
SAMPLES=24
INTERVAL=0.25
WARMUP=2
PERF_SECS=10
PERF_HZ=999
RESULTS="$(cd "$(dirname "$0")/.." && pwd)/results"
mkdir -p "$RESULTS"
TCK=$(getconf CLK_TCK)

require bpftool jq hackbench nproc perf getconf
require_built "$TRUETOP"
require_root
warn_if_not_performance hotpath

# CPU time the kernel accounts as busy, summed over cores, in jiffies.
busy_jiffies() { awk '/^cpu / { print $2 + $3 + $4 + $7 + $8 + $9 }' /proc/stat; }

hackbench_once() { hackbench --pipe --groups "$GROUPS_N" --loops "$LOOPS" >/dev/null 2>&1 || true; }

hackbench_best() {
    local best="" t
    for _ in 1 2 3; do
        t=$(hackbench --pipe --groups "$GROUPS_N" --loops "$LOOPS" 2>/dev/null | awk '/Time/ {print $2}')
        [[ -n $t ]] || continue
        if [[ -z $best ]] || awk "BEGIN { exit !($t < $best) }"; then best=$t; fi
    done
    echo "$best"
}

prev_stats=$(cat /proc/sys/kernel/bpf_stats_enabled 2>/dev/null || echo 0)
truetop_pid=
storm_pid=
cleanup() {
    [[ -n $storm_pid ]] && kill "$storm_pid" 2>/dev/null || true
    [[ -n $truetop_pid ]] && kill "$truetop_pid" 2>/dev/null || true
    sysctl -qw kernel.bpf_stats_enabled="$prev_stats" 2>/dev/null || true
}
trap cleanup EXIT
sysctl -qw kernel.bpf_stats_enabled=1

echo "== coarse overhead: hackbench without truetop ==" >&2
baseline=$(hackbench_best)

"$TRUETOP" --bench "$TICKS" >/dev/null 2>&1 &
truetop_pid=$!
sleep 2

echo "== coarse overhead: hackbench with truetop ==" >&2
with=$(hackbench_best)

echo "== per-event distribution ($SAMPLES windows) ==" >&2
( while kill -0 "$truetop_pid" 2>/dev/null; do hackbench_once; done ) &
storm_pid=$!

samples="$(mktemp)"
read -r c0 n0 <<<"$(prog_stats)"
for _ in $(seq 1 "$SAMPLES"); do
    sleep "$INTERVAL"
    read -r c1 n1 <<<"$(prog_stats)"
    dc=$((c1 - c0)); dn=$((n1 - n0)); c0=$c1; n0=$n1
    if ((dc > 0)); then
        awk "BEGIN { printf \"%.1f\n\", $dn / $dc }" >> "$samples"
    fi
done
read -r n med p25 p75 lo hi <<<"$(iqr_stats <"$samples")"
rm -f "$samples"

echo "== perf cross-check (${PERF_SECS}s) ==" >&2
perf_data=$(mktemp)
b0=$(busy_jiffies)
read -r pc0 _ <<<"$(prog_stats)"
perf record -a -q -e cycles -F "$PERF_HZ" -o "$perf_data" -- sleep "$PERF_SECS" >/dev/null 2>&1 || true
b1=$(busy_jiffies)
read -r pc1 _ <<<"$(prog_stats)"

read -r perf_total perf_prog <<<"$(perf script -i "$perf_data" 2>/dev/null \
    | awk '/bpf_prog_[0-9a-f]*_sched_switch/ { p++ } END { print NR, p + 0 }')"
rm -f "$perf_data"

perf_ns=$(awk "BEGIN {
    events = $pc1 - $pc0
    if (events <= 0 || $perf_total <= 0) { print 0; exit }
    printf \"%.1f\", ($perf_prog / $perf_total) * (($b1 - $b0) / $TCK) / events * 1e9
}")
perf_share=$(awk "BEGIN { if ($perf_total > 0) printf \"%.2f\", $perf_prog / $perf_total * 100; else print 0 }")
helpers_ns=$(awk "BEGIN { printf \"%.1f\", $med - $perf_ns }")
overhead=$(awk "BEGIN { printf \"%+.0f\", ($with - $baseline) / $baseline * 100 }")

printf '\n'
{
    printf 'per-event total (bpf_stats)    : %s ns  (median, n=%s, IQR [%s, %s], range [%s, %s])\n' \
        "$med" "$n" "$p25" "$p75" "$lo" "$hi"
    printf '  program code (perf)          : %s ns  (%s%% of busy cycles, %s samples)\n' \
        "$perf_ns" "$perf_share" "$perf_total"
    printf '  helper calls (difference)    : %s ns  (ktime, probe reads, map ops)\n' "$helpers_ns"
    printf 'coarse storm overhead (o.o.m.) : ~%s%% (%s s -> %s s)\n' "$overhead" "$baseline" "$with"
} | tee "$RESULTS/hotpath.txt"
