#!/usr/bin/env bash
# Shared helpers for the bench/*/run.sh scripts. Source, don't execute:
#   source "$(dirname "$0")/../lib.sh"

require() {
    for c in "$@"; do
        command -v "$c" >/dev/null || { echo "missing dependency: $c" >&2; exit 1; }
    done
}

require_built() {
    for b in "$@"; do
        [[ -x $b ]] || { echo "build first: cargo build --release" >&2; exit 1; }
    done
}

require_root() {
    [[ $EUID -eq 0 ]] || { echo "run as root (truetop loads eBPF): sudo $0" >&2; exit 1; }
}

# warn_if_not_performance <xtask-bench-name>
warn_if_not_performance() {
    local gov
    gov=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || true)
    [[ ${gov:-} == performance ]] \
        || echo "warning: governor '${gov:-unknown}' != performance; run via 'cargo xtask bench $1' for stable numbers" >&2
}

# utime + stime (ticks) of a pid. `stat` fields 14,15 sit at $12,$13 once the
# parenthesised comm is dropped.
cpu_ticks() {
    local rem
    rem=$(cut -d')' -f2- "/proc/$1/stat" 2>/dev/null) || { echo 0; return; }
    awk '{print $12 + $13}' <<<"$rem"
}

# run_cnt and run_time_ns for the sched_switch program; "0 0" if not loaded yet.
prog_stats() {
    bpftool prog show --json 2>/dev/null \
        | jq -r 'first(.[] | select(.name == "sched_switch"))
                 | "\(.run_cnt // 0) \(.run_time_ns // 0)"' 2>/dev/null \
        || echo "0 0"
}

# Median, IQR and range of stdin numbers (one per line), dropping the first
# $WARMUP as ramp-up. Prints "n med p25 p75 lo hi"; "0 0 0 0 0 0" if empty.
iqr_stats() {
    awk -v warm="${WARMUP:-0}" '
        { v[NR] = $1 }
        END {
            n = 0
            for (i = warm + 1; i <= NR; i++) a[++n] = v[i]
            if (n == 0) { print "0 0 0 0 0 0"; exit }
            for (i = 1; i <= n; i++)
                for (j = i + 1; j <= n; j++)
                    if (a[j] < a[i]) { t = a[i]; a[i] = a[j]; a[j] = t }
            med = (n % 2) ? a[(n + 1) / 2] : (a[n / 2] + a[n / 2 + 1]) / 2
            printf "%d %.1f %.1f %.1f %.1f %.1f", n, med, a[int(n * 0.25) + 1], a[int(n * 0.75) + 1], a[1], a[n]
        }'
}

# Writes a throwaway btop config with a 1s refresh (its own default varies) and
# echoes the XDG_CONFIG_HOME to pass it: `env "XDG_CONFIG_HOME=$(btop_home)" btop`.
# The caller owns cleanup (rm -rf) since only it knows its trap.
btop_home() {
    local home
    home=$(mktemp -d)
    mkdir -p "$home/btop"
    printf 'update_ms = 1000\n' >"$home/btop/btop.conf"
    echo "$home"
}
