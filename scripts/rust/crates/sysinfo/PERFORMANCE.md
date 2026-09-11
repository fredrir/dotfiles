# CLI performance

| Measurement | Value |
| --- | --- |
| Date | 2026-09-11 |
| Platform | macOS 26.6.2, Apple Silicon |
| Build | Cargo release profile |
| Samples | First invocation, then five warm invocations |
| Wall time | Includes `/usr/bin/time` wrapper |
| Peak RSS | Maximum across warm invocations |

| Command | Python median ms | Rust median ms | Speedup | Python RSS MiB | Rust RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| `sysinfo --help` | 79.5 | 4.4 | 17.9× | 37.9 | 6.5 |
| `sysinfo` | 180.4 | 86.5 | 2.1× | 31.9 | 13.5 |
| `sysinfo --pretty` | 500.6 | 418.7 | 1.2× | 32.8 | 17.3 |
| `sysinfo --full` | 486.7 | 421.0 | 1.2× | 32.0 | 17.3 |
| `sysinfo bench list` | 59.5 | 4.7 | 12.7× | 32.8 | 7.3 |
| `sysinfo __complete runs` | 58.6 | 4.6 | 12.9× | 32.0 | 7.2 |

| Documentation preview | Rust median ms |
| --- | ---: |
| `dotfile docs --dry-run` | 170.7 |
| `dotfile docs --only keybinds --dry-run` | 158.4 |

Machine-specific measurements; the first invocation is not a cold-cache benchmark. Full/pretty views retain optional fastfetch enrichment.

| Report | Path |
| --- | --- |
| Python baseline | [macos-before.json](../../../python/tests/performance/baselines/macos-before.json) |
| Rust release | [macos-after.json](../../../python/tests/performance/baselines/macos-after.json) |

The reports include observed subprocess probes: Python counts `Popen` calls; Rust counts bounded collector probes. These are not total descendant-process counts.

```sh
DOTFILE_PERF_SYSINFO="$PWD/scripts/rust/target/release/sysinfo" \
DOTFILE_PERF_DOTFILE="$PWD/scripts/rust/target/release/dotfile" \
DOTFILE_PERF_OUTPUT=/tmp/dotfile-performance.json \
  dotfile dev test -l python -p performance --python-workers 1
```
