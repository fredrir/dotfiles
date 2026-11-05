# dotfmt

## Commands

<!-- cli:commands:start -->
| Command  | Description                                                                                                                               |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `dotfmt` | Formats configured BibTeX, conf, env, ignore, ini, JSON, LaTeX, Lua, Markdown, plist, TOML and YAML files with one layered configuration. |
<!-- cli:commands:end -->

```sh
dotfmt .
dotfmt --check .
dotfmt -l json,markdown,yaml .
dotfmt -l lua --dialect luau script.luau
dotfmt -l latex,bib .
dotfmt -l latex --dialect context main.tex
dotfmt -l ini ~/.gitconfig
dotfmt -l ini --dialect ssh --stdin ~/.ssh/config < ~/.ssh/config
dotfmt -l ignore .gitignore .gitattributes
dotfmt -l env .env
dotfmt -l plist Info.plist
dotfmt -e note.md < note.md
dotfmt --explain ~/.ssh/config
dotfmt -c -l md note.md
dotfmt --add .
dotfmt --sync .
dotfmt --diagnose src/
dotfmt --fix src/
dotfmt --fix --diff src/
dotfmt --diagnose --json src/App.tsx
dotfmt --migrate prettier > dotfmt.dotfile
```

Editor mode uses the filename for language detection and local configuration, reads the buffer from stdin, and writes formatted text to stdout. Syntax errors in the buffer return the original input unchanged, with no diagnostics and exit status `0`. Configuration, I/O and internal errors also return the original input, but report the error and exit `1`. Use `--verbose` to diagnose syntax errors; `--check` retains normal failure status. It never writes the named file. Conform uses one formatter for all supported filetypes:

```lua
dotfmt = { command = "dotfmt", args = { "-e", "$FILENAME" } }
```

`--explain` prints what editor mode would do with a filename, one `key value` row per line, without reading the file. `language` and `dialect` name the engine; `skipped` is `unmapped`, `excluded` or `disabled`; each applied `config` follows, outermost first. A `.json` file reports dialect `auto`, since comments decide between JSON and JSONC. `--lang` and `--dialect` apply as in editor mode, and configuration errors fail the same way.

```text
language ini
dialect  ssh
config   ~/.config/dotfmt/dotfmt.dotfile
```

The [VS Code extension](../../scripts/node/packages/dotfmt/README.md) runs the `dotfmt-lsp` language server for formatting, ranges, on-type formatting, error diagnostics and code actions. Other editors can keep using editor mode (`--editor`) and `--explain`.

Selection uses filenames and `dotfmt.dotfile`, not the editor's filetype. A `.json` file with comments is formatted as JSONC, keeping its comments and trailing commas; `dialect = json` in the applicable `json` block drops them instead. Map extensionless names through that block's `include` rules.

## Repairs

Input that nearly parses is repaired before formatting, in file runs, `--stdin` and editor mode alike. `--verbose` lists each repair with its line and column; `--check` reports repairs as differences. `repair = false` turns repairs off globally or in one language block, and a nearer value wins as with any other setting.

| Language | Repairs |
| --- | --- |
| JSON | Missing, stray and trailing commas; `;` for `,`; missing `:` and `=` for `:`; brackets left open at the end, closed with the wrong bracket or repeated; `//`, `/* */` and `#` comments; Markdown code fences; single, curly and backtick quotes; unquoted keys; `True`, `False`, `None`, `NaN`, `Infinity`, `undefined`; hex numbers; control characters, lone surrogates, invalid UTF-8 and unusual whitespace in or around strings |
| JSONC, HuJSON, `.json` with comments | The JSON repairs, keeping comments and trailing commas; `#` comments become `//` |
| JSONL | The JSON repairs within each record, and closing brackets between records |
| TOML | Missing commas in arrays and inline tables, trailing commas in inline tables |
| YAML | Tab indentation, when no indented line uses spaces |
| Lua | Missing commas between table fields |

A repair never invents a value, so `{"a":}` stays an error.

