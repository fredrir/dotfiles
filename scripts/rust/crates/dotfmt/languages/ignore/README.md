# Ignore engine

Internal library used by `dotfmt -l ignore`. See [dotfmt](../../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l ignore .gitignore .ignore .prettierignore .vscodeignore .gitattributes
dotfmt -l ignore --check .
dotfile dev check -p dotfmt-ignore
```

| Behavior | Value |
| --- | --- |
| Parser | Purpose-built lossless splitter; one line model per dialect, patterns and attributes stay byte slices |
| Built-in files | `.gitignore`, `*.gitignore`, `.ignore`, `ignore`, `*.ignore`, `.prettierignore`, `.vscodeignore`, `.gitattributes`, `*.gitattributes`, `$GIT_DIR/info/exclude`, `$GIT_DIR/info/attributes` |
| Preserved | Line order, duplicates, negation lines, leading whitespace, internal whitespace, escapes, trailing backslashes, BOM, lone CR, opaque lines |
| Normalized | CRLF to LF, blank runs, comment spacing (opt-in), trailing whitespace the dialect ignores, attribute separators and alignment |
| Verification | The output is reparsed and compared with the input fingerprint; a pattern, attribute or order change is an internal error and the file stays unchanged |
| Malformed input | Lines whose fields cannot be split safely are opaque and printed byte for byte; non-UTF-8 input is a syntax error |
| Encoding | UTF-8; an existing BOM is preserved; only CRLF is normalized, a lone CR is pattern data |
| Configuration | `ignore { ... }` in `dotfmt.dotfile`; global width, indent and quote style are accepted and ignored |

Engine defaults below apply before configuration.

| Setting | Default | Values |
| --- | --- | --- |
| `dialect` | `auto` | `auto`, `git` (`gitignore`), `ignore` (`ripgrep`), `prettier` (`prettierignore`), `vscode` (`vscodeignore`), `attributes` (`gitattributes`, `attr`) |
| `line_endings` | `unix` | `unix`, `windows`; only newline bytes are rewritten |
| `final_newline` | `false` | Boolean; not added to whitespace-only files or after a final lone CR |
| `blank_lines` | `1` | 0–10 consecutive blank lines; leading and trailing blank runs are removed |
| `trim_trailing_whitespace` | `true` | Boolean; never touches a trailing run protected by a backslash |
| `trim_leading_whitespace` | `false` | Boolean; only effective for `attributes`, where leading whitespace is insignificant |
| `format_comments` | `false` | Boolean; add or collapse the separator after `#` where safe |
| `align` | `true` | Boolean; attribute column in `.gitattributes`, a no-op elsewhere |
| `align_max` | `24` | 0–1000; widest pattern to align, `0` for no cap; a wider pattern makes the group fall back to one space |

`width`, `indent` and `quote_style` apply from the shared global block and are ignored; `final_newline` is honoured from either block.

## Dialects

| Dialect | Trailing whitespace | Blank line | Comments | Notes |
| --- | --- | --- | --- | --- |
| `git` | trailing spaces only | empty or spaces | `#` at byte 0 | A tab is pattern data; `  #x` is a pattern |
| `ignore` | spaces and tabs | spaces and tabs | `#` at byte 0 | Mirrors the `ignore` crate's `trim_right` |
| `prettier` | spaces and tabs | spaces and tabs | `#` at byte 0 | A trailing backslash is kept instead of dropping the line |
| `vscode` | trailing spaces only | empty or spaces | `#` at byte 0 | Conservative until VS Code's parser varies from gitignore |
| `attributes` | spaces and tabs | spaces and tabs | `#` after leading whitespace | Quoted patterns, `[attr]` macros, attribute values |

Bare `ignore` is treated as the `git` dialect because it is usually installed as `core.excludesFile`; `.ignore` is the ripgrep dialect. This is conservative for both readers: the formatter never removes a trailing tab that Git treats as pattern data, and a ripgrep reader trims it on its own.

Trailing whitespace runs immediately preceded by a backslash are never edited in any dialect. This covers `b.txt\ ` in gitignore, `a\  ` in Prettier and ambiguous escapes in `.gitattributes`. Patterns and attributes are never reordered, deduplicated, unescaped or rewritten. Comment formatting only changes the separator after a lone `#`; `###`, `#!` and `#` are left alone. Whitespace-only input is kept as it stands, apart from configured line endings and BOM handling, so a non-empty file is never emptied.
