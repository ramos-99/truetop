//! Compiles `truetop-ebpf` to a BPF object for `include_bytes_aligned!` to pull
//! into the loader. Cargo cannot express this as an ordinary dependency - the
//! eBPF crate builds for its own target with `-Z build-std` - so this runs a
//! nested cargo instead (see the note in Cargo.toml).

use anyhow::{Context as _, Result, anyhow};
use aya_build::{Package, Toolchain};

fn main() -> Result<()> {
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

    let toolchain = ebpf.metadata["ebpf"]["toolchain"]
        .as_str()
        .context("truetop-ebpf: [package.metadata.ebpf] toolchain is not set")?;

    aya_build::build_ebpf(
        [Package {
            name: ebpf.name.as_str(),
            root_dir: root_dir.as_str(),
            ..Default::default()
        }],
        Toolchain::Custom(toolchain),
    )
}
