//! Build truetop's artifacts and run them under sudo - eBPF needs root at load.

use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use cargo_metadata::{Message, camino::Utf8PathBuf};

use crate::privilege::as_root;

/// Build truetop and run the binary as root, forwarding any extra arguments.
pub fn run(extra: &[String]) -> Result<()> {
    let bin = build_bin()?;
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    run_as_root(bin.as_str(), &extra)
}

/// Build the integration tests and run each as root with `--ignored`. An optional
/// argument selects a single test target: `cargo xtask test cross_kernel`.
pub fn test(filter: &[String]) -> Result<()> {
    let mut args = vec!["test", "-p", "truetop"];
    if let Some(target) = filter.first() {
        args.extend(["--test", target.as_str()]);
    }
    args.push("--no-run");
    let bins = executables(&args, "test", None)?;
    if bins.is_empty() {
        bail!("no integration test binaries were built");
    }
    for bin in bins {
        run_as_root(bin.as_str(), &["--ignored", "--test-threads=1"])?;
    }
    Ok(())
}

fn build_bin() -> Result<Utf8PathBuf> {
    executables(
        &["build", "-p", "truetop", "--bin", "truetop"],
        "bin",
        Some("truetop"),
    )?
    .into_iter()
    .next()
    .context("truetop binary was not produced")
}

/// Run a cargo command and collect the executables it emits, filtered by target
/// kind (and optionally name).
fn executables(args: &[&str], kind: &str, name: Option<&str>) -> Result<Vec<Utf8PathBuf>> {
    let mut child = Command::new(cargo())
        .args(args)
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .spawn()
        .context("spawning cargo")?;

    let reader = std::io::BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut bins = Vec::new();
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
            bins.push(exe);
        }
    }

    if !child.wait().context("waiting for cargo")?.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    Ok(bins)
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