## Flags

<!-- cli:flags:start -->
| Flag                      | Description                                                                                                                                                                                |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `-l`, `--lang <LANGUAGE>` | Select configured languages; repeat or comma-separate bib (bibtex, biblatex), conf, env (dotenv), ignore (gitignore), ini, json, latex (tex), lua, markdown (md), plist, toml, yaml (yml). |
| `--migrate <SOURCE>`      | Print a dotfmt.dotfile migrated from another formatter; currently prettier                                                                                                                 |
| `--check`                 | Report formatting differences without writing files.                                                                                                                                       |
| `--fix`                   | Apply safe fixes (unused imports, type-only imports, import ordering) and format                                                                                                           |
| `--fix-unsafe`            | Apply safe and unsafe fixes selected by the configuration, including code removal. Implies --fix                                                                                           |
| `--diff`                  | Print a unified diff instead of writing when used with --fix                                                                                                                               |
| `--diagnose`              | Report findings without writing or fixing                                                                                                                                                  |
| `--json`                  | Emit machine-readable JSON for --diagnose/--fix                                                                                                                                            |
| `-c`, `--clean`           | Format Markdown and strip all Markdown formatting; requires --lang markdown.                                                                                                               |
| `-a`, `--add`             | Offer dotfmt.dotfile to the target, asking before writing.                                                                                                                                 |
| `-s`, `--sync`            | Replace an existing target dotfmt.dotfile with the shared configuration.                                                                                                                   |
| `--dialect <DIALECT>`     | Override the dialect for one selected language.                                                                                                                                            |
| `-e`, `--editor`          | Read standard input for an editor.                                                                                                                                                         |
| `--explain <FILENAME>`    | Print the language, dialect and configuration files editor mode would use for a filename, or why it is skipped.                                                                            |
| `--stdin <FILENAME>`      | Read standard input as the named file, using its configuration and dialect.                                                                                                                |
| `--owns`                  | Read NUL-separated filenames and emit those selected by the effective configuration.                                                                                                       |
| `-v`, `--verbose`         | Show detailed formatting results and each repair.                                                                                                                                          |
| `-q`, `--quiet`           | Report only failures.                                                                                                                                                                      |
| `-h`, `--help`            | Shows help for the selected command and exits.                                                                                                                                             |
| `--completions <SHELL>`   | Prints a shell completion script for the named shell and exits.                                                                                                                            |
| `-V`, `--version`         | Prints the version and exits.                                                                                                                                                              |
<!-- cli:flags:end -->

## Configuration

| Precedence, low to high | Source |
| --- | --- |
| 1 | Engine defaults |
| 2 | `${XDG_CONFIG_HOME:-$HOME/.config}/dotfmt/dotfmt.dotfile` |
| 3 | Ancestor `dotfmt.dotfile` files, outermost to nearest |
| 4 | CLI dialect override |

Each layer applies globals, then language settings. Only specified values replace inherited values. A nearer global value replaces an outer language value. Empty language blocks enable inherited defaults; `enabled = false` disables a language. If no language block is declared anywhere, the shipped default set is enabled: every config/data/markup language including TypeScript, CSS and HTML, but not JavaScript. Any explicit language block keeps the legacy subset semantics. `--lang` selects configured languages; it does not enable disabled or absent blocks.

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

env {}
ignore {
  align = true
}
ini {
  align = true
}
plist {}

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

The shipped config enables fifteen languages: the twelve config/data languages plus TypeScript, CSS and HTML/embedded markup. JavaScript is a separate, opt-in language because adding it by default would start formatting every `.js`/`.jsx` file on upgrade. The same defaults apply when no language block is declared at all, so `dotfmt` works without writing a config first. `dotfile sync` installs the shipped config; `--add` and `--sync` use that single configuration file.

Discovery honors the nearest `.prettierignore` for Prettier compatibility. Explicitly named files still require `--lang` when their filename is unfamiliar; use `included_files` for custom mappings.

## TypeScript, CSS, HTML, Tailwind and fixes

