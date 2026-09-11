# sysinfo

| Name | Value |
| --- | --- |
| Package | `workstation-sysinfo` |
| Binary | `sysinfo` |
| Platforms | Linux, macOS |
| Collection | In-process native probes; optional `fastfetch` enrichment |
| Pretty palette | `ui-theme`; generated `~/.config/dotfile/ui/theme.json` |
| Release measurements | [PERFORMANCE.md](PERFORMANCE.md) |

```sh
sysinfo
sysinfo --pretty --health
sysinfo --full --json --timings > system.json
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
| `src/report.rs` | `describe_hardware`, `describe_install`, platform normalization |
| `tests/` | Linux/macOS contracts, CLI/probe isolation, render regressions |

## Environment

| Env | Default |
| --- | --- |
| `DOTFILE_ROOT` | Repository discovered from executable or build location |
| `DOTFILE_UI_THEME` | Explicit runtime palette; overrides repository and installed palettes |
| `SYSINFO_CONFIG` | `$DOTFILE_ROOT/config/hosts.dotfile` |
| `SYSINFO_HOST` | Pinned host, then local inventory match |
| `SYSINFO_HOSTNAME` | Detected display hostname |
| `XDG_CONFIG_HOME` | `$HOME/.config`; host pin: `dotfile/host` |
| `DOTFILE_DEV_BUILD_MANIFEST` | Unset; prepared native artifacts are authoritative when set |

## Contracts

| Name | Value |
| --- | --- |
| System JSON | Schema 1: normalized hardware, installation, system view, health |
| Timing output | `--timings`: stderr only, including subprocess probe count |
| Host precedence | `SYSINFO_HOST` → host pin → inventory match |
| Health | Current hardware errors and warnings |
| Ownership | Read-only collection, normalization, and presentation |

Benchmarks, history, host registration, and tuning are owned by [hwtune](../hwtune/README.md).
