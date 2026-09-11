# sysinfo

| Name | Value |
| --- | --- |
| Package | `workstation-sysinfo` |
| Binary | `sysinfo` |
| Platforms | Linux, macOS |
| Collection | In-process native probes; optional `fastfetch` enrichment |
| Pretty palette | `dotfile theme palette --json` |
| Release measurements | [PERFORMANCE.md](PERFORMANCE.md) |

```sh
sysinfo
sysinfo --pretty --health
sysinfo --full --json --timings > system.json
sysinfo bench plan --tier standard --only cpu,mem,disk --json
sysinfo bench run --tier quick --host archie
sysinfo bench compare archie@a3f19c2e archie
sysinfo bench health --host archie --json
dotfile docs --only benchmarks
dotfile dev check --pkg sysinfo --lang rust
```

## Layout and APIs

| Path | API |
| --- | --- |
| `src/collect/` | `collect_snapshot(full)`, bounded `probe`; Linux/macOS collectors |
| `src/model.rs` | `Snapshot`, `SystemView`, `HealthIssue`, `RenderOptions` |
| `src/inventory.rs` | `InventoryContext`, `parse_hosts`, `resolve_with`, `local_hostnames_with` |
| `src/health.rs` | `health_issues`, `health_summary`, CPU temperature limits |
| `src/presentation/` | `build_view`, plain/pretty renderers, branding assets |
| `src/bench/store.rs` | `Store::new(root)`, run history, baselines, exclusive lock |
| `src/bench/record.rs` | Schema 1 runs, metrics, hardware epochs |
| `src/bench/compare.rs` | `compare_runs`, method and environment gates |
| `src/bench/health.rs` | `benchmark_issues` |
| `src/bench/suites/` | CPU, memory, cache, disk, GPU, thermal, workload jobs |
| `tests/` | Linux/macOS contract fixtures, CLI/probe isolation, render regressions |
| `../bench-workloads/` | Separate native measurement worker |

## Environment

| Env | Default / fallback |
| --- | --- |
| `DOTFILE_ROOT` | Repository discovered from executable or build location |
| `SYSINFO_CONFIG` | `$DOTFILE_ROOT/config/hosts.dotfile` |
| `SYSINFO_HOST` | Pinned host, then local inventory match |
| `SYSINFO_HOSTNAME` | Detected display hostname |
| `XDG_CONFIG_HOME` | `$HOME/.config`; host pin: `dotfile/host` |
| `SYSINFO_BENCHMARKS` | `$DOTFILE_ROOT/benchmarks` |
| `XDG_CACHE_HOME` | `$HOME/.cache`; benchmark work/cache: `dotfile/bench` |
| `SYSINFO_BENCH_WORKLOADS` | Shared native resolver for `bench-workloads` |
| `SYSINFO_COLLECTOR` | In-process collector; override executable emits module JSON |
| `SYSINFO_COLLECT_TRACE` | Unset; presence enables collector timing output |
| `DOTFILE_DEV_BUILD_MANIFEST` | Unset; prepared native artifacts are authoritative when set |

## Contracts

| Name | Value |
| --- | --- |
| System JSON | Schema 1: normalized hardware, installation, system view, health |
| Timing output | `--timings`: stderr only, including subprocess probe count |
| Host precedence | Explicit argument → `SYSINFO_HOST` → host pin → inventory match |
| History | Existing `benchmarks/<host>/<run_id>.json` and `baselines.dotfile` |
| Epoch | Existing 4-byte BLAKE2s hardware identity; device ordering normalized |
| Writes | Atomic replacement and fsync; CLI mutations hold the store lock |
| Pruning | Retains baselines and oldest run per hardware epoch; rechecks under lock |
| Method compatibility | Same method major/minor; tool, platform and host gates still apply |
| Workload compatibility | Same committed dotfiles revision; dirty revisions block comparisons |
| Measurement changes | Change method major/minor when workloads or measurement semantics change |
| Quick tier | No disk or sustained thermal suite |
| Disk budget | Standard: 30 GiB; heavy: 70 GiB; failed attempts consume reserved writes |
| Plan | Reports dependencies and predicted writes; runs no measurement workloads |

`bench-workloads` remains separate so collection and presentation changes do not alter the measurement worker. Historical scores are comparable only when their recorded method and environment gates pass.
