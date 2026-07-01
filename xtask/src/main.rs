//! Workspace task runner. eBPF needs root at load time, so privileged runs go
//! through here instead of a global cargo runner that would also force
//! `cargo test` under sudo.
//!
//!   cargo xtask run [args...]         build truetop and run it as root
//!   cargo xtask test                  run the eBPF integration tests as root
//!   cargo xtask bench [names...]      run the benchmarks (all, or a subset)
//!   cargo xtask update-btf-fixtures   refresh the BTF test fixtures

mod bench;
mod btf_fixtures;
mod runner;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => runner::run(&args[1..]),
        Some("test") => runner::test(),
        Some("bench") => bench::bench(&args[1..]),
        Some("update-btf-fixtures") => btf_fixtures::update(),
        _ => bail!("usage: cargo xtask <run | test | bench | update-btf-fixtures> [args...]"),
    }
}
