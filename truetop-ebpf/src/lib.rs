//! Empty by design. A package needs a lib target to be depended on by path, and
//! `truetop`'s build script depends on this one so cargo invalidates the loader
//! when the kernel code changes. The programs themselves are all in `main.rs`.
#![no_std]
