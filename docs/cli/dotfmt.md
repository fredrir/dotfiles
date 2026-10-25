# dotfmt

## Commands

<!-- cli:commands:start -->
| Command  | Description                                                                                                      |
| -------- | ---------------------------------------------------------------------------------------------------------------- |
| `dotfmt` | Formats configured BibTeX, conf, JSON, LaTeX, Lua, Markdown, TOML and YAML files with one layered configuration. |
<!-- cli:commands:end -->

```sh
dotfmt .
dotfmt --check .
dotfmt -l json,markdown,yaml .
dotfmt -l lua --dialect luau script.luau
dotfmt -l latex,bib .
dotfmt -l latex --dialect context main.tex
dotfmt -l conf --stdin ~/.ssh/config < ~/.ssh/config
dotfmt -e note.md < note.md
dotfmt -c -l md note.md
dotfmt --add .
dotfmt --sync .
```

Editor mode uses the filename for language detection and local configuration, reads the buffer from stdin, and writes formatted text to stdout. Formatting or configuration errors return the original input unchanged, with no diagnostics and exit status `0`. Use `--verbose` to diagnose failures; `--check` retains normal failure status. It never writes the named file. Conform uses one formatter for all supported filetypes:

```lua
dotfmt = { command = "dotfmt", args = { "-e", "$FILENAME" } }
```

Selection uses filenames and `dotfmt.dotfile`, not the editor's filetype. For JSONC stored as `.json`, set `dialect = jsonc` in the applicable `json` block; map extensionless names through that block's `include` rules.

## Flags

<!-- cli:flags:start -->
| Flag                      | Description                                                                                                                                  |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `-l`, `--lang <LANGUAGE>` | Select configured languages; repeat or comma-separate bib (bibtex, biblatex), conf, json, latex (tex), lua, markdown (md), toml, yaml (yml). |
| `--check`                 | Report formatting differences without writing files.                                                                                         |
| `-c`, `--clean`           | Format Markdown and strip all Markdown formatting; requires --lang markdown.                                                                 |
| `-a`, `--add`             | Offer dotfmt.dotfile to the target, asking before writing.                                                                                   |
| `-s`, `--sync`            | Replace an existing target dotfmt.dotfile with the shared configuration.                                                                     |
| `--dialect <DIALECT>`     | Override the dialect for one selected language.                                                                                              |
| `-e`, `--editor`          | Read standard input for an editor; JSON mode enables repairs.                                                                                |
| `--stdin <FILENAME>`      | Read standard input as the named file, using its configuration and dialect.                                                                  |
| `--owns`                  | Read NUL-separated filenames and emit those selected by the effective configuration.                                                         |
| `-v`, `--verbose`         | Show detailed formatting results.                                                                                                            |
| `-q`, `--quiet`           | Report only failures.                                                                                                                        |
| `-h`, `--help`            | Shows help for the selected command and exits.                                                                                               |
| `--completions <SHELL>`   | Prints a shell completion script for the named shell and exits.                                                                              |
| `-V`, `--version`         | Prints the version and exits.                                                                                                                |
<!-- cli:flags:end -->

## Configuration

| Precedence, low to high | Source |
| --- | --- |
| 1 | Engine defaults |
| 2 | `${XDG_CONFIG_HOME:-$HOME/.config}/dotfmt/dotfmt.dotfile` |
| 3 | Ancestor `dotfmt.dotfile` files, outermost to nearest |
| 4 | CLI dialect override |

Each layer applies globals, then language settings. Only specified values replace inherited values. A nearer global value replaces an outer language value. Empty language blocks enable inherited defaults; `enabled = false` disables a language. `--lang` selects configured languages; it does not enable disabled or absent blocks.

```text
{
  width = 80
  final_newline = false
  quote_style = double
}

conf {
  indent = 2
  include {
    *.ssh
  }
}

markdown {}
json {}
toml {}
yaml {}
latex {}
bib {}

lua {
  width = 120
  verify = false
  excluded_files {
    init.lua
  }
}

excluded_files {
  README.md
}
```

| Rule | Behavior |
| --- | --- |
| `included_files` / `include`, global | Nonempty restricts eligible files; empty allows all supported mappings |
| `included_files` / `include`, language | Adds custom filename mappings alongside built-in extensions |
| `excluded_files` / `exclude` | Removes matching files |
| Pattern syntax | Gitignore patterns, negation, directory rules, escaped special characters |
| Pattern roots | Config directory; installed global config uses invocation directory |
| Inheritance | Later matching rules win; explicitly empty blocks reset the inherited list |
| Ambiguous mappings | Error; select the language explicitly |
| Custom extensions | Must contain syntax understood by the assigned engine |

The shipped config enables all eight languages. `dotfile sync` installs it. `--add` and `--sync` use that single configuration file.

## Languages and settings

