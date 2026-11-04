# dotfmt-env

Formats dotenv files (`.env`, `.env.*`, `*.env`, and a bare `env`) without
rewriting values.

## Invariants

- Quoted value bytes are copied verbatim: no unescaping, requoting, or interpolation. Layout whitespace around `=` and trailing whitespace outside quotes are normalized; quoted contents are never touched.
- Entries keep their written order; duplicate keys are never merged.
- `export` is preserved, including its original spacing.
- Inline comments are recognized only when `#` follows whitespace, so
  `URL=https://host/#frag` stays a value.
- A double-quoted value may span lines; its newlines (including `CR`) are
  content and are never rewritten.
- A BOM is preserved. A whitespace-only file is returned unchanged.
- Lines that are not shaped like entries (a bare key, shell syntax) are kept
  verbatim. Unterminated quotes, trailing text after a quoted value, and a
  lone `CR` outside a double-quoted value are syntax errors.

## Settings

| Setting | Default | Values |
| --- | --- | --- |
| `indent` | `2` | `1..=16`, accepted for the shared block, unused |
| `width` | `80` | `1..=10000`, accepted for the shared block, unused |
| `final_newline` | `true` | `true`, `false` |
| `line_endings` | `unix` | `unix`, `windows` (between items only) |
| `blank_lines` | `1` | `0..=10` |
| `align` | `false` | `true` writes `key = value` and aligns `=` in contiguous entry groups |
| `align_max` | `24` | `0..=1000`, widest entry prefix considered for alignment |
| `format_comments` | `false` | add one space after `#` where missing (`#!` is left alone) |
| `trim_trailing_whitespace` | `true` | trim whitespace after items, outside quoted values |

`align` is opt-in because spaces around `=` are not accepted by every shell or
container env-file reader.
