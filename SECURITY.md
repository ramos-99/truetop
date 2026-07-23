# Security policy

## Reporting

Report vulnerabilities privately through GitHub's
[security advisory form](https://github.com/ramos-99/truetop/security/advisories/new).
If that is unavailable, email m@martimcr.com. Do not open a public issue.

Include the commit or release, the kernel version and architecture, and a
reproduction. A proof of concept shortens triage.

This is a small project: expect an acknowledgement within a week and an honest
estimate, not a fixed timeline. Fixes land on `main` and in the next release.
Reporters are credited unless they decline.

## Trust boundary

truetop loads eBPF programs and needs `CAP_BPF` and `CAP_PERFMON`, or root. That
privilege is what makes it worth attacking, so the surface that matters is
narrow:

- The user-space loader and its `/proc` parsing. It is Rust; the `unsafe` blocks
  are the sharp edges: the raw `bpf(2)` calls in `batch`, and the libc FFI.
- The `task_struct` offsets read from kernel BTF and injected at load. Wrong
  offsets misread kernel memory, but every read goes through
  `bpf_probe_read_kernel`, which faults safely rather than corrupting.
- The eBPF programs, which the kernel verifier accepts before they run.

The kernel verifier and helpers are the boundary below truetop: a flaw in either
is a kernel issue, not a truetop one.

## In scope

Anything in this repository that, given only the documented privileges, lets a
caller exceed them, crash or corrupt the host, or read data across a trust
boundary.

## Out of scope

- Running truetop without the privileges it documents, or granting it more than
  it asks for.
- The `sched_switch` overhead, which is measured and disclosed in the README. It
  is a documented cost, not a vulnerability.
- Kernel bugs reachable only because truetop exercises a kernel path. Report
  those to the kernel.
- Dependency vulnerabilities not triggered by truetop's use of them. Report those
  upstream.

## Supported versions

Before 1.0, only the latest release and `main` receive fixes.
