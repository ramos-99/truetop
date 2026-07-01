//! Minimal BTF reader: resolves a struct field's byte offset against the
//! running kernel's BTF (`/sys/kernel/btf/vmlinux`).
//!
//! This is our portable stand-in for libbpf CO-RE field relocations, which the
//! Rust eBPF toolchain doesn't emit. The offset is read from the live kernel
//! and injected into the program at load (see `main`), so one binary adapts to
//! any kernel version/arch - each machine ships the BTF for its own layout.

use anyhow::{Context as _, Result, bail};

const VMLINUX_BTF: &str = "/sys/kernel/btf/vmlinux";

// BTF kind tags (uapi/linux/btf.h).
const KIND_INT: u32 = 1;
const KIND_ARRAY: u32 = 3;
const KIND_STRUCT: u32 = 4;
const KIND_UNION: u32 = 5;
const KIND_ENUM: u32 = 6;
const KIND_FUNC_PROTO: u32 = 13;
const KIND_VAR: u32 = 14;
const KIND_DATASEC: u32 = 15;
const KIND_DECL_TAG: u32 = 17;
const KIND_ENUM64: u32 = 19;

/// Resolve the byte offset of `field` within `struct_name` from the running
/// kernel's BTF.
pub fn field_byte_offset(struct_name: &str, field: &str) -> Result<u32> {
    Btf::load()?.field_offset(struct_name, field)
}

struct Btf {
    data: Vec<u8>,
    le: bool,
    type_off: usize,
    type_len: usize,
    str_off: usize,
}

impl Btf {
    fn load() -> Result<Self> {
        let data = std::fs::read(VMLINUX_BTF)
            .with_context(|| format!("reading {VMLINUX_BTF} (CONFIG_DEBUG_INFO_BTF required)"))?;
        Self::from_bytes(data).with_context(|| format!("parsing {VMLINUX_BTF}"))
    }

    fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.len() < 24 {
            bail!("too small to be valid BTF");
        }
        // Detect endianness from the magic (0xeb9f).
        let le = match u16::from_le_bytes([data[0], data[1]]) {
            0xeb9f => true,
            _ if u16::from_be_bytes([data[0], data[1]]) == 0xeb9f => false,
            _ => bail!("bad BTF magic"),
        };
        let mut btf = Self {
            data,
            le,
            type_off: 0,
            type_len: 0,
            str_off: 0,
        };
        // btf_header: section offsets are relative to hdr_len.
        let hdr_len = btf.u32(4).context("truncated BTF header")? as usize;
        btf.type_off = hdr_len + btf.u32(8).context("truncated BTF header")? as usize;
        btf.type_len = btf.u32(12).context("truncated BTF header")? as usize;
        btf.str_off = hdr_len + btf.u32(16).context("truncated BTF header")? as usize;
        Ok(btf)
    }

    /// Read a `u32`, or `None` past the buffer - every read is bounds-checked
    /// because the section bounds come from an untrusted header.
    fn u32(&self, at: usize) -> Option<u32> {
        let b = self.data.get(at..at + 4)?;
        let b = [b[0], b[1], b[2], b[3]];
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn string(&self, name_off: u32) -> &str {
        let Some(rest) = self.data.get(self.str_off + name_off as usize..) else {
            return "";
        };
        let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
        std::str::from_utf8(&rest[..end]).unwrap_or("")
    }

    fn field_offset(&self, struct_name: &str, field: &str) -> Result<u32> {
        // Clamp to the buffer: the section length comes from an untrusted header.
        let end = (self.type_off + self.type_len).min(self.data.len());
        let mut cur = self.type_off; // type id 0 is void; the section starts at id 1.
        while cur + 12 <= end {
            let Some(info) = self.u32(cur + 4) else { break };
            let vlen = (info & 0xffff) as usize;
            let kind = (info >> 24) & 0x1f;
            let kflag = (info >> 31) & 1;
            let members = cur + 12;

            if kind == KIND_STRUCT && self.u32(cur).is_some_and(|n| self.string(n) == struct_name) {
                for i in 0..vlen {
                    let m = members + i * 12; // btf_member: name_off, type, offset
                    let (Some(name), Some(bits)) = (self.u32(m), self.u32(m + 8)) else {
                        continue;
                    };
                    if self.string(name) == field {
                        // For bitfield-bearing structs (kflag) the low 24 bits
                        // are the bit offset; otherwise the whole word is.
                        let bit_off = if kflag == 1 { bits & 0x00ff_ffff } else { bits };
                        return Ok(bit_off / 8);
                    }
                }
                bail!("field `{field}` not found in `{struct_name}`");
            }
            cur = members + extra_len(kind, vlen);
        }
        bail!("struct `{struct_name}` not found in BTF");
    }
}

