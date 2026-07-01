//! Benchmark orchestration: one entry point for all three benchmarks so a run is
//! reproducible instead of a pile of ad-hoc commands.
//!
//!   cargo xtask bench                    run micro, macro and hotpath
//!   cargo xtask bench micro              run only the listed benchmarks
//!   cargo xtask bench macro hotpath      (any subset, in any order)
//!   cargo xtask bench --no-prep [...]    skip CPU tuning (VMs, CI)
//!
//! For stable numbers the run pins the governor to `performance`, disables turbo,
//! and pins the micro benchmark to one core. The original CPU settings are
//! restored on exit, including on error or panic, so the machine is never left
//! tuned.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};

use crate::runner::cargo;

const USAGE: &str = "usage: cargo xtask bench [--no-prep] [micro] [macro] [hotpath]";

#[derive(PartialEq)]
enum Bench {
    Micro,
    Macro,
    Hotpath,
}

pub fn bench(args: &[String]) -> Result<()> {
    let mut prep = true;
    let mut picks = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--no-prep" => prep = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "micro" => push_unique(&mut picks, Bench::Micro),
            "macro" => push_unique(&mut picks, Bench::Macro),
            "hotpath" => push_unique(&mut picks, Bench::Hotpath),
            other => bail!("unknown argument `{other}`\n{USAGE}"),
        }
    }
    if picks.is_empty() {
        picks = vec![Bench::Micro, Bench::Macro, Bench::Hotpath];
    }

    // Every benchmark writes here. Create it as the user first, before any sudo
    // step, so the scripts' root-run `mkdir -p` doesn't leave it root-owned.
    std::fs::create_dir_all(results_dir()).context("creating bench/results")?;

    // macro and hotpath run the release binaries; build once up front.
    if picks.iter().any(|b| *b != Bench::Micro) {
        run(Command::new(cargo()).args(["build", "--release"])).context("cargo build --release")?;
    }

    // Tune the CPU for the whole run; `_perf` restores it when this scope ends.
    let _perf = prep.then(CpuPerf::engage).flatten();

    // Record the machine and its tuned state alongside the numbers.
    write_env(&results_dir());

    for pick in picks {
        match pick {
            Bench::Micro => micro()?,
            Bench::Macro => macro_bench()?,
            Bench::Hotpath => hotpath()?,
        }
    }
    Ok(())
}

fn push_unique(picks: &mut Vec<Bench>, b: Bench) {
    if !picks.contains(&b) {
        picks.push(b);
    }
}

/// The workspace root, one level above this crate.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under the workspace root")
}

/// Where all three benchmarks write their output.
fn results_dir() -> std::path::PathBuf {
    root().join("bench/results")
}

/// micro: criterion, no root. Clean the baseline, point criterion's report at the
/// shared results dir, and pin to one core.
fn micro() -> Result<()> {
    eprintln!("== micro ==");
    let criterion = results_dir().join("criterion");
    let _ = std::fs::remove_dir_all(&criterion);

    let mut cmd = if have("taskset") {
        let mut c = Command::new("taskset");
        c.args(["-c", "1", &cargo()]);
        c
    } else {
        Command::new(cargo())
    };
    cmd.args(["bench", "-p", "truetop-bench"])
        .env("CRITERION_HOME", &criterion);
    run(&mut cmd).context("micro benchmark")
}

/// macro: strace vs top/htop, needs root, then redraw the plot.
fn macro_bench() -> Result<()> {
    eprintln!("== macro ==");
    sudo_script("bench/macro/run.sh")?;
    if run(&mut Command::new(root().join("bench/macro/plot.py"))).is_err() {
        eprintln!("macro: plot skipped (needs python + matplotlib)");
    }
    Ok(())
}

/// hotpath: bpftool run stats under hackbench, needs root.
fn hotpath() -> Result<()> {
    eprintln!("== hotpath ==");
    sudo_script("bench/hotpath/run.sh")
}

fn sudo_script(rel: &str) -> Result<()> {
    run(Command::new("sudo").arg("bash").arg(root().join(rel))).with_context(|| rel.to_owned())
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {cmd:?}"))?;
    if !status.success() {
        bail!("command failed: {cmd:?}");
    }
    Ok(())
}

