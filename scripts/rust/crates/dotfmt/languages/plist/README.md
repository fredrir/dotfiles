# plist engine

Internal library used by `dotfmt -l plist`. See [dotfmt](../../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l plist file.plist directory/
dotfmt -l plist --check directory/
dotfmt -e path/to/file.plist < file.plist
dotfile dev check -p dotfmt-plist
```

| Behavior | Value |
| --- | --- |
| Parser | [quick-xml](https://github.com/tafia/quick-xml) 0.42; raw events are sliced back out of the source, so text, attributes and markup keep their exact spelling |
| Normalized | Inter-element whitespace only: indentation, one element per line, `<key>` and its value on separate lines, blank runs (at most `blank_lines`), line endings on those breaks, the final newline |
| Preserved | Text and `<data>` content bytes, entity spellings (`&amp;`, `&#38;`), CDATA, attribute order and spelling, comments, processing instructions, key order, duplicate keys, the XML declaration and DOCTYPE |
| Verification | Output must reparse and keep every raw text value, decoded `<data>` payload, container order, per-dictionary last-wins map, attributes and every comment; otherwise the file remains unchanged |
| Repairs | None: XML must parse. Binary `bplist00` and OpenStep ASCII property lists return `Output::Verbatim`, and the caller leaves them untouched |
| Encoding | UTF-8; an existing UTF-8 BOM is preserved; layout line breaks follow `line_endings`, while line breaks inside text and data content are never rewritten |
| Configuration | `plist { ... }` in `dotfmt.dotfile`; global values and local overrides inherit; skip files with `excluded_files` |

Engine defaults below apply before configuration; see [shipped configuration](../../../../../../shared/tools/dotfmt.dotfile).

| Setting | Default | Values |
| --- | --- | --- |
| `indent` | `2` | 1–16 spaces per level |
| `indent_type` | `spaces` | `spaces`, `tabs`; tabs use one tab per level |
| `width` | `80` | 1–10000; accepted from the shared global block, unused in v1 |
| `final_newline` | `true` | Boolean |
| `line_endings` | `unix` | `unix`, `windows` |
| `blank_lines` | `1` | 0–10 consecutive blank lines kept between elements |
| `data_width` | `0` | `0` preserves `<data>` content; 1–120 re-wraps valid base64 at that many columns |
| `prolog` | `preserve` | `preserve`, `normalize`; `normalize` rewrites the XML declaration and DOCTYPE to Apple's spelling and inserts a missing DOCTYPE |
| `self_closing` | `preserve` | `preserve`, `always`, `never`; controls empty elements such as `<true/>` and `<dict></dict>`; empty `<data></data>` stays paired because Apple rejects `<data/>` |
| `sort_keys` | `false` | Only `false` in v1: reordering keys would change duplicate-key shadowing |

`width`, `indent` and `final_newline` also apply from the shared global block. `quote_style` is accepted there and ignored, because plist values are never rewritten. Every other setting belongs to the `plist` block.

## Layout rules

`<plist>` is printed at column zero, and its single value follows directly below it at column zero, matching Apple's output. Nested containers indent their children by one level, and every element starts on its own line. Blank lines between siblings are kept up to `blank_lines`; leading and trailing blank lines inside a container are dropped.

Text elements are printed from their source bytes: `<string>  padded  </string>`, multi-line strings, entities and CDATA are copied unchanged, even when their content contains line breaks. `<data>` is copied unchanged unless `data_width` is non-zero, in which case the decoded bytes are re-encoded and wrapped under an indented `<data>` element. Either way the decoded payload is verified.

Comments and processing instructions between elements are printed on their own line at the surrounding indentation. Comments and processing instructions before and after `<plist>` are kept in place. `prolog = normalize` only rewrites the XML declaration and DOCTYPE; comments and processing instructions are never touched.

## Limitations

Only XML property lists are formatted. Binary `bplist00` files and OpenStep ASCII plists (`{ key = value; }`) are returned as `Output::Verbatim`; the formatter cannot represent them as text and does not convert between encodings.

XML element names are matched exactly as Apple writes them, so unknown or differently-cased elements are rejected instead of being guessed at. `<plist>` must contain exactly one value, `<dict>` must alternate `<key>` and value elements, and `<key>` is only valid inside a dictionary. Nesting deeper than 100 containers is rejected. Malformed XML, malformed base64 in `<data>`, and property-list shape errors are reported as syntax diagnostics with a line and column.

`sort_keys = true` is rejected in v1. Sorting is the only operation that would change which of two duplicate keys wins, and the formatter preserves duplicate keys in written order instead.