TypeScript/TSX, JavaScript/JSX, CSS/SCSS/Sass/Less and HTML/Vue/Svelte/Astro use the same configuration and selection rules as every other language. The formatter is quiet by design: syntax errors leave the file untouched, `--editor` passes the original buffer through and exits `0`, and only dotfmt's own configuration/I/O/internal failures are reported.

dotfmt is a formatter, not a linter. In the editor it publishes errors only: syntax/configuration failures, plus project `tsc` errors when `dotfmt.typeAware` is on. Findings never become squiggles, and dotfmt publishes no Hint, Information or Warning diagnostics. `--diagnose`, `--fix`, `--fix-unsafe`, quick fixes and the `source.fixAll.dotfmt`/`source.organizeImports.dotfmt`/`source.format.dotfmt` code actions remain the lint and repair surface.

`--diagnose` reports findings without editing and exits `1` when findings remain. `--fix` applies only `Safe` fixes; `--fix-unsafe` also applies `Unsafe` fixes and requires an explicit flag. Both intersect applicability with the configured fix categories, so `--fix-unsafe` never removes exports unless `remove_unused_exports = true` and never removes locals with side-effecting initializers unless `remove_impure_locals = true`. Quick fixes cover unused imports (a bare `import "module"` preserves side effects), type-only import conversion, organize imports and unsafe unused-variable removal (never applied by plain `--fix`). Exported declarations, `.d.ts` files, classic JSX factory bindings, decorator metadata, impure initializers and files containing `eval`/`new Function` are protected. Fixes are applied as byte-range edits, then the result is formatted, reparsed and re-analyzed up to a bounded fixpoint. `--fix --diff` prints the exact change instead of writing it.

Formatting applies fixes too when `dotfmt.dotfile` enables them. All options default to off, so a plain format only changes layout:

| TypeScript/JavaScript option | Default | Effect |
| --- | --- | --- |
| `fixes` | `off` | Preset for format-time fixes: `off`, `safe` or `unsafe` |
| `remove_unused_imports` | `false` | Remove unused imports; a bare `import "module"` keeps its side effect |
| `convert_type_only_imports` | `false` | Convert imports used only as types to `import type` |
| `organize_imports` | `false` | Sort and group import runs, applying their import fixes |
| `remove_unused_exports` | `false` | Remove unused exports (unsafe, needs project analysis) |
| `remove_unused_locals` | `false` | Remove unused local variables (unsafe) |
| `remove_impure_locals` | `false` | Remove unused locals with side-effecting initializers (unsafe, opt-in) |
| `remove_unused_parameters` | `false` | Diagnose-only; no safe parameter-removal fix exists |

`fixes = safe` implies `remove_unused_imports`, `convert_type_only_imports` and `organize_imports`; `fixes = unsafe` adds `remove_unused_locals`. It never implies `remove_unused_exports`, which changes public API and must be set explicitly. Unused locals with side-effecting initializers are retained under `fixes = unsafe` unless `remove_impure_locals = true`; pure locals still follow `fixes = unsafe`. `remove_unused_parameters` is diagnose-only because there is no safe parameter-removal fix. A `true` option applies even with `fixes = off`. Selected fixes run before the final format, at most 3 passes, and reformatting is idempotent. Fixes apply to full-document formatting and `willSaveWaitUntil` only; range, ranges and on-type formatting never apply fixes, and `--check` reports layout differences only. Safe fixes run in untrusted workspaces; destructive/export/type-aware categories and project JavaScript require trust.

`type_aware = true` (default) runs the project's `tsc --noEmit --pretty false --locale en --project <tsconfig>` once per reachable project config, expands TypeScript project references, and keeps only errors. The cache fingerprints path aliases, linked workspace packages and referenced sources; TS6305/TS6310 become one actionable build hint instead of published errors. Unused-only codes 6133, 6138, 6192, 6196, 6198, 6199, 6205, 7027 and 7028 are filtered, so unused code is never an editor error; `--diagnose` reports it instead. `remove_unused_exports = true` lets formatting remove project-wide unused exports; `--diagnose` reports them without removing anything. Type information never blocks formatting.

