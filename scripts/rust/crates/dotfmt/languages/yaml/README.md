# YAML engine

Internal library used by `dotfmt -l yaml`. See [dotfmt](../../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l yaml file.yaml directory/
dotfmt -l yaml --check directory/
dotfmt -e path/to/file.yml < file.yml
dotfile dev check -p dotfmt-yaml
```

| Behavior | Value |
| --- | --- |
| Parser | [saphyr-parser](https://github.com/saphyr-rs/saphyr); YAML 1.2, no subprocess |
| Normalized | Indentation, sequence dashes, spacing around `:` and `#`, blank lines (at most one), flow collection layout, quote style |
| Preserved | Scalar text, block scalar content, comments, anchors, aliases, tags, explicit keys, directives and document markers |
| Verification | Output must produce the same parse events as the input; otherwise the file remains unchanged |
| Flow collections | Kept as written when they contain comments, empty values or explicit keys |
| Final newline | A trailing block scalar keeps its final line break as written |
| Encoding | UTF-8; an existing UTF-8 BOM is preserved; line breaks are normalized |
| Configuration | `yaml { ... }` in `dotfmt.dotfile`; global values and local overrides inherit; skip files with `excluded_files` |

Engine defaults below apply before configuration; see [shipped configuration](../../../../../../shared/tools/dotfmt.dotfile).

| Setting | Default | Values |
| --- | --- | --- |
| `width` | `80` | 1–10000; wraps flow collections; never reflows scalars |
| `indent` | `2` | 1–16 spaces |
| `quote_style` | `auto-prefer-double` | `auto`, `auto-prefer-double`, `auto-prefer-single` pick the quotes needing fewer escapes; `double` (`force-double`), `single` (`force-single`) always convert unless single quotes cannot hold the escapes; single-line strings only |
| `line_endings` | `unix` | `unix`, `windows` |
| `indent_sequences` | `true` | Boolean; indent block sequences inside mappings |
| `trailing_comma` | `false` | Boolean; wrapped flow collections |
| `brace_spacing` | `false` | Boolean; `{ a: 1 }` |
| `bracket_spacing` | `false` | Boolean; `[ 1 ]` |
| `dash_spacing` | `one-space` | `one-space`, `indent` |
| `prefer_single_line` | `false` | Boolean; join multi-line flow collections that fit |
| `format_comments` | `false` | Boolean; add a space after `#` |
| `final_newline` | `true` | Boolean |