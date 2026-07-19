//! Benchmark orchestration: one entry point for every benchmark so a run is
//! reproducible instead of a pile of ad-hoc commands. Each benchmark is a
//! submodule with a `run`; this file only picks, tunes, and dispatches.
//!
//!   cargo xtask bench                    run micro, macro and hotpath
//!   cargo xtask bench micro              run only the listed benchmarks
//!   cargo xtask bench selfcpu            (any subset, in any order)
//!   cargo xtask bench --no-prep [...]    skip CPU tuning (VMs, CI)
//!
//! For stable numbers the run pins the governor to `performance`, disables turbo,
//! and pins the micro benchmark to one core. The original CPU settings are
//! restored on exit, including on error or panic, so the machine is never left
//! tuned.

mod cpu_perf;
mod env;
mod hotpath;
mod macro_bench;
mod micro;
mod selfcpu;
mod switch;

use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};

use crate::{
    bench::{cpu_perf::CpuPerf, env::write_env},
    runner::cargo,
};

const USAGE: &str =
    "usage: cargo xtask bench [--no-prep] [micro] [macro] [hotpath] [selfcpu] [switch]";

#[derive(PartialEq)]
enum Bench {
    Micro,
    Macro,
    Hotpath,
    Selfcpu,
    Switch,
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
            "selfcpu" => push_unique(&mut picks, Bench::Selfcpu),
            "switch" => push_unique(&mut picks, Bench::Switch),
            other => bail!("unknown argument `{other}`\n{USAGE}"),
        }
    }
    // selfcpu is opt-in: it is slow and needs btop/htop installed.
    if picks.is_empty() {
        picks = vec![Bench::Micro, Bench::Macro, Bench::Hotpath];
    }

    // Every benchmark writes here. Create it as the user first, before any sudo
    // step, so the scripts' root-run `mkdir -p` doesn't leave it root-owned.
    std::fs::create_dir_all(results_dir()).context("creating bench/results")?;

    // Every benchmark but micro runs the release binaries; build once up front.
    if picks.iter().any(|b| *b != Bench::Micro) {
        exec(Command::new(cargo()).args(["build", "--release"]))
            .context("cargo build --release")?;
    }

    // Tune the CPU for the whole run; `_perf` restores it when this scope ends.
    let _perf = prep.then(CpuPerf::engage).flatten();

    // Record the machine and its tuned state alongside the numbers.
    write_env(&results_dir());

    for pick in picks {
        match pick {
            Bench::Micro => micro::run()?,
            Bench::Macro => macro_bench::run()?,
            Bench::Hotpath => hotpath::run()?,
            Bench::Selfcpu => selfcpu::run()?,
            Bench::Switch => switch::run()?,
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
pub(super) fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under the workspace root")
}

/// Where every benchmark writes its output.
pub(super) fn results_dir() -> std::path::PathBuf {
    root().join("bench/results")
}

/// Run a benchmark's root script (eBPF and strace need it), elevating if needed.
pub(super) fn root_script(rel: &str) -> Result<()> {
    exec(crate::privilege::as_root("bash").arg(root().join(rel))).with_context(|| rel.to_owned())
}

pub(super) fn exec(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("spawning {cmd:?}"))?;
    if !status.success() {
        bail!("command failed: {cmd:?}");
    }
    Ok(())
}

pub(super) fn have(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
