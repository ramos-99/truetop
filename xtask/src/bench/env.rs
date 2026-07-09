//! Machine and run provenance, written next to the numbers.

use std::{path::Path, process::Command};

use super::cpu_perf::current_governor;

/// Write machine and run provenance next to the numbers. Best-effort: any field
/// that can't be read is simply omitted.
pub fn write_env(results: &Path) {
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
    line("governor:", current_governor().unwrap_or_default());

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
