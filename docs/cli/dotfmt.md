# dotfmt

## Commands

<!-- cli:commands:start -->
| Command  | Description                                                                           |
| -------- | ------------------------------------------------------------------------------------- |
| `dotfmt` | Formats configured conf, JSON, Lua and Markdown files with one layered configuration. |
<!-- cli:commands:end -->

```sh
dotfmt .
dotfmt --check .
dotfmt -l json,markdown .
dotfmt -l lua --dialect luau script.luau
dotfmt -l conf --stdin ~/.ssh/config < ~/.ssh/config
dotfmt -e note.md < note.md
dotfmt --add .
dotfmt --sync .
```

Editor mode uses the filename for language detection and local configuration, reads the buffer from stdin, and writes only formatted text to stdout. It never writes the named file. Conform uses one formatter for all supported filetypes:

```lua
dotfmt = { command = "dotfmt", args = { "-e", "$FILENAME" } }
```

Selection uses filenames and `dotfmt.dotfile`, not the editor's filetype. For JSONC stored as `.json`, set `dialect = jsonc` in the applicable `json` block; map extensionless names through that block's `include` rules.

## Flags

<!-- cli:flags:start -->
| Flag                      | Description                                                                           |
| ------------------------- | ------------------------------------------------------------------------------------- |
| `-l`, `--lang <LANGUAGE>` | Select configured languages; repeat or comma-separate conf, json, lua, markdown (md). |
| `--check`                 | Report formatting differences without writing files.                                  |
| `-a`, `--add`             | Offer dotfmt.dotfile to the target, asking before writing.                            |
| `-s`, `--sync`            | Replace an existing target dotfmt.dotfile with the shared configuration.              |
| `--dialect <DIALECT>`     | Override the dialect for one selected language.                                       |
| `-e`, `--editor`          | Read standard input for an editor; JSON mode enables repairs.                         |
| `--stdin <FILENAME>`      | Read standard input as the named file, using its configuration and dialect.           |
| `--owns`                  | Read NUL-separated filenames and emit those selected by the effective configuration.  |
| `-v`, `--verbose`         | Show detailed formatting results.                                                     |
| `-q`, `--quiet`           | Report only failures.                                                                 |
| `-h`, `--help`            | Shows help for the selected command and exits.                                        |
| `--completions <SHELL>`   | Prints a shell completion script for the named shell and exits.                       |
| `-V`, `--version`         | Prints the version and exits.                                                         |
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

The shipped config enables all four languages. `dotfile sync` installs it. `--add` and `--sync` use that single configuration file.

## Languages and settings

| Language | Built-in files | Dialects |
| --- | --- | --- |
| `conf` | `.conf`, `.config`, `.dotfile` | Filename-dependent plain/Hyprland and block formatting |
| `json` | `.json`, `.jsonc`, `.hujson`, `.jwcc` | `auto`, `json`, `jsonc`, `hujson` (`jwcc`) |
| `lua` | `.lua`, `.luau` | `auto`, `all`, `lua51`–`lua54`, `luajit`, `luau` |
| `markdown` (`md`) | `.md`, `.markdown`, `.mdown`, `.mkd` | `auto`, `commonmark`, `gfm` (`github`), `obsidian` |

| Global setting | Application |
| --- | --- |
| `final_newline` | All languages |
| `indent` | Conf blocks, JSON, Lua |
| `width` | Lua line wrapping and Markdown table layout |
| `quote_style` | Lua quote selection; JSON always emits double quotes; conf/Markdown preserve quoting |

| Language | Additional settings |
| --- | --- |
| Conf | `align`, `align_max`, `blank_lines` |
| JSON | `dialect`; `indent = -1` uses tabs, `0` emits compact JSON, `1`–`7` uses spaces |
| Lua | `dialect`, `indent_type`, `line_endings`, `call_parentheses`, `collapse_simple_statement`, `space_after_function_names`, `block_newline_gaps`, `sort_requires`, `verify` |
| Markdown | `dialect`, `table_style`, `autosize_table`, `heading_blank_lines`, `list_marker`, `trim_trailing_blank_lines` |

Markdown table dividers use three dashes by default, preserving alignment colons. Set `autosize_table = true` in the `markdown` block to size dividers to aligned columns; compact tables keep three dashes.

Known global settings apply where supported. Unsupported language-local settings are errors. Markdown width does not reflow prose. `auto` detects Lua/JSON dialects from filenames and Obsidian from vault ancestry. Markdown preserves frontmatter and protected content; JSONC/HuJSON editor mode preserves comments.

| Result                                                      | Exit status |
| --- | --- |
| Successful format / clean check                             | `0`         |
| Formatting, configuration, I/O failure or check differences | `1`         |
| CLI usage error                                             | `2`         |