# sysinfo

| Name                 | Value                                                                 |
| -------------------- | --------------------------------------------------------------------- |
| Package              | `workstation-sysinfo`                                                 |
| Binary               | `sysinfo`                                                             |
| Platforms            | Linux, macOS                                                          |
| Collection           | Parallel in-process probes per scope; optional `fastfetch` enrichment |
| Pretty palette       | `ui-theme`; generated `~/dotfiles/config/ui/theme.json`               |
| Release measurements | [PERFORMANCE.md](PERFORMANCE.md)                                      |

```sh
sysinfo
sysinfo --pretty --health
sysinfo --full --json --timings > system.json
sysinfo -s
sysinfo -sc -n 10
sysinfo -st archie
dotfile dev check --pkg sysinfo --lang rust
```

## Scopes and probes

| Scope     | Flags             | Enrichment | Identity probes | CPU load    |
| --------- | ----------------- | ---------- | --------------- | ----------- |
| Dashboard | `-p`              | no         | no              | sampled     |
| Summary   | default, `--json` | no         | yes             | not shown   |
| Full      | `-f`, `--full`    | optional   | yes             | sampled     |
| Processes | `-s`, `--system`  | no         | no              | per process |

| Name        | Value                                                             |
| ----------- | ----------------------------------------------------------------- |
| Enrichment  | `fastfetch` on `PATH`, asked only for kinds no collector owns     |
| Identity    | `$SHELL` version and terminal `--version`                         |
| CPU load    | Two tick readings around the collectors; no sleep, no subprocess  |
| Hostnames   | macOS dynamic store in process; `hostname` probe only as fallback |

## Processes

| Column  | Value                                            |
| ------- | ------------------------------------------------ |
| CPU     | Share of all logical cores, then cores in use    |
| MEM     | Share of physical memory, then size              |
| GPU     | Share of all GPUs; `—` without GPU work          |
| TIME    | Running time: `s`, `m`, `h`, `d`, `w`, `y`       |
| COMMAND | Program and arguments; `App ×N` for a group of N |

| Name              | Value                                                               |
| ----------------- | ------------------------------------------------------------------- |
| Window            | Two process readings 200 ms apart                                   |
| Default rank      | Largest of CPU, memory, and GPU share                               |
| `-c`, `-m`, `-g`  | Rank by CPU, memory, or GPU share                                   |
| Group             | Same app along the parent chain, plus same-app siblings of one user |
| App               | Outermost `.app` bundle, else executable, else process name         |
| `--split`         | One row per process                                                 |
| macOS memory      | Physical footprint; `ps` RSS for other users' processes             |
| macOS other users | Setuid `ps` readings inside the window                              |
| macOS GPU         | `IOAccelerator` client `AppUsage` GPU time; undocumented            |
| Linux GPU         | NVML per-process SM utilization over the last second                |
| `-t`, `--target`  | `ssh <host>` runs the host's `sysinfo -s --json`; rendered locally  |

## Layout and APIs

| Path                 | API                                                                          |
| -------------------- | ---------------------------------------------------------------------------- |
| `src/collect/`       | `collect_snapshot(full)`, `Scope`, bounded `probe`; Linux/macOS collectors   |
| `src/collect/cpu.rs` | `Sampler`, `percentages`, tick counter parsers                               |
| `src/model.rs`       | `Snapshot`, `SystemView`, `HealthIssue`, `RenderOptions`                     |
| `src/inventory.rs`   | `InventoryContext`, `parse_hosts`, `resolve_with`, `local_hostnames_with`    |
| `src/health.rs`      | `health_issues`, `health_summary`, CPU temperature limits                    |
| `src/presentation/`  | `build_view`, plain/pretty renderers, branding assets                        |
| `src/report.rs`      | `describe_hardware`, `describe_install`, platform normalization              |
| `src/top/`           | `sample`, `report`, `group::rows`, `rank`, `render::render`, `remote::fetch` |
| `tests/`             | Linux/macOS contracts, CLI/probe isolation, render regressions               |

## Environment

| Env                          | Default                                                               |
| ---------------------------- | --------------------------------------------------------------------- |
| `DOTFILE_ROOT`               | Repository discovered from executable or build location               |
| `DOTFILE_UI_THEME`           | Explicit runtime palette; overrides repository and installed palettes |
| `SYSINFO_CONFIG`             | `$DOTFILE_ROOT/config/hosts.dotfile`                                  |
| `SYSINFO_HOST`               | Pinned host, then local inventory match                               |
| `SYSINFO_HOSTNAME`           | Detected display hostname                                             |
| `XDG_CONFIG_HOME`            | `$HOME/.config`; host pin: `dotfile/host`                             |
| `DOTFILE_DEV_BUILD_MANIFEST` | Unset; prepared native artifacts are authoritative when set           |
| `DOTFILES_COMPILED`          | `--target` host: `$HOME/dotfiles/.bin`                                |

## Contracts

| Name            | Value                                                            |
| --------------- | ---------------------------------------------------------------- |
| System JSON     | Schema 1: normalized hardware, installation, system view, health |
| Process JSON    | Schema 1: host, cores, memory, gpu, ranked rows                  |
| Timing output   | `--timings`: stderr only, including subprocess probe count       |
| Host precedence | `SYSINFO_HOST` → host pin → inventory match                      |
| Health          | Current hardware errors and warnings                             |
| Module owner    | Native collectors win; enrichment only fills unowned kinds       |
| CPU load window | The collection itself, minus this process and its probes         |
| Ownership       | Read-only collection, normalization, and presentation            |

Benchmarks, history, host registration, and tuning are owned by [hwtune](../hwtune/README.md).