`tsc` discovery: `DOTFMT_TSC`, an explicit `dotfmt.typescript.tsdk`, the project's `node_modules/typescript/lib/tsc.js` under Node, `node_modules/.bin/tsc`, `@typescript/native-preview`/`tsgo`, then `tsc` on `PATH`. The resolved compiler and version are logged once per project; failures clear stale diagnostics, and three consecutive failures or timeouts back off until the compiler or configuration changes.

Tailwind sorting supports TSX `className`/`class`, `cn`/`clsx`/`cva`/`twMerge`, template literals, CSS `@apply`, and HTML/markup class attributes. `tailwind = auto` engages only when the file or project uses Tailwind; `tailwind_order = project` requires the project's own Tailwind and fails with a configuration diagnostic if it cannot be resolved. The project bridge runs the official `prettier-plugin-tailwindcss` sorter through Node, caches complete per-theme candidate sets under `${XDG_CACHE_HOME:-~/.cache}/dotfmt/tailwind`, invalidates them with a dependency digest, and falls back to the Rust pattern sorter for `auto`/`rust`. `tailwind_merge` is opt-in because collapsing conflicting utilities can change rendering.

In VS Code, the extension uses LSP for document/range/ranges formatting, `;`/`}` on-type formatting, error diagnostics, quick fixes, `source.fixAll.dotfmt`, `source.organizeImports.dotfmt` and `source.format.dotfmt`. Language settings are resource-scoped, so every folder in a multi-root workspace resolves its own configuration, compiler scope and fix mode:

| VS Code setting | Default | Scope | Effect |
| --- | --- | --- | --- |
| `dotfmt.typeAware` | `true` | resource | Publish project `tsc` errors |
| `dotfmt.diagnostics.syntax` | `true` | resource | Publish syntax and configuration errors; `false` stops publication but formatting failures still log and set status |
| `dotfmt.diagnostics.tscScope` | `"open"` | resource | `open` publishes errors for open documents only; `project` publishes every project error |
| `dotfmt.diagnostics.debounceMs` | `250` | resource | Diagnostic debounce, 50–2000 ms |
| `dotfmt.typescript.tsdk` | `""` | resource | TypeScript install used for `tsc`; empty discovers the project's compiler |
| `dotfmt.fixes.onSave` | `"off"` | resource | Run the selected `off`/`safe`/`unsafe` fix mode through `textDocument/willSaveWaitUntil`, before writing |
| `dotfmt.format.*` | unset | resource | Native overlay for every generated formatting option; unset inherits `dotfmt.dotfile` |
| `dotfmt.formatExtra` | `{}` | resource | Free-form overlay using the dotfile shape; lower precedence than `dotfmt.format.*` |
| `dotfmt.codeActions.unusedExports` | `true` | resource | Offer the unsafe unused-export quick fix from project analysis in trusted workspaces |
| `dotfmt.server.path` | `""` | machine-overridable | Language server binary; empty uses the bundled one |
| `dotfmt.path` | `""` | machine-overridable | CLI binary for manual checks and `dotfmt.doctor`; empty uses the bundled one |
| `dotfmt.trace.server` | `"off"` | window | `off`/`messages`/`verbose` LSP trace in its own output channel |

`dotfmt.doctor` prints the server and CLI paths and versions, the resolved configuration chain, the `tsc` compiler and version, workspace trust, and a copyable summary. Trust is explicit: the server starts untrusted, and project `tsc`, project JavaScript (Tailwind order) and unsafe fixes run only when the client or `DOTFMT_TRUSTED` grants trust.

`--migrate prettier` reads `package.json#prettier` and JSON `.prettierrc` files and prints a starting configuration; JavaScript configs and unsupported plugins are reported, not guessed. The nearest `.prettierignore` is honored during discovery.

## Languages and settings

