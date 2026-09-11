# CLI performance

| Measurement | Value |
| --- | --- |
| Date | 2026-09-11 |
| Platform | macOS 26.6.2, Apple Silicon |
| Build | Cargo release profile |
| Samples | First invocation, then five warm invocations |
| Wall time | Includes `/usr/bin/time` wrapper |
| Peak RSS | Maximum across warm invocations |

| Operation | Python median ms | Rust median ms | Speedup | Python RSS MiB | Rust RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| `sysinfo --help` | 79.5 | 4.4 | 17.9× | 37.9 | 6.5 |
| `sysinfo` | 180.4 | 86.5 | 2.1× | 31.9 | 13.5 |
| `sysinfo --pretty` | 500.6 | 418.7 | 1.2× | 32.8 | 17.3 |
| `sysinfo --full` | 486.7 | 421.0 | 1.2× | 32.0 | 17.3 |
| Benchmark-history listing, before extraction | 59.5 | 4.7 | 12.7× | 32.8 | 7.3 |
| Benchmark-run completion, before extraction | 58.6 | 4.6 | 12.9× | 32.0 | 7.2 |

| Documentation preview | Rust median ms |
| --- | ---: |
| `dotfile docs --dry-run` | 170.7 |
| `dotfile docs --only keybinds --dry-run` | 158.4 |

Machine-specific measurements; the first invocation is not a cold-cache benchmark. Full/pretty views retain optional fastfetch enrichment. Benchmark-history measurements predate extraction into hwtune and do not measure the current hwtune binary.

| Report | Path |
| --- | --- |
| Python baseline | [macos-before.json](benchmarks/cli-macos-before.json) |
| Rust release | [macos-after.json](benchmarks/cli-macos-after.json) |

The reports include observed subprocess probes: Python counts `Popen` calls; Rust counts bounded collector probes. These are not total descendant-process counts.

New measurements use Hyperfine directly; its timings exclude the shell and time wrapper used above.

```sh
cargo build --release --locked --manifest-path scripts/rust/Cargo.toml -p workstation-sysinfo -p dotfile-cli
hyperfine --shell=none --warmup 1 --runs 5 --export-json /tmp/sysinfo-timings.json \
  'scripts/rust/target/release/sysinfo --help' \
  'scripts/rust/target/release/sysinfo' \
  'scripts/rust/target/release/sysinfo --pretty' \
  'scripts/rust/target/release/sysinfo --full'
hyperfine --shell=none --warmup 1 --runs 5 \
  'scripts/rust/target/release/dotfile docs --dry-run' \
  'scripts/rust/target/release/dotfile docs --only keybinds --dry-run'
```
