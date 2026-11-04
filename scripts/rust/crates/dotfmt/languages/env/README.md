# dotfmt-env

Formats dotenv files (`.env`, `.env.*`, `*.env`, and a bare `env`) without
rewriting values.

## Invariants

- Quoted value bytes are copied verbatim: no unescaping, requoting, or interpolation. Quoted contents are never touched.
- Entries keep their written order; duplicate keys are never merged.
- `export` is preserved, including its original spacing.
- Whitespace around `=` is preserved byte-for-byte unless `align = true`. To a shell, `A=1`, `A= 1`, and `A = 1` are different kinds of line. Whitespace that only trails an empty value follows `trim_trailing_whitespace`.
- Trailing whitespace outside quotes is trimmed, except whitespace escaped by an odd number of backslashes: it is part of the value. A blank line that ends a shell continuation is preserved even when `blank_lines = 0`.
- Inline comments are recognized only when `#` follows unescaped whitespace,
  so `URL=https://host/#frag` and `TOKEN=foo\ #bar` stay values.
- A line starting with `#` immediately after a line ending in an odd number of
  backslashes is kept verbatim: the shell joins it to the previous word, so it
  may not be a comment at all.
- A double-quoted value may span lines; its newlines (including `CR`) are
  content and are never rewritten.
- A BOM is preserved. A BOM that was content in the middle of a file is never
  promoted to the file BOM by dropping leading blank lines. A whitespace-only
  file is returned unchanged.
- Lines that are not shaped like entries (a bare key, shell syntax) are kept
  verbatim. Unterminated quotes, trailing text after a quoted value, and a
  lone `CR` outside a quoted value are syntax errors.

## Settings

| Setting | Default | Values |
| --- | --- | --- |
| `indent` | `2` | `1..=16`, accepted for the shared block, unused |
| `width` | `80` | `1..=10000`, accepted for the shared block, unused |
| `final_newline` | `true` | `true`, `false` |
| `line_endings` | `unix` | `unix`, `windows` (between items only) |
| `blank_lines` | `1` | `0..=10` |
| `align` | `false` | `true` rewrites gaps as `key = value` and aligns `=` in contiguous entry groups |
| `align_max` | `24` | `0..=1000`, widest entry prefix considered for alignment |
| `format_comments` | `false` | add one space after `#` where missing (`#!` is left alone) |
| `trim_trailing_whitespace` | `true` | trim whitespace after items outside quoted values, except escaped whitespace |

`align` is opt-in because spaces around `=` change what a shell sees and are not
accepted by every shell or container env-file reader.
