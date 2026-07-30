//! Raw BPF map syscalls the typed aya API does not cover, addressed by file
//! descriptor. `BPF_MAP_LOOKUP_BATCH` reads a per-CPU `u32 → u64` map in O(1)
//! syscalls instead of `Map::iter`'s O(N) `get_next_key` + lookup (see
//! bench/BENCHMARKS.md); `BPF_MAP_DELETE_ELEM` reaps a departed process's entry.
//! Both need the raw fd, which only `MapData` exposes - and holding `MapData`,
//! rather than a typed map, is what forecloses aya's typed `remove` here.

use std::{
    collections::HashMap,
    io,
    os::fd::{AsFd, AsRawFd, RawFd},
};

use aya::maps::MapData;

use crate::metrics::Snapshot;

// uapi/linux/bpf.h `bpf_cmd`.
const DELETE_ELEM: libc::c_long = 3;
const LOOKUP_BATCH: libc::c_long = 24;
const BATCH: usize = 4096;

/// Delete `key` from a `u32`-keyed map through its fd; on a per-CPU map this
/// clears every CPU's slot at once. Reaps a departed process once the tick has
/// read its final totals (see `reaper`).
pub fn delete_elem(fd: RawFd, key: u32) {
    let attr = ElemAttr {
        map_fd: fd as u32,
        key: &raw const key as u64,
        ..Default::default()
    };
    // SAFETY: a valid bpf_attr for DELETE_ELEM - a live map fd and a pointer to a
    // u32 key that outlives the call.
    unsafe {
        libc::syscall(
            libc::SYS_bpf,
            DELETE_ELEM,
            &raw const attr,
            size_of::<ElemAttr>(),
        );
    }
}

/// The single-element variant of `union bpf_attr` (uapi/linux/bpf.h); `key` is
/// 8-byte aligned, hence the pad after the 32-bit fd.
#[repr(C)]
#[derive(Default)]
struct ElemAttr {
    map_fd: u32,
    _pad: u32,
    key: u64,
    value: u64,
    flags: u64,
}

/// Reusable scratch, so draining the maps costs no allocation per tick.
pub struct BatchReader {
    nr_cpus: usize,
    keys: Box<[u32]>,
    values: Box<[u64]>,
}

impl BatchReader {
    /// Entries per `BPF_MAP_LOOKUP_BATCH` call. Public so the integration test
    /// sizes a map from the real boundary rather than a literal that would
    /// quietly stop straddling it.
    pub const BATCH: usize = BATCH;

    /// `nr_cpus` must be the *possible* CPU count (`CpuCounts::possible`): the
    /// kernel writes one value slot per possible CPU per key.
    pub fn new(nr_cpus: usize) -> Self {
        Self {
            nr_cpus,
            keys: vec![0; BATCH].into_boxed_slice(),
            values: vec![0; BATCH * nr_cpus].into_boxed_slice(),
        }
    }

    /// Read every entry, summing each key's per-CPU slots into `tgid → ns`.
    pub fn sum_per_cpu(&mut self, fd: RawFd) -> HashMap<u32, u64> {
        let mut totals = HashMap::new();
        let mut cursor = Cursor::default();
        while let Some((count, exhausted)) = self.next_batch(fd, &mut cursor) {
            fold(&self.keys, &self.values, count, self.nr_cpus, &mut totals);
            // A zero-entry batch that is not the tail would spin.
            if exhausted || count == 0 {
                break;
            }
        }
        totals
    }

    /// A per-CPU counter map, summed and timestamped as a [`Snapshot`]; the
    /// caller diffs it against the previous one for a per-interval rate. The
    /// shared read path for every metric backed by one of these maps (CPU,
    /// I/O wait).
    pub(crate) fn read_counter(&mut self, map: &MapData) -> Snapshot {
        Snapshot::new(self.sum_per_cpu(map.fd().as_fd().as_raw_fd()))
    }

