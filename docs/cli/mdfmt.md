# mdfmt

```sh
mdfmt README.md
mdfmt .
mdfmt --check .
mdfmt --stdin README.md < README.md
cat README.md | mdfmt
```

## Settings

| Name | Default | Values |
| --- | --- | --- |
| `width` | `80` | `0`–`10000`; `0` preserves prose line breaks and allows unlimited table alignment |
| `table_style` | `auto` | `auto`, `aligned`, `compact` |
| `heading_blank_lines` | `1` | `0`–`3` |
| `list_marker` | `-` | `-`, `*`, `+` |
| `final_newline` | `false` | `true`, `false` |

```text
mdfmt {
  width                = 80
  table_style          = auto
  heading_blank_lines  = 1
  list_marker          = -
  final_newline        = false
}
```

The nearest `mdfmt.dotfile` above each file wins, followed by
`${XDG_CONFIG_HOME:-$HOME/.config}/mdfmt/mdfmt.dotfile`, `~/mdfmt.dotfile`, then
built-in defaults. Configurations replace rather than merge with one another.
`--stdin FILENAME` uses the filename for this lookup; bare stdin uses the
working directory.

## Formatting

| Content | Behavior |
| --- | --- |
| Tables | Align if the complete table fits `width`; otherwise use one space around each cell |
| Table width | Unicode display columns, including Markdown syntax and container indentation |
| Long cells, URLs, code spans | Preserve content; `width` is a soft limit |
| Headings | ATX headings with configured blank lines beneath; no trailing blank lines at EOF |
| Paragraphs | Reflow at `width`; preserve explicit hard breaks |
| Lists | Consistent bullet marker and ordered numbering; preserve tasks and nesting |
| Emphasis | Normalize to `*italic*` and `**bold**`, with escaping as needed |
| Links | Normalize destinations and titles; expand reference links inline |
| Code | Fenced blocks; preserve code contents |
| Extensions | GFM tables, tasks, strikethrough, autolinks, and footnotes |
| Frontmatter | Preserve YAML `---` and TOML `+++` blocks |
| Newlines | LF; no final newline by default |
| Directory targets | `.md`, `.markdown`, `.mdown`, `.mkd`, case-insensitive |
| Traversal | Shared build/cache exclusions; skip symlinks in directory walks |
| Explicit files | Format regardless of extension; follow explicit symlinks |
| Writes | Atomic replacement, preserving permissions; unchanged files are not rewritten |

## Flags

| Flag | Behavior |
| --- | --- |
| `--check` | Exit `1` on formatting differences or failure; write nothing |
| `--stdin FILENAME` | Read stdin, resolve settings beside filename, emit Markdown |
| `-` | Read stdin using settings from the working directory |
| `-v`, `--verbose` | Show unchanged files and config sources |
| `-q`, `--quiet` | Show only failures |
| `--completions SHELL` | Print shell completions |
| `-h`, `--help` | Show help |
| `-V`, `--version` | Show version |

`dotfile format` uses `mdfmt` for Markdown. `dotfile format --add` offers
`mdfmt.dotfile`; `dotfile sync` installs the binary and shared config.