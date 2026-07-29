<!--
Summarise the change here. The detail belongs in the commit messages; if this is
a single commit, its message is the summary. Subjects are `area: what it does`,
see CONTRIBUTING.md. Link an issue with `Fixes: #N`.
-->

### Tests

- [ ] Added or updated, and they fail without this change
- [ ] Not applicable, because:

### Checks

- [ ] `cargo +nightly fmt --all`
- [ ] `cargo +nightly clippy --workspace --exclude truetop-ebpf --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] `cargo xtask test`, which needs root and loads the programs on a live kernel

### If this touches `sched_switch` or the collector

Published figures are machine-specific and yours will not match them. What counts
is before and after on the same machine, in one session:

- [ ] `cargo xtask bench hotpath` reads about the same either side, or the
      difference is explained
- [ ] If performance is the point of this change, both numbers are in the
      description