/// Bytes of per-kind trailing data after the 12-byte `btf_type` header.
fn extra_len(kind: u32, vlen: usize) -> usize {
    match kind {
        KIND_INT | KIND_VAR | KIND_DECL_TAG => 4,
        KIND_ARRAY => 12,
        KIND_STRUCT | KIND_UNION | KIND_DATASEC | KIND_ENUM64 => 12 * vlen,
        KIND_ENUM | KIND_FUNC_PROTO => 8 * vlen,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal BTF (`le` picks endianness, which differs per arch): an int
    // (id 1, skipped by the walk) then struct task { pid@0, tgid@4 }. btf_type
    // and btf_member are three u32s each; strings are task@1 pid@6 tgid@10.
    fn sample_btf(le: bool) -> Vec<u8> {
        let strings = b"\0task\0pid\0tgid\0";
        let word = |v: &mut Vec<u8>, x: u32| {
            v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() });
        };

        let mut types = Vec::new();
        for w in [
            0,
            KIND_INT << 24,
            4,
            0,
            1,
            (KIND_STRUCT << 24) | 2,
            8,
            6,
            1,
            0,
            10,
            1,
            32,
        ] {
            word(&mut types, w);
        }

        let mut buf = Vec::new();
        let magic = 0xeb9fu16;
        buf.extend_from_slice(&if le {
            magic.to_le_bytes()
        } else {
            magic.to_be_bytes()
        });
        buf.extend_from_slice(&[1, 0]); // version, flags
        // hdr_len, type_off, type_len, str_off, str_len
        for w in [
            24,
            0,
            types.len() as u32,
            types.len() as u32,
            strings.len() as u32,
        ] {
            word(&mut buf, w);
        }
        buf.extend_from_slice(&types);
        buf.extend_from_slice(strings);
        buf
    }

    #[test]
    fn resolves_field_offset_in_both_endiannesses() {
        for le in [true, false] {
            let btf = Btf::from_bytes(sample_btf(le)).unwrap();
            assert_eq!(btf.field_offset("task", "pid").unwrap(), 0);
            assert_eq!(btf.field_offset("task", "tgid").unwrap(), 4);
        }
    }

    #[test]
    fn unknown_field_is_error() {
        let btf = Btf::from_bytes(sample_btf(true)).unwrap();
        assert!(btf.field_offset("task", "missing").is_err());
    }

    #[test]
    fn unknown_struct_is_error() {
        let btf = Btf::from_bytes(sample_btf(true)).unwrap();
        assert!(btf.field_offset("missing", "pid").is_err());
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(Btf::from_bytes(vec![0; 64]).is_err());
    }

    #[test]
    fn rejects_undersized_input() {
        assert!(Btf::from_bytes(vec![0x9f, 0xeb, 1, 0]).is_err());
    }

    #[test]
    fn out_of_bounds_sections_do_not_panic() {
        let mut blob = sample_btf(true);
        blob[16..20].copy_from_slice(&u32::MAX.to_le_bytes()); // str_off past the buffer
        let btf = Btf::from_bytes(blob).unwrap();
        assert!(btf.field_offset("task", "pid").is_err());
    }

    #[test]
    fn arbitrary_input_does_not_panic() {
        for len in 0..512usize {
            let mut data: Vec<u8> = (0..len).map(|i| (i * 31 + 7) as u8).collect();
            // Force a valid magic for half the cases to drive the walk deeper.
            if len >= 2 && len % 2 == 0 {
                data[0] = 0x9f;
                data[1] = 0xeb;
            }
            let _ = Btf::from_bytes(data).and_then(|b| b.field_offset("task_struct", "pid"));
        }
    }

    // Real kernel BTF from testdata/ (fetched via `cargo xtask update-btf-fixtures`):
    // (file, task_struct::pid offset, task_struct::tgid offset). Offsets come from
    // pahole - see testdata/PROVENANCE.md.
    #[cfg(feature = "btf-fixtures")]
    const REAL_WORLD: &[(&str, u32, u32)] = &[
        ("ubuntu-x86_64.btf.tar.xz", 2264, 2268),
        ("ubuntu-arm64.btf.tar.xz", 1336, 1340),
        ("centos7-x86_64.btf.tar.xz", 1188, 1192),
    ];

    #[cfg(feature = "btf-fixtures")]
    fn vmlinux_btf(file: &str) -> Vec<u8> {
        let path = format!("{}/testdata/{file}", env!("CARGO_MANIFEST_DIR"));
        let xz = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mut reader = xz.as_slice();
        let mut tar_bytes = Vec::new();
        lzma_rs::xz_decompress(&mut reader, &mut tar_bytes).expect("xz decompress");
        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        let mut entry = archive
            .entries()
            .expect("tar entries")
            .next()
            .expect("one tar entry")
            .expect("tar entry");
        let mut btf = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut btf).expect("read btf");
        btf
    }

    #[cfg(feature = "btf-fixtures")]
    #[test]
    fn resolves_real_world_offsets() {
        assert!(
            !REAL_WORLD.is_empty(),
            "no fixtures pinned; run `cargo xtask update-btf-fixtures`"
        );
        for &(file, pid, tgid) in REAL_WORLD {
            let btf = Btf::from_bytes(vmlinux_btf(file)).unwrap();
            assert_eq!(
                btf.field_offset("task_struct", "pid").unwrap(),
                pid,
                "{file} pid"
            );
            assert_eq!(
                btf.field_offset("task_struct", "tgid").unwrap(),
                tgid,
                "{file} tgid"
            );
        }
    }
}
