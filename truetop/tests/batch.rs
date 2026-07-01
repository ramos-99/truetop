//! Integration: `BatchReader::sum_per_cpu` against a per-CPU map populated with
//! known values, so the per-CPU stride and the *possible*-CPU count are checked
//! against the kernel, not just the dev machine's live data. Run: `cargo xtask
//! test`.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use anyhow::{Result, bail};
use aya::util::nr_cpus;
use truetop::BatchReader;

const MAP_CREATE: libc::c_long = 0; // BPF_MAP_CREATE
const MAP_UPDATE_ELEM: libc::c_long = 2; // BPF_MAP_UPDATE_ELEM
const PERCPU_HASH: u32 = 5; // BPF_MAP_TYPE_PERCPU_HASH

#[repr(C)]
#[derive(Default)]
struct MapCreateAttr {
    map_type: u32,
    key_size: u32,
    value_size: u32,
    max_entries: u32,
    map_flags: u32,
    inner_map_fd: u32,
    numa_node: u32,
    map_name: [u8; 16],
}

#[repr(C)]
#[derive(Default)]
struct MapElemAttr {
    map_fd: u32,
    _pad: u32,
    key: u64,
    value: u64,
    flags: u64,
}

fn bpf<T>(cmd: libc::c_long, attr: &mut T) -> i64 {
    // SAFETY: `attr` is a zero-initialised bpf_attr variant matching `cmd`, and
    // its size is passed so the kernel reads exactly the fields we set.
    unsafe { libc::syscall(libc::SYS_bpf, cmd, attr as *mut T, size_of::<T>()) }
}

fn create_percpu_hash() -> Result<OwnedFd> {
    let mut attr = MapCreateAttr {
        map_type: PERCPU_HASH,
        key_size: 4,
        value_size: 8,
        max_entries: 8,
        ..Default::default()
    };
    let fd = bpf(MAP_CREATE, &mut attr);
    if fd < 0 {
        bail!("BPF_MAP_CREATE: {}", std::io::Error::last_os_error());
    }
    // SAFETY: BPF_MAP_CREATE returned a fresh, owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

/// `per_cpu` must hold one value per *possible* CPU - the layout the kernel reads
/// for a per-CPU map update.
fn update_percpu(map: &OwnedFd, key: u32, per_cpu: &[u64]) -> Result<()> {
    let mut attr = MapElemAttr {
        map_fd: map.as_raw_fd() as u32,
        key: &raw const key as u64,
        value: per_cpu.as_ptr() as u64,
        ..Default::default()
    };
    if bpf(MAP_UPDATE_ELEM, &mut attr) != 0 {
        bail!("BPF_MAP_UPDATE_ELEM: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

#[test]
#[ignore = "needs root + a live kernel; run: cargo xtask test"]
fn sum_per_cpu_folds_known_values() -> Result<()> {
    let ncpus = nr_cpus().map_err(|(s, e)| anyhow::anyhow!("{s}: {e}"))?;

    let map = create_percpu_hash()?;
    let key: u32 = 4242;
    let per_cpu: Vec<u64> = (1..=ncpus as u64).collect();
    update_percpu(&map, key, &per_cpu)?;

    let totals = BatchReader::new(ncpus).sum_per_cpu(map.as_raw_fd());

    let expected: u64 = (1..=ncpus as u64).sum();
    assert_eq!(totals.get(&key), Some(&expected));
    Ok(())
}
