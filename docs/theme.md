# dotfile theme

A theme profile is an immutable set of five UI primitives and sixteen ANSI colors. Semantic
roles and application mappings live in the generator so every profile follows the same rules.

```toml
name = "Midnight Blue"
dark = true

[ui]
background = "#15152b"
primary = "#070132"
accent = "#15152b"
surface = "#191a1b"
foreground = "#f4faff"

[ansi.normal]
# black, red, green, yellow, blue, magenta, cyan, white

[ansi.bright]
# black, red, green, yellow, blue, magenta, cyan, white
```

No application roles or overrides belong in a profile. The parser rejects missing and unknown
fields.

## Semantic colors

Primitive colors describe palette intent. They are not assumed to be readable in every context.
The tooling derives contextual roles such as `primary_fill`, `on_primary`,
`primary_text_on_canvas`, `border_on_panel`, and `error_text_on_canvas`.

The literal semantic aliases are deterministic:

| Alias | Primitive |
|---|---|
| `background` | `ui.background` |
| `primary` | `ui.primary` |
| `accent`, `sidebar` | `ui.accent` |
| `surface`, `border` | `ui.surface` |
| `foreground` | `ui.foreground` |
| `error`, `warning`, `success`, `info` | ANSI normal red, yellow, green, blue |

Application maps use the contextual variants, not those literal aliases, whenever a color is
text, a fill, or a boundary. For example, text on the sidebar uses `text_on_panel` rather than
assuming `ui.foreground` will contrast with `ui.accent`.

Normal text must reach 4.5:1 contrast. Meaningful boundaries, progress indicators, and inactive
text must reach 3:1. Hue-bearing foreground roles retain the source hue while changing OKLab
lightness by the smallest amount needed. Raw ANSI colors remain unchanged for terminals and are
reported, but are not required to pass a UI-text threshold.

`dotfile theme check` validates every profile before generation, including resolved GTK, KDE,
Obsidian, and Yazi pairs. `dotfile theme contrast [PROFILE]` prints the same audit. Generated
per-profile reports live in `theme/contrast/`, and complete Yazi state snapshots live in
`theme/snapshots/yazi/`.

Yazi validation is pinned to the `26.8.15+` theme contract in the tooling. Its hosted JSON
schema currently trails the application for `[which].border` and the renamed `[help]` fields,
so generated snapshots deliberately do not attach that stale schema.

## Profile assignment

`config/profiles.dotfile` declares which profile each group uses and allows package-specific
overrides:

```text
shared {
  theme    = mocha
  obsidian = latte
}

linux/kde {
  theme = latte
}
```

| Group | Owns |
|---|---|
| `shared` | WezTerm, Starship, Zsh, Obsidian, Neovim, Yazi, and Fastfetch |
| `linux/common` | GTK colors and settings, Quicklaunch |
| `linux/kde` | `kdeglobals`, desktop applet state, panel presets |
| `linux/arch`, `linux/ubuntu`, `macos` | Platform-specific Fastfetch config and logo |