| Language | Built-in files | Dialects |
| --- | --- | --- |
| `bib` | `.bib` | None; BibTeX and BibLaTeX syntax share one grammar |
| `conf` | `.conf`, `.config`, `.dotfile` | Filename-dependent plain/Hyprland and block formatting |
| `env` (`dotenv`) | `.env`, `.env.*`, `*.env`, `env` | None; values, exports, order and duplicate keys are preserved |
| `ignore` (`gitignore`) | `.gitignore`, `.ignore`, `ignore`, `.prettierignore`, `.vscodeignore`, `.gitattributes`, `.git/info/exclude`, `.git/info/attributes` | `auto`, `git` (`gitignore`), `ignore` (`ripgrep`), `prettier` (`prettierignore`), `vscode` (`vscodeignore`), `attributes` (`gitattributes`) |
| `ini` | `.ini`, `.service`, `.timer`, `.path`, `.socket`, `.target`, `.mount`, `.automount`, `.swap`, `.slice`, `.scope`, `.device`, `.network`, `.netdev`, `.link`, `.gitconfig`, `.gitmodules`, `.git/config`, `git/config`, `.editorconfig`, `ssh_config`, `sshd_config`, `.ssh/config`, `.ssh/config.d/*` | `auto`, `generic`, `git` (`gitconfig`), `editorconfig`, `systemd`, `ssh` |
| `json` | `.json`, `.jsonc`, `.hujson`, `.jwcc`, `.jsonl`, `.ndjson`, `.jsonlines` | `auto`, `json`, `jsonc`, `hujson` (`jwcc`), `jsonl` (`ndjson`, `jsonlines`) |
| `latex` (`tex`) | `.tex`, `.ltx`, `.latex`, `.sty`, `.cls`, `.dtx`, `.ins`, `.def`, `.clo`, `.lco`, `.fd`, `.bbx`, `.cbx`, `.lbx`, `.pgf`, `.tikz`, `.ctx`, `.mkii`, `.mkiv`, `.mkvi`, `.mkxl` | `auto`, `latex` (`pdftex`, `xelatex`, `lualatex`, `expl3`, `beamer`, `tikz`), `plain` (`tex`, `optex`), `context` (`ctx`, `mkiv`, `lmtx`) |
| `lua` | `.lua`, `.luau` | `auto`, `all`, `lua51`–`lua54`, `luajit`, `luau` |
| `markdown` (`md`) | `.md`, `.markdown`, `.mdown`, `.mkd` | `auto`, `commonmark`, `gfm` (`github`), `obsidian` |
| `plist` | `.plist` | None; XML plists are formatted, binary and OpenStep plists pass through unchanged |
| `toml` | `.toml` | None |
| `yaml` (`yml`) | `.yaml`, `.yml` | None |
| `typescript` (`ts`, `tsx`) | `.ts`, `.tsx`, `.mts`, `.cts` | `auto`, `ts`, `tsx` |
| `javascript` (`js`, `jsx`) | `.js`, `.jsx`, `.mjs`, `.cjs` | `auto`, `js`, `jsx`; opt-in block |
| `css` | `.css`, `.scss`, `.sass`, `.less`, `.pcss` | `auto`, `css`, `scss`, `sass`, `less` |
| `html` | `.html`, `.htm`, `.xhtml`, `.vue`, `.svelte`, `.astro` | `auto`, `html`, `vue`, `svelte`, `astro`, `angular`, `xml` |

| Global setting | Application |
| --- | --- |
| `final_newline` | All languages |
| `indent` | Bib, Conf `.dotfile` blocks, Ini (Git and SSH), JSON, LaTeX, Lua, Plist, TOML, YAML, TypeScript, CSS, HTML |
| `width` | Bib and LaTeX wrapping, Lua line wrapping, Markdown table layout, YAML flow collections, TOML arrays/inline tables, and TypeScript/CSS/HTML line breaking; accepted and ignored by Env, Ignore, Ini and Plist |
| `quote_style` | Lua and YAML quote selection, and TypeScript/HTML/CSS string quoting; `auto`, `auto-prefer-double` and `auto-prefer-single` resolve to the engine's preferred quote; JSON always emits double quotes; Bib, conf, env, ignore, ini, LaTeX, Markdown, plist and TOML preserve quoting and ignore it |
| `repair` | All languages; defaults to `true`; the new config engines do not repair, so malformed input is reported instead |

