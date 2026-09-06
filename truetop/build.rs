//! Compiles `truetop-ebpf` to a BPF object for `include_bytes_aligned!` to pull
//! into the loader. Cargo cannot express this as an ordinary dependency - the
//! eBPF crate builds for its own target with `-Z build-std` - so this runs a
//! nested cargo instead (see the note in Cargo.toml).
//!
//! The toolchain is the dated nightly pinned in `truetop-ebpf/Cargo.toml`;
//! `TRUETOP_EBPF_TOOLCHAIN` replaces it. `-Z build-std` is why that pin is a
//! nightly: on stable it needs [`RUSTC_BOOTSTRAP`], which
//! `aya_build::build_ebpf` reads and this does not set.
//!
//! [`RUSTC_BOOTSTRAP`]: https://doc.rust-lang.org/beta/unstable-book/compiler-environment-variables/RUSTC_BOOTSTRAP.html

use anyhow::{Context as _, Result, anyhow};
use aya_build::{Package, Toolchain};

/// Names a toolchain to build the eBPF object with, in place of the pin.
const TOOLCHAIN_VAR: &str = "TRUETOP_EBPF_TOOLCHAIN";

fn main() -> Result<()> {
    println!("cargo::rerun-if-env-changed={TOOLCHAIN_VAR}");

    let metadata = cargo_metadata::MetadataCommand::new()
        .no_deps()
        .exec()
        .context("reading cargo metadata")?;
    let ebpf = metadata
        .packages
        .iter()
        .find(|package| package.name.as_str() == "truetop-ebpf")
        .context("truetop-ebpf is not a member of this workspace")?;
    let root_dir = ebpf
        .manifest_path
        .parent()
        .ok_or_else(|| anyhow!("no parent directory for {}", ebpf.manifest_path))?;

    let pinned = ebpf.metadata["ebpf"]["toolchain"]
        .as_str()
        .context("truetop-ebpf: [package.metadata.ebpf] toolchain is not set")?;

    let toolchain = std::env::var(TOOLCHAIN_VAR).unwrap_or_else(|_| pinned.to_owned());
    if toolchain != pinned {
        println!(
            "cargo::warning=building the eBPF object with {toolchain}, not the pinned {pinned}"
        );
    }

    aya_build::build_ebpf(
        [Package {
            name: ebpf.name.as_str(),
            root_dir: root_dir.as_str(),
            ..Default::default()
        }],
        Toolchain::Custom(&toolchain),
    )
}
