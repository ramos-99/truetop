//! Rebuild this crate when `bpf-linker` changes. The object cannot link without
//! it, but it is a binary, not a crate, so cargo has no way to depend on it -
//! artifact dependencies would say this properly, and rust-lang/cargo#12385
//! keeps them impractical. The resolved path's mtime stands in as the cache key.
//!
//! Imperfect by construction: a different `bpf-linker` appearing earlier in
//! `$PATH` goes unnoticed. Catching that means invalidating on `$PATH` and on
//! every directory in it, which costs more rebuilds than the case is worth.

use which::which;

fn main() {
    let bpf_linker =
        which("bpf-linker").expect("bpf-linker not on $PATH; `cargo install bpf-linker`");
    println!("cargo:rerun-if-changed={}", bpf_linker.display());
}
