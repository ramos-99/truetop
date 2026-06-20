//! Types shared across the eBPF ↔ user-space FFI boundary (`#[repr(C)]`,
//! CLAUDE.md §4). Phase 1 is CPU-only and crosses the boundary as a bare
//! `PerCpuHashMap<u32, u64>`, so nothing lives here yet; the block-I/O keys and
//! records land in Phase 2.
#![no_std]