| Language | Built-in files | Dialects |
| --- | --- | --- |
| `bib` | `.bib` | None; BibTeX and BibLaTeX syntax share one grammar |
| `conf` | `.conf`, `.config`, `.dotfile` | Filename-dependent plain/Hyprland and block formatting |
| `json` | `.json`, `.jsonc`, `.hujson`, `.jwcc`, `.jsonl`, `.ndjson`, `.jsonlines` | `auto`, `json`, `jsonc`, `hujson` (`jwcc`), `jsonl` (`ndjson`, `jsonlines`) |
| `latex` (`tex`) | `.tex`, `.ltx`, `.latex`, `.sty`, `.cls`, `.dtx`, `.ins`, `.def`, `.clo`, `.lco`, `.fd`, `.bbx`, `.cbx`, `.lbx`, `.pgf`, `.tikz`, `.ctx`, `.mkii`, `.mkiv`, `.mkvi`, `.mkxl` | `auto`, `latex` (`pdftex`, `xelatex`, `lualatex`, `expl3`, `beamer`, `tikz`), `plain` (`tex`, `optex`), `context` (`ctx`, `mkiv`, `lmtx`) |
| `lua` | `.lua`, `.luau` | `auto`, `all`, `lua51`–`lua54`, `luajit`, `luau` |
| `markdown` (`md`) | `.md`, `.markdown`, `.mdown`, `.mkd` | `auto`, `commonmark`, `gfm` (`github`), `obsidian` |
| `toml` | `.toml` | None |
| `yaml` (`yml`) | `.yaml`, `.yml` | None |

| Global setting | Application |
| --- | --- |
| `final_newline` | All languages |
| `indent` | Bib, Conf blocks, JSON, LaTeX, Lua, TOML, YAML |
| `width` | Bib and LaTeX wrapping, Lua line wrapping, Markdown table layout, YAML flow collections and TOML arrays/inline tables |
| `quote_style` | Lua and YAML quote selection; JSON always emits double quotes; Bib, conf, LaTeX, Markdown and TOML preserve quoting and ignore it |

| Language | Additional settings |
| --- | --- |
| Bib | `indent_type`, `line_endings`, `blank_lines`, `align`, `align_max`, `trailing_comma`, `wrap`, `wrap_min`, `field_name_case`, `entry_type_case` |
| Conf | `align`, `align_max`, `blank_lines` |
| JSON | `dialect`; `indent = -1` uses tabs, `0` emits compact JSON, `1`–`7` uses spaces; JSONL ignores `indent` |
| LaTeX | `dialect`, `indent_type`, `line_endings`, `wrap`, `wrap_min`, `wrap_chars`, `format_tables`, `blank_lines`, `verify`, `lists`, `verbatims`, `no_indent_envs` |
| Lua | `dialect`, `indent_type`, `line_endings`, `call_parentheses`, `collapse_simple_statement`, `space_after_function_names`, `block_newline_gaps`, `sort_requires`, `verify` |
| Markdown | `dialect`, `table_style`, `autosize_table`, `blank_lines`, `heading_blank_lines`, `list_marker`, `trim_trailing_blank_lines` |
| TOML | `line_endings`, `allowed_blank_lines`, `align_entries`, `align_comments`, `align_single_comments`, `array_trailing_comma`, `array_auto_expand`, `array_auto_collapse`, `compact_arrays`, `compact_inline_tables`, `compact_entries`, `inline_table_expand`, `indent_tables`, `indent_entries`, `reorder_keys`, `format_comments` |
| YAML | `line_endings`, `indent_sequences`, `trailing_comma`, `brace_spacing`, `bracket_spacing`, `dash_spacing`, `prefer_single_line`, `format_comments` |

Markdown table dividers use three dashes by default, preserving alignment colons. Set `autosize_table = true` in the `markdown` block to size dividers to aligned columns; compact tables keep three dashes.

LaTeX verbatim environments and inline `\verb`, `\lstinline`, `\mintinline`, `\url`, `\path`, `\href` and file arguments are protected byte-for-byte before layout. A `\begin`, `\end`, `\item` or sectioning command that shares a line with non-whitespace text is split off with a terminating `%`, so the newline adds no space token; `% tex-fmt: off`/`% tex-fmt: on` regions are preserved byte-for-byte. `dialect = context` maps `\start...`/`\stop...` environments onto the same indentation engine; `plain` leaves environment syntax alone. The formatter rejects nesting deeper than 100 levels. `format_tables` aligns `tabular` rows only; width wraps text and comments but never rewrites protected content. `verify = true` formats a second time and requires the result to settle.

Bib output keeps block and field order, field and entry keys, value delimiters, macro names used in values and every value byte; it never deduplicates or sorts. Field and entry type case are configurable, and malformed input is reported instead of repaired: a bare `@` that does not start a block, an unterminated `@comment`, a comment between concatenated value parts and a comment before the end of `@preamble` are located syntax errors. `@string` definition names follow `field_name_case`; macro uses keep their spelling. `line_endings = windows` also rewrites newlines inside multi-line values. `wrap` only replaces existing spaces inside values with line breaks and continuation indentation; verification ignores those inserted whitespace changes.

Known global settings apply where supported. Unsupported language-local settings are errors. Markdown width does not reflow prose. Markdown drops leading blank lines and collapses runs longer than `blank_lines` outside code and HTML blocks. `auto` detects Lua/JSON dialects from filenames and Obsidian from vault ancestry. Markdown preserves frontmatter and protected content; `--clean` with `--lang markdown` formats and then strips all Markdown formatting, leaving plain text, code and table cells. JSONC/HuJSON editor mode preserves comments. JSONL reads any whitespace-separated values, like `jq -c`, and writes one compact record per line, dropping blank lines. YAML output must parse to the same values as its input, otherwise the file is left unchanged. TOML output must preserve values and comments or the file is left unchanged.

| Result | Exit status |
| --- | --- |
| Successful format / clean check | `0` |
| Editor formatting/configuration failure, without `--verbose` or `--check` | `0` (original input) |
| Formatting, configuration, I/O failure or check differences | `1` |
| CLI usage error | `2` |