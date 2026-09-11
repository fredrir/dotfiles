# Shared terminal UI

| Directory | Package | Owns |
|---|---|---|
| [theme](theme/) | `ui-theme` | Semantic palette, color policy, runtime loading, live reload |
| [terminal](terminal/) | `ui-terminal` | Inline/alternate sessions, signals, resize, safe text, capability policy |
| [widgets](widgets/) | `ui-widgets` | Styled lines, hints, choices, viewport, prompt editing, cached search |
| [picker](picker/) | `ui-picker` | Single/multiple selection, cascading menus, fzf presentation |
| [file-explorer](file-explorer/) | `ui-file-explorer` | Local/remote navigation, source adapters, cancellable loading |
| [cli](cli/) | `ui-cli` | Clap presentation, diagnostics, plain confirmations |
| [progress](progress/) | `ui-progress` | Activity, phases, spinners, progress bars, throttled transfer output |
| [diff-view](diff-view/) | `ui-diff-view` | Bounded text comparison, line/hunk navigation, unified/split views |
| [gallery](gallery/) | `ui-gallery` | Interactive component gallery and rendered theme previews |

| Boundary | Owner |
|---|---|
| Terminal lifecycle | `ui-terminal` |
| Theme generation and contrast resolution | `dotfile::theme` |
| Runtime palette contract | `ui-theme` |
| Git operations and merge decisions | Consumer |
| File/remote identity and transport | Consumer/source adapter |
| CLI convenience imports | `workstation` reexports |

## Commands

```sh
dotfile theme gallery
dotfile theme gallery latte
dotfile theme preview sexy-purple
dotfile theme check
dotfile theme sync
dotfile dev check -p ui-theme,ui-terminal,ui-widgets,ui-picker,ui-file-explorer,ui-cli,ui-progress,ui-diff-view,ui-gallery
```

| Theme input | Priority |
|---|---|
| `DOTFILE_UI_THEME` | Explicit palette file; authoritative |
| `DOTFILE_ROOT/shared/ui/theme.json` | Explicit repository root |
| `${XDG_CONFIG_HOME:-$HOME/.config}/dotfile/ui/theme.json` | Installed palette |
| Compiled repository `shared/ui/theme.json` | Repository fallback |
| Terminal palette | No usable file |

Explicit palette paths do not fall through to another file. Live sessions keep the last valid palette if reload fails. `ThemeHandle::source()` and `error()` expose resolution. Reload checks run at most once per second; renderers receive immutable palettes.

| Policy | Behavior |
|---|---|
| `NO_COLOR`, `CLICOLOR=0`, `TERM=dumb` | Plain presentation in automatic mode |
| `PREFERS_REDUCED_MOTION`, `REDUCE_MOTION`, `REDUCED_MOTION`, consumer-specific flag | Static activity presentation |
| Redirected output / CI | Plain command output; explicit terminal adapters retain their own stream policy |
| `ui-terminal/crossterm` | Raw/cursor session without Ratatui |
| `ui-terminal/ratatui` | Inline and alternate Ratatui sessions |
| `ui-progress/ratatui` | Rich progress widgets |
| Explicit `Style::from_palette` | Fixed palette for previews and deterministic rendering |

## Selection

```rust
use ui_picker::{Item, Mode, Outcome, Picker};
use ui_theme::Style;

let style = Style::for_stdout();
let result = Picker::new(
    "Select sources",
    [Item::new(1, "Local files"), Item::new(2, "Remote snapshot")],
    &style,
)
.mode(Mode::Multiple)
.run()?;

match result {
    Outcome::Selected(ids) => { /* consume stable IDs */ }
    Outcome::Cancelled | Outcome::Interrupted | Outcome::Unavailable => {}
}
```

| Picker key | Action |
|---|---|
| `↑` / `↓`, `j` / `k` | Move focus |
| `/` or typing | Search |
| `Space` / `Tab` | Toggle focused item in multiple mode |
| `*` | Toggle all matching enabled items |
| `Enter` | Accept |
| `Esc` | Leave search or cancel |
| `?` | Contextual help |

Native pickers and explorers use `ui_terminal::Surface` and `ScriptedSurface`.

Selections survive filtering; disabled items remain inspectable. Search input has precedence over letter navigation. `SelectionState`, `ScriptedSurface`, and explicit palettes support deterministic behavior checks without a live terminal.

## Comparison

| Key | Action |
|---|---|
| `j` / `k`, Page Up / Down | Scroll |
| `[` / `]` | Previous/next changed block |
| `h` / `l` | Horizontal scroll in the standalone viewer |
| `v` | Unified / side-by-side |

Dotfile merge prompts retain their decision shortcuts; diff navigation uses `j/k`, paging, brackets, and `v`. Narrow layouts display unified text. Binary inputs and bounded previews are labeled; the source values and merge operations stay with the caller.

## Verification

| Coverage | Cases |
|---|---|
| State | Filtered selection, disabled rows, empty results, cascade backtracking |
| Rendering | Unicode cell widths, controls, narrow/empty viewports, theme roles |
| Terminal | Resize, cancellation, signals, raw mode and cursor restoration |
| Remote sources | Bounded work, stale results, cancellation while loading |
| Themes | Explicit/installed fallback, malformed reload, indexed colors, contrast |
| Consumers | Existing CLI output, completions, remote paths, merge choice protocol |

| Performance boundary | Mechanism |
|---|---|
| Filtering | Normalize once per source update; reuse indexed search text |
| Selection count | Incremental checked-item count |
| Remote browsing | Bounded worker queues; generation checks; cooperative cancellation |
| Idle rendering | Redraw on input, resize, data or palette changes |
| Progress | Throttled terminal writes |
| Diff | Bounded text/line previews and computation deadline |

```sh
cargo run --manifest-path scripts/rust/Cargo.toml --release -p ui-widgets --example search-bench -- 100000
```
