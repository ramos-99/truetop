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

mod cpu_perf;
mod env;

use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};

use crate::{
    bench::{cpu_perf::CpuPerf, env::write_env},
    runner::cargo,
};

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
