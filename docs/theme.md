# dotfile theme

## Commands

| Command                | Result                                  |
| ---------------------- | --------------------------------------- |
| `dotfile theme check`  | Validate every profile and color map    |
| `dotfile theme dry`    | Show generated drift                    |
| `dotfile theme sync`   | Regenerate assigned theme outputs       |
| `dotfile theme preview`| Preview a profile                       |

## Profile

```toml
name = "Midnight Blue"
dark = true

[ui] # Mandatory
background = "#15152b" # Main background / Secondary-Sidebar- Colors
primary     = "#070132" # Active / Highlight / Primary- Colors
accent      = "#15152b" # Sidebar Background / Support- Colors
surface     = "#191a1b" # Dimmed / Border- Colors
foreground  = "#f4faff" # Text colors / brightest foreground

[ansi.normal] # Mandatory
black   = "#150f38"
red     = "#39377e"
green   = "#46448c"
yellow  = "#53519a"
blue    = "#605ea8"
magenta = "#6d6bb6"
cyan    = "#7a78c4"
white   = "#b0aedd"

[ansi.bright] # Mandatory
black   = "#26236b"
red     = "#5553a0"
green   = "#6462ae"
yellow  = "#7371bc"
blue    = "#8280ca"
magenta = "#918fd6"
cyan    = "#aaa8e0"
white   = "#ececff"
```

| Rule        | Value                                      |
| ----------- | ------------------------------------------ |
| Root keys   | `name`, `dark`, `ui`, `ansi`               |
| Color value | `#rrggbb`                                  |
| UI keys     | `background`, `primary`, `accent`, `surface`, `foreground` |
| ANSI keys   | `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white` |
| Overrides   | Semantic, font, and application overrides are rejected |

## Semantic colors

| Role           | Source                    |
| -------------- | ------------------------- |
| `background`   | `ui.background`           |
| `primary`      | `ui.primary`              |
| `accent`       | `ui.accent`               |
| `sidebar`      | `ui.accent`               |
| `surface`      | `ui.surface`              |
| `border`       | `ui.surface`              |
| `foreground`   | `ui.foreground`           |
| `error`        | `ansi.normal.red`         |
| `warning`      | `ansi.normal.yellow`      |
| `success`      | `ansi.normal.green`       |
| `info`         | `ansi.normal.blue`        |
| `cursor`       | `ui.foreground`           |
| `cursor_text`  | `ui.background`           |
| `selection_bg` | `ui.primary`              |
| `selection_fg` | `contrast(ui.primary)`    |

Application maps use semantic roles. Profiles cannot replace the source mapping.

## Assignment

`config/profiles.dotfile` assigns profiles to groups and packages.

### `config/profiles.dotfile`

```
shared {
  theme    = mocha
  obsidian = latte
}

linux/kde {
  theme = latte
}
```

| group                                 | owns                                                     |
| ------------------------------------- | -------------------------------------------------------- |
| `shared`                              | wezterm, starship, zsh, obsidian, nvim, fastfetch        |
| `linux/common`                        | GTK colours and settings, quicklaunch                    |
| `linux/kde`                           | kdeglobals, desktop-appletsrc, panel presets             |
| `linux/arch`, `linux/ubuntu`, `macos` | that platform's fastfetch config and logo                |
