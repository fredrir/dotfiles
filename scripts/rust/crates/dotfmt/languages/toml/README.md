# TOML engine

Internal library used by `dotfmt -l toml`. See [dotfmt](../../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l toml file.toml directory/
dotfmt -l toml --check directory/
dotfmt -e path/to/file.toml < file.toml
dotfile dev check -p dotfmt-toml
```

| Behavior | Value |
| --- | --- |
| Parser | [toml_edit](https://github.com/toml-rs/toml) 0.22; TOML 1.0, read for spans only, with a hand-written layout engine and printer |
| Normalized | Indentation, spacing around `=` and `,`, padding inside `[]` and `[[...]]` headers, padding around key dots, blank lines (at most `allowed_blank_lines`), array and inline table layout |
| Preserved | String literals of every style, multi-line string content, number and date-time spelling, quoted and dotted keys, header key spelling, comments, section order |
| Verification | Output must reparse and keep every decoded key, scalar value, array order, table and array-of-tables structure and every comment; otherwise the file remains unchanged |
| Arrays | One line when they fit and contain no comments, one item per line otherwise; comments always keep the multi-line layout |
| Inline tables | Kept on one line; only arrays nested in `{ ... }` may span lines, because TOML 1.0 forbids line breaks between inline-table entries |
| Encoding | UTF-8; an existing UTF-8 BOM is preserved; CRLF line breaks are normalized to LF, including inside multi-line strings |
| Configuration | `toml { ... }` in `dotfmt.dotfile`; global values and local overrides inherit; skip files with `excluded_files` |

Engine defaults below apply before configuration; see [shipped configuration](../../../../../../shared/tools/dotfmt.dotfile).

| Setting | Default | Values |
| --- | --- | --- |
| `width` | `80` | 1–10000; expands arrays that do not fit |
| `indent` | `2` | 1–16 spaces |
| `line_endings` | `unix` | `unix`, `windows` |
| `allowed_blank_lines` | `1` | 0–10 consecutive blank lines kept |
| `align_entries` | `false` | Boolean; align `=` within a group |
| `align_comments` | `true` | Boolean; align trailing comments within a group |
| `align_single_comments` | `true` | Boolean; align a group that holds one comment |
| `array_trailing_comma` | `true` | Boolean; trailing comma on multi-line arrays |
| `array_auto_expand` | `true` | Boolean; expand an array over `width` |
| `array_auto_collapse` | `true` | Boolean; collapse an array that fits |
| `compact_arrays` | `true` | Boolean; `[1, 2]`, otherwise `[ 1, 2 ]` |
| `compact_inline_tables` | `false` | Boolean; `{a = 1}`, otherwise `{ a = 1 }` |
| `compact_entries` | `false` | Boolean; drop the spaces around `=`, giving `a=1` |
| `inline_table_expand` | `true` | Boolean; expand arrays nested in an inline table over `width`, including inline tables written as array items |
| `indent_tables` | `false` | Boolean; indent a subtable header and its contents when an ancestor header was printed before it |
| `indent_entries` | `false` | Boolean; indent entries one level under their header |
| `reorder_keys` | `false` | Boolean; sort entries within a group |
| `format_comments` | `false` | Boolean; add a space after `#` |
| `final_newline` | `true` | Boolean |

`width`, `indent` and `final_newline` also apply from the shared global block. `quote_style` is accepted there and ignored, because TOML literals are never rewritten. Every other setting belongs to the `toml` block.

## Layout rules

Blank lines are kept up to `allowed_blank_lines` and never inserted, and blank lines at the start of a file are dropped. A group is a run of entries with nothing printed between them: a kept blank line, an own-line comment or a header starts a new group, so removing blank lines also merges the groups they separated. `align_entries`, `align_comments` and `reorder_keys` only work inside one group, and a group whose values span several lines is never aligned.

`reorder_keys` sorts a group by its decoded key path and keeps each entry's trailing comment with it. A comment on its own line starts a new group, so it stays in front of the entries it introduced and those entries are not moved; headers keep the document order. Comment placement is verified as a multiset in this mode, because sorting legitimately reorders comments.

Sections keep the order they were written in, including `[[table]]` elements interleaved with other sections, and dotted-key entries keep their written order (`a.b = 1`, `c = 2`, `a.d = 3` is not regrouped), so moving a comment never changes which entry it describes. A parent that exists only because a deeper header named it keeps no header of its own. `indent_tables` indents a subtable under the nearest ancestor header printed before it, so a parent written after its child cannot indent it.

## Limitations

Inline tables cannot be expanded: TOML 1.0 allows a line break inside `{ ... }` only within a value. `inline_table_expand` therefore expands the arrays nested in an over-long inline table, which is valid TOML, rather than the table itself. A comment inside `{ ... }` is a syntax error in TOML 1.0, so comments are only preserved around inline tables and inside their values.

`width` is a target, not a guarantee: a line holding a multi-line string keeps its contents as written, and a single scalar that does not fit is left alone. Nesting is limited to 79 levels by the embedded parser, counting arrays, inline tables, dotted keys, table headers and their combinations; deeper documents are rejected instead of formatted.
