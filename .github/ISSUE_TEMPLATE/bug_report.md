---
name: Bug report
about: truetop does something wrong
labels: bug
---

<!--
truetop reads kernel structures through offsets resolved at load, so the kernel
and distribution are the difference between a report that can be looked at and
one that cannot. Security issues go through SECURITY.md instead.
-->

**What happened**, and what you expected instead:

**How to reproduce**:

**Kernel** (`uname -a`):

**Distribution**:

**Running as**: root, `setcap`, container, VM

**Startup log** (`RUST_LOG=info` prints the resolved offsets and map sizing):

<!-- If a number looks wrong, add the same window from top, htop or iotop. -->
