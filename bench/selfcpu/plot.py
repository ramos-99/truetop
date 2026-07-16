#!/usr/bin/env python3
"""Plot each monitor's CPU% and RSS vs process count (selfcpu.csv)."""

import csv
from collections import defaultdict
from pathlib import Path

import matplotlib.pyplot as plt

RESULTS = Path(__file__).resolve().parent.parent / "results"

rows = defaultdict(list)
with open(RESULTS / "selfcpu.csv") as f:
    for r in csv.DictReader(f):
        rows[r["tool"]].append(
            (
                int(r["procs"]),
                float(r["cpu_median"]),
                float(r["cpu_p25"]),
                float(r["cpu_p75"]),
                float(r["rss_mb"]),
            )
        )

# htop/btop each get a colour; the two truetop lines share one, solid vs dashed.
style = {
    "htop": dict(color="C1", linestyle="-"),
    "btop": dict(color="C3", linestyle="-"),
    "truetop-ui": dict(color="C2", linestyle="-"),
    "truetop-collector": dict(color="C2", linestyle="--"),
}


def save(fig, ax, ylabel, name):
    ax.set(xlabel="processes", ylabel=ylabel)
    ax.grid(True, linestyle=":", alpha=0.4)
    ax.legend()
    fig.tight_layout()
    out = RESULTS / name
    fig.savefig(out)
    print(f"wrote {out}")


fig, ax = plt.subplots(figsize=(8, 5))
for tool, st in style.items():
    pts = sorted(rows.get(tool, []))
    if not pts:
        continue
    xs = [p[0] for p in pts]
    ax.plot(xs, [p[1] for p in pts], marker="o", label=tool, **st)
    ax.fill_between(xs, [p[2] for p in pts], [p[3] for p in pts], color=st["color"], alpha=0.12)
save(fig, ax, "CPU % of one core", "selfcpu-cpu.svg")

fig, ax = plt.subplots(figsize=(8, 4))
for tool, st in style.items():
    pts = sorted(rows.get(tool, []))
    if not pts:
        continue
    ax.plot([p[0] for p in pts], [p[4] for p in pts], marker="o", label=tool, **st)
save(fig, ax, "RSS (MiB)", "selfcpu-rss.svg")