| Language | Additional settings |
| --- | --- |
| Bib | `indent_type`, `line_endings`, `blank_lines`, `align`, `align_max`, `trailing_comma`, `wrap`, `wrap_min`, `field_name_case`, `entry_type_case` |
| Conf | `align`, `align_max`, `blank_lines` (`.dotfile` blocks only; `.conf`/`.config` files use dialect layout and `final_newline`) |
| Env | `line_endings`, `blank_lines`, `align`, `align_max`, `format_comments`, `trim_trailing_whitespace` |
| Ignore | `dialect`, `line_endings`, `blank_lines`, `trim_trailing_whitespace`, `trim_leading_whitespace`, `format_comments`, `align`, `align_max` |
| Ini | `dialect`, `indent`, `indent_type`, `line_endings`, `blank_lines`, `align`, `align_max`, `format_comments` |
| JSON | `dialect`; `indent = -1` uses tabs, `0` emits compact JSON, `1`–`7` uses spaces; JSONL ignores `indent` |
| LaTeX | `dialect`, `indent_type`, `line_endings`, `wrap`, `wrap_min`, `wrap_chars`, `format_tables`, `blank_lines`, `verify`, `lists`, `verbatims`, `no_indent_envs` |
| Lua | `dialect`, `indent_type`, `line_endings`, `call_parentheses`, `collapse_simple_statement`, `space_after_function_names`, `block_newline_gaps`, `sort_requires`, `verify` |
| Markdown | `dialect`, `table_style`, `autosize_table`, `blank_lines`, `heading_blank_lines`, `list_marker`, `trim_trailing_blank_lines` |
| Plist | `indent`, `indent_type`, `line_endings`, `blank_lines`, `data_width`, `prolog`, `self_closing`, `sort_keys` (must be `false`) |
| TOML | `line_endings`, `allowed_blank_lines`, `align_entries`, `align_comments`, `align_single_comments`, `array_trailing_comma`, `array_auto_expand`, `array_auto_collapse`, `compact_arrays`, `compact_inline_tables`, `compact_entries`, `inline_table_expand`, `indent_tables`, `indent_entries`, `reorder_keys`, `format_comments` |
| YAML | `line_endings`, `indent_sequences`, `trailing_comma`, `brace_spacing`, `bracket_spacing`, `dash_spacing`, `prefer_single_line`, `format_comments` |
| TypeScript | `dialect`, `indent_type`, `line_endings`, `quote_style`, `jsx_quote_style`, `quote_properties`, `semicolons`, `trailing_commas`, `arrow_parentheses`, `bracket_spacing`, `bracket_same_line`, `attribute_position`, `expand`, `jsdoc`, `embedded_css`, `sort_imports`, `import_order`, `import_internal`, `import_side_effects`, `import_newlines`, `import_partition_by_newline`, `import_partition_by_comment`, `type_aware`, `fixes`, `remove_unused_imports`, `remove_unused_exports`, `remove_unused_locals`, `remove_impure_locals`, `remove_unused_parameters`, `convert_type_only_imports`, `organize_imports`, `verify`, `tailwind`, `tailwind_order`, `tailwind_dedupe`, `tailwind_merge`, `tailwind_functions`, `tailwind_attributes`, `tailwind_stylesheet`, `tailwind_config` |
| JavaScript | Same settings as TypeScript; add `javascript {}` to opt in |
| CSS | `dialect`, `indent_type`, `line_endings`, `hex_case`, `quotes`, `format_comments`, `align_comments`, `single_line_block_threshold`, `tailwind`, `tailwind_order`, `tailwind_dedupe`, `tailwind_merge`, `tailwind_stylesheet`, `tailwind_config` |
| HTML | `dialect`, `indent_type`, `line_endings`, `quotes`, `format_comments`, `closing_bracket_same_line`, `max_attrs_per_line`, `script_indent`, `style_indent`, `tailwind`, `tailwind_dedupe`, `tailwind_stylesheet`, `tailwind_config` |