    /// One batch call: the entries written, and whether the kernel reported the
    /// map exhausted. `None` if the call failed, leaving the buffers meaningless.
    ///
    /// A short batch is *not* exhaustion. The kernel copies whole buckets and
    /// stops when the next one will not fit the buffer, leaving the cursor on it;
    /// only `ENOENT` ends the walk.
    fn next_batch(&mut self, fd: RawFd, cursor: &mut Cursor) -> Option<(usize, bool)> {
        let mut attr = BatchAttr::new(fd, cursor, &mut self.keys, &mut self.values);
        // SAFETY: the key/value buffers are sized BATCH × nr_cpus, exactly what
        // the kernel writes for this per-CPU map; `attr` is a valid bpf_attr.
        let ret = unsafe {
            libc::syscall(
                libc::SYS_bpf,
                LOOKUP_BATCH,
                &raw mut attr,
                size_of::<BatchAttr>(),
            )
        };
        cursor.started = true;

        if ret == 0 {
            return Some((attr.count as usize, false));
        }
        // The ENOENT tail still carries entries; any other error leaves `count`
        // meaningless.
        (io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT))
            .then_some((attr.count as usize, true))
    }
}

/// Sum each key's per-CPU slots into `totals`. `values` is row-major: `nr_cpus`
/// contiguous slots per key; only the first `count` keys are live.
fn fold(
    keys: &[u32],
    values: &[u64],
    count: usize,
    nr_cpus: usize,
    totals: &mut HashMap<u32, u64>,
) {
    for i in 0..count {
        let slots = &values[i * nr_cpus..(i + 1) * nr_cpus];
        totals.insert(keys[i], slots.iter().sum());
    }
}

/// The opaque batch token: NULL on the first call, then the kernel's cursor.
#[derive(Default)]
struct Cursor {
    token: u64,
    started: bool,
}

impl Cursor {
    fn in_ptr(&self) -> u64 {
        if self.started {
            &raw const self.token as u64
        } else {
            0
        }
    }

    fn out_ptr(&mut self) -> u64 {
        &raw mut self.token as u64
    }
}

/// The `batch` variant of `union bpf_attr` (uapi/linux/bpf.h).
#[repr(C)]
#[derive(Default)]
struct BatchAttr {
    in_batch: u64,
    out_batch: u64,
    keys: u64,
    values: u64,
    count: u32,
    map_fd: u32,
    elem_flags: u64,
    flags: u64,
}

impl BatchAttr {
    fn new(fd: RawFd, cursor: &mut Cursor, keys: &mut [u32], values: &mut [u64]) -> Self {
        Self {
            in_batch: cursor.in_ptr(),
            out_batch: cursor.out_ptr(),
            keys: keys.as_mut_ptr() as u64,
            values: values.as_mut_ptr() as u64,
            count: BATCH as u32,
            map_fd: fd as u32,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_sums_each_key_across_cpus() {
        let mut totals = HashMap::new();
        fold(&[10, 20], &[1, 2, 3, 4, 5, 6], 2, 3, &mut totals);
        assert_eq!(totals, HashMap::from([(10, 6), (20, 15)]));
    }

    #[test]
    fn fold_handles_a_single_cpu() {
        let mut totals = HashMap::new();
        fold(&[7], &[42], 1, 1, &mut totals);
        assert_eq!(totals, HashMap::from([(7, 42)]));
    }

    #[test]
    fn fold_ignores_entries_past_count() {
        // Scratch buffers outlive a batch; slots beyond `count` are stale.
        let mut totals = HashMap::new();
        fold(&[1, 2, 99], &[10, 20, 7777], 2, 1, &mut totals);
        assert_eq!(totals, HashMap::from([(1, 10), (2, 20)]));
    }

    #[test]
    fn fold_of_an_empty_batch_is_empty() {
        let mut totals = HashMap::new();
        fold(&[0; 4], &[0; 4], 0, 1, &mut totals);
        assert!(totals.is_empty());
    }
}
