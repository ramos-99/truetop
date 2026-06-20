//! Minimal BTF reader: resolves a struct field's byte offset against the
//! running kernel's BTF (`/sys/kernel/btf/vmlinux`).
//!
//! This is our portable stand-in for libbpf CO-RE field relocations, which the
//! Rust eBPF toolchain doesn't emit. The offset is read from the live kernel
//! and injected into the program at load (see `main`), so one binary adapts to
//! any kernel version/arch — each machine ships the BTF for its own layout.

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
        if data.len() < 24 {
            bail!("{VMLINUX_BTF} too small to be valid BTF");
        }
        // Detect endianness from the magic (0xeb9f).
        let le = match u16::from_le_bytes([data[0], data[1]]) {
            0xeb9f => true,
            _ if u16::from_be_bytes([data[0], data[1]]) == 0xeb9f => false,
            _ => bail!("bad BTF magic in {VMLINUX_BTF}"),
        };
        let mut btf = Self {
            data,
            le,
            type_off: 0,
            type_len: 0,
            str_off: 0,
        };
        // btf_header: section offsets are relative to hdr_len.
        let hdr_len = btf.u32(4) as usize;
        btf.type_off = hdr_len + btf.u32(8) as usize;
        btf.type_len = btf.u32(12) as usize;
        btf.str_off = hdr_len + btf.u32(16) as usize;
        Ok(btf)
    }

    fn u32(&self, at: usize) -> u32 {
        let b = [
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ];
        if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    }

    fn string(&self, name_off: u32) -> &str {
        let start = self.str_off + name_off as usize;
        let end = self.data[start..]
            .iter()
            .position(|&c| c == 0)
            .map_or(self.data.len(), |p| start + p);
        std::str::from_utf8(&self.data[start..end]).unwrap_or("")
    }

    fn field_offset(&self, struct_name: &str, field: &str) -> Result<u32> {
        let end = self.type_off + self.type_len;
        let mut cur = self.type_off; // type id 0 is void; the section starts at id 1.
        while cur + 12 <= end {
            let info = self.u32(cur + 4);
            let vlen = (info & 0xffff) as usize;
            let kind = (info >> 24) & 0x1f;
            let kflag = (info >> 31) & 1;
            let members = cur + 12;

            if kind == KIND_STRUCT && self.string(self.u32(cur)) == struct_name {
                for i in 0..vlen {
                    let m = members + i * 12; // btf_member: name_off, type, offset
                    if self.string(self.u32(m)) == field {
                        // For bitfield-bearing structs (kflag) the low 24 bits
                        // are the bit offset; otherwise the whole word is.
                        let bits = self.u32(m + 8);
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