fn have(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

const GOVERNOR0: &str = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
const GOVERNOR_GLOB: &str = "/sys/devices/system/cpu/cpu*/cpufreq/scaling_governor";
const TURBO_KNOBS: [(&str, &str); 2] = [
    ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
    ("/sys/devices/system/cpu/cpufreq/boost", "0"),
];

/// Best-effort CPU tuning that reverts on drop. Every field is `Some` only if we
/// changed it, so restore touches exactly what we set and nothing else.
struct CpuPerf {
    governor: Option<String>,
    turbo: Option<(&'static str, String)>,
}

impl CpuPerf {
    /// Returns `None` if nothing could be tuned (no knobs, or no privileges), so
    /// the run proceeds untuned rather than failing.
    fn engage() -> Option<Self> {
        let mut perf = Self {
            governor: None,
            turbo: None,
        };

        let mut any_knob = false;

        // Governor: read cpu0's current value, switch every CPU to performance.
        if let Some(orig) = read_trim(GOVERNOR0) {
            any_knob = true;
            if orig == "performance" {
                eprintln!("cpu: governor already performance");
            } else if write_glob(GOVERNOR_GLOB, "performance") {
                eprintln!("cpu: governor {orig} -> performance");
                perf.governor = Some(orig);
            } else {
                eprintln!("cpu: could not set governor (need sudo?)");
            }
        }

        // Turbo: whichever knob this kernel exposes.
        for (path, off) in TURBO_KNOBS {
            let Some(orig) = read_trim(path) else {
                continue;
            };
            any_knob = true;
            if orig == off {
                eprintln!("cpu: turbo already off");
            } else if write_one(path, off) {
                eprintln!("cpu: turbo off ({path}: {orig} -> {off})");
                perf.turbo = Some((path, orig));
            } else {
                eprintln!("cpu: could not disable turbo (need sudo?)");
            }
            break;
        }

        if !any_knob {
            eprintln!("cpu: no cpufreq knobs found; numbers may be noisy");
        }

        // Restore only runs for what we actually changed.
        if perf.governor.is_none() && perf.turbo.is_none() {
            return None;
        }
        Some(perf)
    }
}

impl Drop for CpuPerf {
    fn drop(&mut self) {
        if let Some(orig) = &self.governor {
            write_glob(GOVERNOR_GLOB, orig);
            eprintln!("cpu: governor restored to {orig}");
        }
        if let Some((path, orig)) = &self.turbo {
            write_one(path, orig);
            eprintln!("cpu: turbo restored ({path}: {orig})");
        }
    }
}

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
}

/// Write machine and run provenance next to the numbers. Best-effort: any field
/// that can't be read is simply omitted.
fn write_env(results: &Path) {
    let mut out = String::from("truetop benchmark environment\n\n");
    let mut line = |label: &str, value: String| {
        let value = value.trim();
        if !value.is_empty() {
            out.push_str(&format!("{label:<10} {value}\n"));
        }
    };

    line("date:", cmd_stdout("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]));
    line(
        "commit:",
        cmd_stdout("git", &["rev-parse", "--short", "HEAD"]),
    );
    line("kernel:", cmd_stdout("uname", &["-srmo"]));
    line("cpu:", cpu_model());
    line(
        "cores:",
        std::thread::available_parallelism()
            .map(|n| format!("{n} logical"))
            .unwrap_or_default(),
    );
    line("governor:", read_trim(GOVERNOR0).unwrap_or_default());

    if std::fs::write(results.join("env.txt"), out).is_err() {
        eprintln!("could not write env.txt");
    }
}

/// First `model name` from `/proc/cpuinfo`, or empty if unavailable.
fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim() == "model name"))
                .map(|(_, v)| v.trim().to_owned())
        })
        .unwrap_or_default()
}

/// Trimmed stdout of a command, or empty on any failure.
fn cmd_stdout(bin: &str, args: &[&str]) -> String {
    Command::new(bin)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// Write one sysfs file as root. Values here are our own constants, never input.
fn write_one(path: &str, val: &str) -> bool {
    sudo_sh(&format!("echo {val} > {path}"))
}

/// Write a value to every file matching a glob (the shell expands it; `tee`
/// fans out where a single `>` redirect cannot).
fn write_glob(glob: &str, val: &str) -> bool {
    sudo_sh(&format!("echo {val} | tee {glob} >/dev/null"))
}

fn sudo_sh(script: &str) -> bool {
    Command::new("sudo")
        .args(["sh", "-c", script])
        .status()
        .is_ok_and(|s| s.success())
}
