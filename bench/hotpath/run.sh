#!/usr/bin/env bash
#
# Hotpath benchmark: sched_switch per-event kernel cost, sampled as a distribution
# under a sustained hackbench storm (median + spread from bpftool run stats), plus
# a coarse whole-system overhead. See ../BENCHMARKS.md.
#
# Quiet machine, on AC (xtask tunes governor/turbo): sudo ./run.sh
set -euo pipefail
cd "$(dirname "$0")"

TRUETOP=../../target/release/truetop
TICKS=120
GROUPS_N=$(nproc)          # not GROUPS: that is a special bash array (group ids)
LOOPS=5000
SAMPLES=24
INTERVAL=0.25
WARMUP=2
RESULTS="$(cd "$(dirname "$0")/.." && pwd)/results"
mkdir -p "$RESULTS"

for c in bpftool jq hackbench nproc; do
    command -v "$c" >/dev/null || { echo "missing dependency: $c" >&2; exit 1; }
done
[[ -x $TRUETOP ]] || { echo "build first: cargo build --release" >&2; exit 1; }
[[ $EUID -eq 0 ]] || { echo "run as root (truetop loads eBPF): sudo $0" >&2; exit 1; }

gov=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || true)
[[ ${gov:-} == performance ]] || echo "warning: governor '${gov:-unknown}' != performance; run via 'cargo xtask bench hotpath' for stable numbers" >&2

prog_stats() {
    bpftool prog show --json 2>/dev/null \
        | jq -r 'first(.[] | select(.name == "sched_switch"))
                 | "\(.run_cnt // 0) \(.run_time_ns // 0)"' 2>/dev/null \
        || echo "0 0"
}

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

stats=$(awk -v warm="$WARMUP" '
    { v[NR] = $1 }
    END {
        n = 0
        for (i = warm + 1; i <= NR; i++) a[++n] = v[i]
        if (n == 0) { print "n/a n/a n/a n/a n/a n/a"; exit }
        for (i = 1; i <= n; i++)
            for (j = i + 1; j <= n; j++)
                if (a[j] < a[i]) { t = a[i]; a[i] = a[j]; a[j] = t }
        med = (n % 2) ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2
        printf "%d %.1f %.1f %.1f %.1f %.1f", n, med, a[int(n * 0.25) + 1], a[int(n * 0.75) + 1], a[1], a[n]
    }' "$samples")
rm -f "$samples"

read -r n med p25 p75 lo hi <<<"$stats"
overhead=$(awk "BEGIN { printf \"%+.0f\", ($with - $baseline) / $baseline * 100 }")

printf '\n'
{
    printf 'per-event kernel cost (median) : %s ns  (n=%s)\n' "$med" "$n"
    printf 'per-event IQR  [p25, p75]      : [%s, %s] ns\n' "$p25" "$p75"
    printf 'per-event range [min, max]     : [%s, %s] ns\n' "$lo" "$hi"
    printf 'coarse storm overhead (o.o.m.) : ~%s%% (%s s -> %s s)\n' "$overhead" "$baseline" "$with"
} | tee "$RESULTS/hotpath.txt"
