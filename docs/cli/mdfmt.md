# mdfmt

```sh
mdfmt README.md
mdfmt .
mdfmt --check .
mdfmt --stdin README.md < README.md
mdfmt -eq --stdin note.md < note.md
mdfmt --dialect obsidian notes/
mdfmt --dialect github README.md
cat README.md | mdfmt
```

## Settings

| Name | Default | Values |
| --- | --- | --- |
| `dialect` | `auto` | `auto`, `commonmark`, `gfm` / `github`, `obsidian` |
| `width` | `80` | Table alignment threshold only, `0`–`10000`; `0` allows unlimited alignment |
| `table_style` | `auto` | `auto`, `aligned`, `compact` |
| `heading_blank_lines` | `1` | `0`–`3` |
| `list_marker` | `-` | `-`, `*`, `+` |
| `trim_trailing_blank_lines` | `true` | `true`, `false`; remove empty or space/tab-only lines at EOF |
| `final_newline` | `false` | `true`, `false` |

```text
mdfmt {
  dialect                   = auto
  width                     = 80
  table_style               = auto
  heading_blank_lines       = 1
  list_marker               = -
  trim_trailing_blank_lines = true
  final_newline             = false
}
```

The nearest `mdfmt.dotfile` above each file wins, followed by
`${XDG_CONFIG_HOME:-$HOME/.config}/mdfmt/mdfmt.dotfile`, `~/mdfmt.dotfile`, then
built-in defaults. Configurations replace rather than merge with one another.
`--stdin FILENAME` uses the filename for this lookup; bare stdin uses the
working directory.

With `trim_trailing_blank_lines = false`, trailing blank lines are retained even
when `final_newline = false`. `final_newline = true` ensures a terminating newline.

## Dialects

| Dialect | Behavior |
| --- | --- |
| `auto` | Configured dialect, then Obsidian beneath a `.obsidian` directory, otherwise GFM |
| `commonmark` | CommonMark syntax without Markdown extensions |
| `gfm`, `github`, `github-flavored-markdown` | GFM tables, tasks, strikethrough and autolinks, plus GitHub alerts, footnotes, math and frontmatter |
| `obsidian`, `obsidian-markdown` | GFM formatting plus Obsidian wiki links, embeds, highlights, comments, tags, math and block references |

An explicit `--dialect` overrides `mdfmt.dotfile`. Vault detection uses each
file's location, including the filename supplied through `--stdin`.

Obsidian wiki links, embeds, math and comments retain their literal contents.
Callout containers and paragraphs containing a standalone block ID are preserved
verbatim so folding markers, custom types and block references remain intact.
Tables containing wiki links still receive the configured table layout. Obsidian
plugin-specific syntax is not interpreted.

## Formatting

| Content | Behavior |
| --- | --- |
| Tables | Align if the complete table fits `width`; otherwise use one space around each cell; retain surplus cells |
| Table width | Unicode display columns, including Markdown syntax and container indentation |
| Long cells, URLs, code spans | Preserve content and existing line breaks; never wrap or join lines |
| Headings | Preserve ATX / setext syntax; configured blank lines beneath; trailing blank lines follow `trim_trailing_blank_lines` |
| Paragraphs | Preserve existing line breaks; never wrap or join lines |
| Lists | Compact simple items; normalize bullets unless that would merge lists; preserve numbering, continuation lines, tasks and nesting |
| Emphasis | Normalize to `*italic*` and `**bold**` when parsing confirms the same structure |
| Links | Normalize inline title quotes when safe; preserve reference links and all definitions, including unused ones |
| Code | Preserve code and fence style; close unfinished fences when needed to retain code content at EOF |
| Extensions | GFM tables, tasks, strikethrough, autolinks, and footnotes |
| Frontmatter | Preserve YAML `---` / `...` and TOML `+++` blocks, including empty metadata |
| Newlines | LF; trim trailing blank lines and omit final newline by default |
| Directory targets | `.md`, `.markdown`, `.mdown`, `.mkd`, case-insensitive |
| Traversal | Shared build/cache exclusions; skip symlinks in directory walks |
| Explicit files | Format regardless of extension; follow explicit symlinks |
| Writes | Atomic replacement, preserving permissions; unchanged files are not rewritten |

## Flags

| Flag | Behavior |
| --- | --- |
| `-e`, `--editor` | Read stdin and suppress routine reports; combine with `--stdin FILENAME` for per-file settings |
| `--dialect DIALECT` | Select Markdown syntax; defaults to `auto` |
| `--check` | Exit `1` on formatting differences or failure; write nothing |
| `--stdin FILENAME` | Read stdin, resolve settings beside filename, emit Markdown |
| `-` | Read stdin using settings from the working directory |
| `-v`, `--verbose` | Show unchanged files and config sources |
| `-q`, `--quiet` | Show only failures |
| `--completions SHELL` | Print shell completions |
| `-h`, `--help` | Show help |
| `-V`, `--version` | Show version |

`-eq` combines editor mode and quiet output. With no targets, piped input is
formatted; a terminal shows help unless `--editor` or `--stdin` is given.
`--check` alone checks stdin; use `--check .` to check a directory. As with
`jqfmt`, explicit `-` can accompany file targets. Editor mode accepts stdin only
and reports failures even when quiet.

`dotfile format` uses `mdfmt` for Markdown. `dotfile format --add` offers
`mdfmt.dotfile`; `dotfile sync` installs the binary and shared config.
