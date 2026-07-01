#!/usr/bin/env python3
"""Plot per-process data syscalls per refresh vs process count (macro.csv)."""

import csv
from collections import defaultdict
from pathlib import Path

import matplotlib.pyplot as plt

# All benchmark outputs live in bench/results/, regardless of the caller's cwd.
RESULTS = Path(__file__).resolve().parent.parent / "results"

series = defaultdict(list)
with open(RESULTS / "macro.csv") as f:
    for row in csv.DictReader(f):
        series[row["tool"]].append(
            (int(row["procs"]), int(row["syscalls_per_refresh"]))
        )

fig, ax = plt.subplots(figsize=(8, 5))
for tool in ("top", "htop", "truetop"):
    points = sorted(series.get(tool, []))
    if points:
        xs, ys = zip(*points)
        ax.plot(xs, ys, marker="o", label=tool)

ax.set(
    xscale="log",
    yscale="log",
    xlabel="processes",
    ylabel="per-process data syscalls per refresh",
    title="CPU collection cost vs process count",
)
ax.grid(True, which="both", linestyle=":", alpha=0.4)
ax.legend()
fig.tight_layout()
out = RESULTS / "scaling.svg"
fig.savefig(out)
print(f"wrote {out}")
