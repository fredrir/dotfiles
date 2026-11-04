# INI engine

Internal library used by `dotfmt -l ini`. See [dotfmt](../../../../../../docs/cli/dotfmt.md) for configuration, selection and CLI behavior.

```sh
dotfmt -l ini .gitconfig .editorconfig unit.service
dotfmt -l ini --dialect ssh ~/.ssh/config
dotfmt -l ini --check .
dotfile dev check -p dotfmt-ini
```

| Behavior | Value |
| --- | --- |
| Parser | Hand-written line model; sections, entries, comments, continuations and raw lines keep source slices |
| Dialects | `generic`, `git` (`gitconfig`), `editorconfig`, `systemd` (`unit`), `ssh` (`ssh-config`) |
| Normalized | Layout whitespace around the separator, trailing entry whitespace, entry indentation for Git/SSH, blank runs, SSH `Keyword = Value` to `Keyword Value`, full-line comment spacing (opt-in) |
| Preserved | Section and block order, duplicate keys, value bytes except trailing layout whitespace, inline `#`/`;` bytes, quoting, escapes, continuation lines, BOM, lone CR |
| Verification | Output is reparsed and compared with an input fingerprint; any section, key, value or continuation change is an internal error and the file stays unchanged |
| Repairs | None: malformed input is reported, never guessed at |
| Encoding | UTF-8; an existing BOM is preserved; CRLF is normalized, a lone CR is content |
| Configuration | `ini { ... }` in `dotfmt.dotfile`; global width, indent, final newline and quote style are accepted |

Engine defaults below apply before configuration.

| Setting | Default | Values |
| --- | --- | --- |
| `dialect` | `auto` | `auto`, `generic` (`ini`, `conf`), `git` (`gitconfig`, `git-config`), `editorconfig` (`editor-config`), `systemd` (`unit`), `ssh` (`sshconfig`, `ssh-config`) |
| `indent` | dialect | Git `2`, SSH `4`; Generic, EditorConfig and Systemd keep entries at column 0 |
| `indent_type` | `spaces` | `spaces`, `tabs`; one tab per level with `tabs` |
| `line_endings` | `unix` | `unix`, `windows` |
| `final_newline` | `true` | Boolean |
| `blank_lines` | `1` | 0–10 consecutive blank lines; leading and trailing runs are removed |
| `align` | `true` | Align `=` in Git/EditorConfig/Generic groups, and SSH keywords; Systemd stays compact |
| `align_max` | `24` | 0–1000; wider keys fall back to one space |
| `format_comments` | `false` | Add one space after `#` or `;` on full-line comments where safe |

## Dialects

| Dialect | Sections / blocks | Separator | Indent | Notes |
| --- | --- | --- | --- | --- |
| `generic` | `[section]` | `key = value` | column 0 | Indented lines are configparser-style continuations and stay byte-exact; values with `#`, `;`, `=`, quoting and `@Variant(...)` stay byte-exact |
| `git` | `[section]` and `[section "sub"]` | `key = value` | 2 spaces | Bare keys and continuation lines are preserved; duplicate keys stay |
| `editorconfig` | `[glob]` | `key = value` | 0 | Section headers are globs; inline `#`/`;` are value bytes |
| `systemd` | `[Unit]`, `[Service]`, ... | `Key=Value` | 0 | Inline `#`/`;` are value bytes; backslash continuations stay byte-exact |
| `ssh` | `Host` / `Match` | `Keyword value` | 4 spaces | Non-empty `Keyword=Value` normalizes to a space; empty `Keyword=` stays distinct |

Trailing entry whitespace is trimmed, except after a value that ends in a backslash: that whitespace decides whether the next line is a continuation and is preserved. Comments are full-line only. Inline markers remain part of the value in every dialect. A backslash at the end of a Git, Generic or Systemd line continues it; continuation lines are emitted byte-for-byte, including leading whitespace. In Generic INI, any indented line is treated as a continuation (configparser, tox and setup.cfg style), not as a new entry.
