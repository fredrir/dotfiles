# BibTeX engine

Internal library used by `dotfmt -l bib`. See [dotfmt](../../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l bib references.bib notes/
dotfmt -l bib --check references.bib
dotfmt -e path/to/references.bib < references.bib
dotfile dev check -p dotfmt-bib
```

| Behavior | Value |
| --- | --- |
| Parser | Purpose-built lossless BibTeX/BibLaTeX parser; byte spans, never decoded values |
| Built-in files | `.bib` |
| Preserved | Block and field order, duplicate fields, entry keys, macro names, value delimiters, every value byte, `@comment`/`@preamble` text, comments |
| Normalized | Indentation, alignment, `=` and comma spacing, entry type and field name case, block separation, line endings |
| Verification | Output is reparsed and compared with the input fingerprint; any mismatch is an internal error and the file stays unchanged |
| Malformed input | Reported as a syntax error with a location; the file stays unchanged |
| Encoding | UTF-8; an existing BOM is preserved; CRLF and CR are normalized, then written as configured |
| Configuration | `bib { ... }` in `dotfmt.dotfile`; global values and local overrides inherit |

Engine defaults below apply before configuration; see [shipped configuration](../../../../../../shared/tools/dotfmt.dotfile).

| Setting | Default | Values |
| --- | --- | --- |
| `width` | `80` | 1–10000; preferred line width when `wrap = true` |
| `indent` | `2` | 1–16 columns per level |
| `indent_type` | `spaces` | `spaces`, `tabs` |
| `line_endings` | `unix` | `unix`, `windows` |
| `final_newline` | `false` | Boolean |
| `blank_lines` | `1` | 0–10 blank lines between blocks |
| `align` | `true` | Boolean; align `=` within an entry |
| `align_max` | `24` | 0–1000; widest field name to align, `0` for no cap; longer groups fall back to one space |
| `trailing_comma` | `false` | Boolean; comma after the last field |
| `wrap` | `false` | Boolean; break long values only at existing spaces |
| `wrap_min` | width − 10 | 1–10000; preferred minimum prefix before a break |
| `field_name_case` | `lower` | `preserve`, `lower` (`lowercase`), `upper` (`uppercase`) |
| `entry_type_case` | `lower` | `preserve`, `lower` (`lowercase`), `upper` (`uppercase`) |

`width`, `indent` and `final_newline` also apply from the shared global block. `quote_style` is accepted there and ignored.

## Syntax

| Form | Support |
| --- | --- |
| Entries | `@type{key, ...}` and `@type(key, ...)`; type and field names are case-insensitive |
| Values | Braced, quoted, numeric, macro, and `#` concatenation |
| Blocks | `@comment`, `@preamble`, `@string` |
| Comments | `%` to end of line outside values; `%` inside a value is literal |
| Delimiters | Braces and parentheses are preserved exactly |
| Keys | Case and braces/parentheses are preserved; whitespace is rejected |

BibTeX braces are counted literally: a backslash does not escape `{`, `}`, or `"` for value boundaries. A value that cannot be parsed is never repaired. `@string` order is preserved because BibTeX requires a macro to be defined before use. Blocks are never sorted or deduplicated.

## Wrapping

`wrap` only replaces existing spaces inside values with a line break and one extra indentation level. It never splits a token, never breaks immediately after `%`, and leaves protected `@comment`/`@preamble` text alone. Verification compares entry values with whitespace collapsed when wrapping is enabled, so only inserted whitespace can differ; every non-whitespace byte must still match.
