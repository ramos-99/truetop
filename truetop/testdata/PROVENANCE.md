# BTF test fixtures

Real kernel BTF used by `btf::tests` to check the parser against layouts it
cannot reach synthetically: large type sections, bitfields, and a second
architecture.

Each file is the verbatim `*.btf.tar.xz` from BTFHub, so its sha256 matches
upstream and provenance is verifiable by re-fetching. The tests decompress them
in memory and never touch the network; `cargo xtask update-btf-fixtures` is the
only thing that downloads, and it verifies the checksums below.

Source: https://github.com/aquasecurity/btfhub-archive @ `f5eaeacd47ab8924bbe554bdfaaef796ada09016`

| file                        | distro / kernel             | arch   | sha256                                                             |
| --------------------------- | --------------------------- | ------ | ---------------------------------------------------------------- |
| `ubuntu-x86_64.btf.tar.xz`  | Ubuntu 20.04, 5.4.0-26      | x86_64 | `f5435aba2a2f85c289dec4c52f7f3f01f7139f3143561e9c1b02ced174514b9d` |
| `ubuntu-arm64.btf.tar.xz`   | Ubuntu 20.04, 5.4.0-26      | arm64  | `41822465e93f9e2b206b6fb08fedf8cb0347f1c2ac03c41a770ac4ff3bdbda6e` |
| `centos7-x86_64.btf.tar.xz` | CentOS 7, 3.10.0-1062.1.1   | x86_64 | `f5caa317e64ee89bab59744ad94b827ef99f53b1216c91446b0293327e3cab80` |

## Oracle offsets

Offsets are read with pahole, an independent BTF parser, so the assertions do
not depend on the code under test. Regenerate per file with:

    xz -dc <file>.btf.tar.xz | tar -xO > vmlinux.btf
    pahole -C task_struct vmlinux.btf   # read the byte offset on pid and tgid

| arch / kernel        | `task_struct::pid` | `task_struct::tgid` |
| -------------------- | ------------------ | ------------------- |
| x86_64, 5.4          | 2264               | 2268                |
| arm64, 5.4           | 1336               | 1340                |
| x86_64, 3.10 (el7)   | 1188               | 1192                |

pahole: v1.31
