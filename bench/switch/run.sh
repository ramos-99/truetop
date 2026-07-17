#!/usr/bin/env bash
#
# switch benchmark: monitoring cost under a high context-switch rate with few
# processes, the axis truetop scales with. stress-ng drives the switches; each
# tool's own CPU% comes from /proc, and for truetop the kernel sched_switch cost is
# added from bpf_stats, so its total is user-space + kernel against htop/btop's
# user-space. See ../BENCHMARKS.md.
#
# Quiet machine, on AC (xtask tunes governor/turbo): sudo ./run.sh
set -euo pipefail
cd "$(dirname "$0")"
source ../lib.sh
export TERM="${TERM:-xterm-256color}"

WORKERS=(1 4 16)          # stress-ng --switch instances: rising switch rate
WINDOWS=6
INTERVAL=3
WARMUP=1
ROWS=50
COLS=200
TRUETOP=../../target/release/truetop
RESULTS="$(cd "$(dirname "$0")/.." && pwd)/results"
OUT="$RESULTS/switch.csv"
mkdir -p "$RESULTS"
TCK=$(getconf CLK_TCK)

require script btop htop stress-ng bpftool jq pgrep getconf
require_built "$TRUETOP"
require_root
warn_if_not_performance switch

BTOP_HOME=$(btop_home)
stress_pid=
wrapper=
pid=
prev_stats=$(cat /proc/sys/kernel/bpf_stats_enabled 2>/dev/null || echo 0)
cleanup() {
    [[ -n $pid ]] && kill "$pid" 2>/dev/null || true
    [[ -n $wrapper ]] && kill "$wrapper" 2>/dev/null || true
    [[ -n $stress_pid ]] && kill "$stress_pid" 2>/dev/null || true
    sysctl -qw kernel.bpf_stats_enabled="$prev_stats" 2>/dev/null || true
    rm -rf "$BTOP_HOME"
}
trap cleanup EXIT
sysctl -qw kernel.bpf_stats_enabled=1

ctxt() { awk '/^ctxt/ {print $2}' /proc/stat; }
median() { iqr_stats | cut -d' ' -f2; }

# measure LABEL PGREP_NAME KERNEL MODE CMD...
#   KERNEL=1 also samples the sched_switch program cost (truetop only).
measure() {
    local label=$1 name=$2 kernel=$3 mode=$4
    shift 4
    if [[ $mode == pty ]]; then
        script -qfc "stty rows $ROWS cols $COLS 2>/dev/null; exec $*" /dev/null >/dev/null 2>&1 &
    else
        "$@" >/dev/null 2>&1 &
    fi
    wrapper=$!
    sleep 1
    pid=$(pgrep -n -x "$name" 2>/dev/null || true)
    if [[ -z $pid ]]; then
        echo "  $label: not found" >&2
        kill "$wrapper" 2>/dev/null || true
        wrapper=
        return
    fi

    local us_samples="" k_samples="" ev_samples="" pe_samples=""
    local c0 t0 n0 k0 c1 t1 n1 k1
    c0=$(cpu_ticks "$pid")
    read -r n0 k0 <<<"$(prog_stats)"
    t0=$(date +%s.%N)
    for _ in $(seq 1 "$WINDOWS"); do
        sleep "$INTERVAL"
        kill -0 "$pid" 2>/dev/null || break
        c1=$(cpu_ticks "$pid")
        read -r n1 k1 <<<"$(prog_stats)"
        t1=$(date +%s.%N)
        us_samples+=$(awk "BEGIN { printf \"%.2f\", (($c1 - $c0) / $TCK) / ($t1 - $t0) * 100 }")$'\n'
        if ((kernel == 1)); then
            k_samples+=$(awk "BEGIN { printf \"%.2f\", (($k1 - $k0) / 1e9) / ($t1 - $t0) * 100 }")$'\n'
            ev_samples+=$(awk "BEGIN { printf \"%.0f\", ($n1 - $n0) / ($t1 - $t0) }")$'\n'
            if ((n1 > n0)); then
                pe_samples+=$(awk "BEGIN { printf \"%.1f\", ($k1 - $k0) / ($n1 - $n0) }")$'\n'
            fi
        fi
        c0=$c1; k0=$k1; n0=$n1; t0=$t1
    done
    kill "$pid" "$wrapper" 2>/dev/null || true
    pid=
    wrapper=

    local us kern=0 ev=0 pe=0
    us=$(printf '%s' "$us_samples" | median)
    if ((kernel == 1)); then
        kern=$(printf '%s' "$k_samples" | median)
        ev=$(printf '%s' "$ev_samples" | median)
        pe=$(printf '%s' "$pe_samples" | median)
    fi
    awk "BEGIN { printf \"switch,%d,%s,%.2f,%.2f,%.2f,%d,%.1f\n\", $RATE, \"$label\", $us, $kern, $us + $kern, $ev, $pe }" >>"$OUT"
}

echo "scenario,ctxt_per_sec,tool,cpu_us,cpu_kernel,cpu_total,events_per_sec,ns_per_event" >"$OUT"
for w in "${WORKERS[@]}"; do
    stress-ng --switch "$w" --timeout 600 >/dev/null 2>&1 &
    stress_pid=$!
    sleep 2

    x0=$(ctxt); sleep 2; x1=$(ctxt)
    RATE=$(awk "BEGIN { printf \"%d\", ($x1 - $x0) / 2 }")
    echo "== $w workers, ~$RATE ctxt/s ==" >&2

    measure htop htop 0 pty htop -d 10
    measure btop btop 0 pty env "XDG_CONFIG_HOME=$BTOP_HOME" btop
    measure truetop truetop 1 pty "$TRUETOP"

    kill "$stress_pid" 2>/dev/null || true
    wait "$stress_pid" 2>/dev/null || true
    stress_pid=
done

echo "wrote $OUT" >&2