Markdown table dividers use three dashes by default, preserving alignment colons. Set `autosize_table = true` in the `markdown` block to size dividers to aligned columns; compact tables keep three dashes.

LaTeX verbatim environments and inline `\verb`, `\lstinline`, `\mintinline`, `\url`, `\path`, `\href` and file arguments are protected byte-for-byte before layout. A `\begin`, `\end`, `\item` or sectioning command that shares a line with non-whitespace text is split off with a terminating `%`, so the newline adds no space token; `% tex-fmt: off`/`% tex-fmt: on` regions are preserved byte-for-byte. `dialect = context` maps `\start...`/`\stop...` environments onto the same indentation engine; `plain` leaves environment syntax alone. The formatter rejects nesting deeper than 100 levels. `format_tables` aligns `tabular` rows only; width wraps text and comments but never rewrites protected content. `verify = true` formats a second time and requires the result to settle.

Bib output keeps block and field order, field and entry keys, value delimiters, macro names used in values and every value byte; it never deduplicates or sorts. Field and entry type case are configurable, and malformed input is reported instead of repaired: a bare `@` that does not start a block, an unterminated `@comment`, a comment between concatenated value parts and a comment before the end of `@preamble` are located syntax errors. `@string` definition names follow `field_name_case`; macro uses keep their spelling. `line_endings = windows` also rewrites newlines inside multi-line values. `wrap` only replaces existing spaces inside values with line breaks and continuation indentation; verification ignores those inserted whitespace changes.

Known global settings apply where supported. Unsupported language-local settings are errors. Markdown width does not reflow prose. Markdown drops leading blank lines and collapses runs longer than `blank_lines` outside code and HTML blocks. `auto` detects Lua/JSON dialects from filenames, Ini and Ignore dialects from filenames and paths, and Obsidian from vault ancestry. Markdown preserves frontmatter and protected content; `--clean` with `--lang markdown` formats and then strips all Markdown formatting, leaving plain text, code and table cells. JSONL reads any whitespace-separated values, like `jq -c`, and writes one compact record per line, dropping blank lines. YAML output must parse to the same values as its input, otherwise the file is left unchanged. TOML output must preserve values and comments or the file is left unchanged.

Ini preserves section order, duplicate keys, comments, quoting, inline marker bytes and continuation bytes; indentation, separator padding, trailing layout whitespace, blank runs outside Generic continuations, SSH `Keyword = Value` normalization and full-line comment spacing change. Ignore preserves every pattern and attribute byte, negation, ordering and BOM; only blank runs, safe trailing whitespace, comment spacing and `.gitattributes` attribute alignment change. A bare `ignore` is treated as Git (`core.excludesFile`); `.ignore` is the ripgrep dialect. Env never rewrites quoted value bytes, quotes, `export`, ordering or duplicate keys; spacing around `=` is preserved by default because shell-sourced `.env` files are sensitive to it; `align` opts into normalized/equally aligned `=`, and unquoted trailing whitespace follows `trim_trailing_whitespace`. Plist formats XML plists only; comments, CDATA, entity spelling, text bytes, key order and duplicate keys are preserved, while binary and OpenStep plists pass through unchanged.

| Result | Exit status |
| --- | --- |
| Successful format / clean check | `0` |
| Editor syntax error, without `--verbose` or `--check` | `0` (original input) |
| Editor configuration, I/O or internal error | `1` (original input) |
| Formatting, configuration, I/O failure or check differences | `1` |
| `--diagnose` with findings | `1` |
| `--diagnose` clean, or `--fix` completed | `0` |
| CLI usage error | `2` |