# hwtune

| Name | Value |
| --- | --- |
| Package / binary | `hwtune` |
| Benchmarks | Linux, macOS |
| Automatic tuning | Linux CPU governors, energy preference, and platform profiles |
| Collection API | [sysinfo](../sysinfo/README.md): normalized hardware and installation snapshots |
| Measurement worker | `bench-workloads`, built separately |
| CLI reference | [hwtune.md](../../../../docs/cli/hwtune.md) |

```sh
hwtune bench plan --tier standard --only cpu,mem,disk --json
hwtune bench run --tier quick --host archie --baseline
hwtune bench compare archie@a3f19c2e archie
hwtune bench report --before archie@a3f19c2e --after archie --json
hwtune bench health --host archie --json
hwtune tune plan --json
hwtune tune auto
hwtune tune auto --apply
hwtune tune apply
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
| `src/bench/suites/` | CPU, memory, cache, disk, GPU, thermal, workload jobs |
| `src/tune/` | OS control discovery, candidate trials, monitored validation, reversible application |
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
