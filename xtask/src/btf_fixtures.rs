//! Refresh the verbatim BTF fixtures used by `btf::tests` from BTFHub. Pin each
//! to a commit and record its sha256; see truetop/testdata/PROVENANCE.md. Leave
//! the sha empty for a first fetch — it is printed for you to record.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

// (file name, BTFHub raw URL pinned to a commit, sha256 of the file).
const BTFHUB: &str = "https://raw.githubusercontent.com/aquasecurity/btfhub-archive/f5eaeacd47ab8924bbe554bdfaaef796ada09016";
const FIXTURES: &[(&str, &str, &str)] = &[
    (
        "ubuntu-x86_64.btf.tar.xz",
        "ubuntu/20.04/x86_64/5.4.0-26-generic.btf.tar.xz",
        "f5435aba2a2f85c289dec4c52f7f3f01f7139f3143561e9c1b02ced174514b9d",
    ),
    (
        "ubuntu-arm64.btf.tar.xz",
        "ubuntu/20.04/arm64/5.4.0-26-generic.btf.tar.xz",
        "41822465e93f9e2b206b6fb08fedf8cb0347f1c2ac03c41a770ac4ff3bdbda6e",
    ),
    (
        "centos7-x86_64.btf.tar.xz",
        "centos/7/x86_64/3.10.0-1062.1.1.el7.x86_64.btf.tar.xz",
        "f5caa317e64ee89bab59744ad94b827ef99f53b1216c91446b0293327e3cab80",
    ),
];

/// Download the pinned fixtures, verify their checksums, and write them verbatim
/// to `truetop/testdata`. Run from the workspace root.
pub fn update() -> Result<()> {
    if FIXTURES.is_empty() {
        bail!("no fixtures pinned; see truetop/testdata/PROVENANCE.md");
    }
    let dir = Path::new("truetop/testdata");
    std::fs::create_dir_all(dir)?;
    for &(name, path, sha256) in FIXTURES {
        let bytes = ureq::get(format!("{BTFHUB}/{path}"))
            .call()
            .with_context(|| format!("fetching {name}"))?
            .into_body()
            .read_to_vec()?;
        let sha = hex(Sha256::digest(&bytes));
        if sha256.is_empty() {
            println!("{name}: {} bytes, sha256 = {sha} (record it)", bytes.len());
        } else if sha != sha256 {
            bail!("{name}: sha256 mismatch\n  expected {sha256}\n  got      {sha}");
        } else {
            println!("{name}: {} bytes, sha256 ok", bytes.len());
        }
        std::fs::write(dir.join(name), &bytes)?;
    }
    Ok(())
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
