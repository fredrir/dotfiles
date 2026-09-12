# hwtune

| Name | Value |
| --- | --- |
| Package / binary | `hwtune` |
| Benchmarks | Linux, macOS |
| Automatic tuning | Linux CPU governors, energy preference, cpuidle governor, and platform profiles |
| Guards | Families measured with the objective; a regression there rejects a candidate |
| Collection API | [sysinfo](../sysinfo/README.md): normalized hardware and installation snapshots |
| Measurement worker | `bench-workloads`, built separately |
| CLI reference | [hwtune.md](../../../../docs/cli/hwtune.md) |

```sh
hwtune bench plan --tier standard --only cpu,mem,disk --json
hwtune bench run --tier quick --host archie --baseline
hwtune bench compare archie@a3f19c2e archie
hwtune bench report --before archie@a3f19c2e --after archie --json
hwtune bench health --host archie --json
hwtune bench run --only compile,idle,sched,ai --note "before"
hwtune tune plan --json
hwtune tune auto --metric compile.dev --guard idle
hwtune tune auto --apply
hwtune tune apply
hwtune run --profile performance -- cargo build --release
hwtune curve status
hwtune curve bench
hwtune gpu sweep --caps 250,275,300,325,350
hwtune stress mem --tool y-cruncher --minutes 30
dotfile docs --only benchmarks
dotfile dev check --pkg hwtune --lang rust,python
```

## Layout and APIs

| Path | API |
| --- | --- |
| `src/bench/store.rs` | `Store::new(root)`, history, baselines, session paths, mutation and measurement locks |
| `src/bench/record.rs` | Schema 1 runs, metrics, hardware epochs |
| `src/bench/compare.rs` | `compare_runs`, method and environment gates |
| `src/bench/health.rs` | `issues`, `issues_for_runs`, baseline regressions, stale history |
| `src/bench/hosts.rs` | Host registration and shared inventory pin writes |
| `src/bench/provenance.rs` | BIOS/LACT source and settings hashes, live controls, stability links |
| `src/bench/experiments.rs` | Configuration groups and before/after reports |
| `src/bench/suites/` | CPU, memory, cache, disk, GPU, thermal, workload, compile, idle, sched, AI jobs |
| `src/bench/suites/compile/` | Pinned crate and lockfile built by the compile job |
| `src/power.rs` | RAPL package energy counters |
| `src/tune/` | OS control discovery, candidate trials, guards, monitored validation, reversible application, scoped runs |
| `src/curve.rs` | Per-core Curve Optimizer evidence, suggestions, per-core throughput samples |
| `src/gpu_sweep.rs` | LACT power-cap sweep with restoration |
| `tests/` | Store, comparison, CLI, tuning, and platform contracts |
| `../bench-workloads/` | Native measurement worker |

## Storage

| Data | Path |
| --- | --- |
| Store schema | `benchmarks/store.json` |
| Runs | `benchmarks/hosts/<host>/runs/<run_id>.json` |
| Baselines | `benchmarks/hosts/<host>/baselines.json` |
| Stability sessions | `benchmarks/hosts/<host>/stability/<id>.json` |
| Tuning sessions | `benchmarks/hosts/<host>/tuning/<id>.json` |
| Desired OS settings | `config/hwtune/<host>.json` |
| Bench settings | `config/hwtune/<host>.bench.dotfile` |
| Per-core throughput samples | `benchmarks/hosts/<host>/curve/<id>.json` |
| GPU sweep sessions | `benchmarks/hosts/<host>/tuning/gpu-sweep-<id>.json` |

## Environment

| Env | Default |
| --- | --- |
| `DOTFILE_ROOT` | Repository discovered from executable or build location |
| `HWTUNE_BENCHMARKS` | `$DOTFILE_ROOT/benchmarks` |
| `HWTUNE_HOST` | Shared host pin, inventory match, then local hostname |
| `HWTUNE_LACT_CONFIG` | `/etc/lact/config.yaml` |
| `HWTUNE_MEASUREMENT_LOCK` | `/tmp/hwtune-measurement.lock` |
| `XDG_CONFIG_HOME` | `$HOME/.config`; shared host pin: `dotfile/host` |
| `XDG_CACHE_HOME` | `$HOME/.cache`; benchmark work/cache: `hwtune/bench` |
| `DOTFILE_DEV_BUILD_MANIFEST` | Unset; prepared native artifacts are authoritative when set |

## Contracts

| Name | Value |
| --- | --- |
| Run schema | 1; metric identities and measurement methods retained |
| Store schema | 1; host directories contain runs, baselines, and session evidence |
| Epoch | 4-byte BLAKE2s hardware identity; device ordering normalized |
| Writes | Atomic writes with fsync; CLI mutations hold the store lock |
| Measurements | Machine-wide exclusive lock across benchmark, stress, and tuning commands |
| Pruning | Retains baselines, oldest runs per epoch, and runs linked to tuning or stability sessions |
| Method compatibility | Same method major/minor, units, direction, comparison scope, and applicable tool/platform/host gates |
| Workload compatibility | Same committed dotfiles revision; dirty revisions block comparisons |
| Measurement changes | Change method major/minor when workloads or measurement semantics change |
| Quick tier | No disk or sustained thermal suite |
| Metric families | `cpu`, `mem`, `cache`, `disk`, `gpu`, `thermal`, `workload`, `compile`, `idle`, `sched`, `ai` |
| Compile job | Embedded pinned crate, clean dev and release builds, `RUSTC_WRAPPER` and `RUSTFLAGS` cleared, default linker |
| Idle job | 8 s windows: package W, GPU W, fan rpm, Tctl; lower is better |
| AI job | `llama-cli` prompt and generation t/s on the configured model; GPU W sampled |
| Guards | Default `idle`; `evaluate` rejects any candidate with a regression outside noise |
| OS control writes | Direct as root; otherwise `sudo tee` after one `sudo -v` prompt kept alive for the session |
| Scoped run | Child runs as the invoking user, through setpriv under sudo; original controls restored on exit or signal |
| GPU sweep | Caps within the LACT range; original cap restored, also on error |
| Disk budget | Standard: 30 GiB; heavy: 70 GiB; failed attempts consume reserved writes |
| Plan | Reports dependencies and predicted writes without running measurement workloads |
| Configuration groups | BIOS and LACT settings hashes; export timestamps do not split equal settings |
| Firmware provenance | Imported settings recorded without claiming they are active |
| Automatic trials | Original OS controls restored unless a validated winner is retained with `--apply` |
| Winner validation | Repeated CPU gains above noise and minimum improvement, monitored stress, temperature and kernel error checks |
| Tuning evidence | Temperature sensors and kernel error journal required |
| Desired state | Git-tracked host profile; `tune apply` validates and applies the checked-out revision |
| Profile application | Validates local identity, controls, and stability; permits slower restored settings |
| Profile rollback | Restore the file with Git, then run `hwtune tune apply` |

```sh
git restore --source=<commit> -- config/hwtune/<host>.json
hwtune tune apply
```
