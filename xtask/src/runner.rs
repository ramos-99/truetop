//! Build truetop's artifacts and run them under sudo - eBPF needs root at load.

use std::{
    io::{BufRead as _, BufReader},
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};
use cargo_metadata::{Message, camino::Utf8PathBuf};

use crate::{privilege::as_root, style::Paint};

/// An executable cargo produced, and the target it came from.
struct Artifact {
    target: String,
    path: Utf8PathBuf,
}

/// Build truetop and run the binary as root, forwarding any extra arguments.
pub fn run(extra: &[String]) -> Result<()> {
    let bin = build_bin()?;
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    run_as_root(bin.as_str(), &extra)
}

/// Build the integration tests and run each as root with `--ignored`. An optional
/// argument selects a single test target: `cargo xtask test cross_kernel`.
///
/// Cargo gives one binary per test file, each printing its own summary, so the
/// run announces which target it is in and closes with a total.
pub fn test(filter: &[String]) -> Result<()> {
    let mut args = vec!["test", "-p", "truetop"];
    if let Some(target) = filter.first() {
        args.extend(["--test", target.as_str()]);
    }
    args.push("--no-run");
    let suites = executables(&args, "test", None)?;
    if suites.is_empty() {
        bail!("no integration test binaries were built");
    }

    let paint = Paint::detect();
    let mut passed = 0;
    for suite in &suites {
        println!("\n{}", paint.bold(&format!("── {} ──", suite.target)));
        passed += run_suite(suite, &paint)?;
    }
    let total = format!("{passed} tests passed across {} targets", suites.len());
    println!("\n{}", paint.green(&total));
    Ok(())
}

/// Run one test binary as root, echoing its output line by line so it still
/// streams, and counting the tests its summaries report. Piping costs libtest
/// its terminal, hence `--color`: without it every `ok` and `FAILED` arrives
/// plain.
fn run_suite(suite: &Artifact, paint: &Paint) -> Result<usize> {
    let mut child = as_root(suite.path.as_str())
        .args([
            "--ignored",
            "--test-threads=1",
            "--color",
            paint.child_flag(),
        ])
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning {}", suite.target))?;

    let reader = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut passed = 0;
    for line in reader.lines() {
        let line = line.with_context(|| format!("reading {} output", suite.target))?;
        if let Some(summary) = line.strip_prefix("test result:") {
            passed += tests_passed(summary);
        }
        println!("{line}");
    }

    if !child
        .wait()
        .with_context(|| format!("waiting for {}", suite.target))?
        .success()
    {
        bail!("{} failed", suite.target);
    }
    Ok(passed)
}

/// Tests passed, from a libtest summary: `test result: ok. 3 passed; 0 failed`.
/// Zero if the line does not parse - the exit status decides pass or fail, this
/// only counts.
fn tests_passed(summary: &str) -> usize {
    let words: Vec<&str> = summary.split_whitespace().collect();
    words
        .iter()
        .position(|word| word.trim_end_matches(';') == "passed")
        .and_then(|at| words.get(at.checked_sub(1)?))
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

fn build_bin() -> Result<Utf8PathBuf> {
    executables(
        &["build", "-p", "truetop", "--bin", "truetop"],
        "bin",
        Some("truetop"),
    )?
    .into_iter()
    .next()
    .map(|artifact| artifact.path)
    .context("truetop binary was not produced")
}

/// Run a cargo command and collect the executables it emits, filtered by target
/// kind (and optionally name).
fn executables(args: &[&str], kind: &str, name: Option<&str>) -> Result<Vec<Artifact>> {
    let mut child = Command::new(cargo())
        .args(args)
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .spawn()
        .context("spawning cargo")?;

    let reader = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut artifacts = Vec::new();
    for message in Message::parse_stream(reader) {
        let Message::CompilerArtifact(artifact) = message.context("reading cargo output")? else {
            continue;
        };
        let kind_ok = artifact.target.kind.iter().any(|k| k.to_string() == kind);
        let name_ok = name.is_none_or(|n| artifact.target.name == n);
        if kind_ok
            && name_ok
            && let Some(exe) = artifact.executable
        {
            artifacts.push(Artifact {
                target: artifact.target.name.to_string(),
                path: exe,
            });
        }
    }

    if !child.wait().context("waiting for cargo")?.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    Ok(artifacts)
}

fn run_as_root(bin: &str, args: &[&str]) -> Result<()> {
    let status = as_root(bin)
        .args(args)
        .status()
        .context("spawning test binary")?;
    if !status.success() {
        bail!("{bin} exited unsuccessfully");
    }
    Ok(())
}

pub(crate) fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}
