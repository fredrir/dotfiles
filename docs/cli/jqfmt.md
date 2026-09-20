# jqfmt

## Commands

<!-- cli:commands:start -->
| Command | Description                                 |
| ------- | ------------------------------------------- |
| `jqfmt` | Formats the JSON in a tree the way jq does. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                                                   |
| ----------------------- | ------------------------------------------------------------------------------------------------------------- |
| `-e`, `--editor`        | Reads what jq refuses: comments, trailing commas, single quotes, unquoted keys, Python names, missing commas. |
| `--dialect <DIALECT>`   | Input dialect (auto detects file extensions; stdin defaults to JSON)                                          |
| `--check`               | Reports what is not laid out instead of writing it; with --editor, a repair is a finding too.                 |
| `-v`, `--verbose`       | Names every file as it is formatted.                                                                          |
| `-q`, `--quiet`         | Reports nothing but failures.                                                                                 |
| `-h`, `--help`          | Shows help for the selected command and exits.                                                                |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                                               |
| `-V`, `--version`       | Prints the version and exits.                                                                                 |
<!-- cli:flags:end -->

## Dialects

| Input | Default dialect |
| --- | --- |
| `.json` | JSON; existing jq-compatible formatting |
| `.jsonc` | JSONC |
| `.hujson`, `.jwcc` | HuJSON / JWCC |
| stdin, other extensions | JSON; override with `--dialect` |

| Behavior | JSONC / HuJSON |
| --- | --- |
| Comments | Preserve `//` and `/* ... */` text and order |
| Trailing commas | Accept and preserve |
| Keys, strings, numbers | Preserve spelling and duplicate keys |
| Layout | `jqfmt.dotfile` indentation and final newline settings |
| Compact layout | Keep newlines required by `//` comments |
| `--editor` | Convert to JSON, removing comments and trailing commas |
| JSON5 | Unsupported |

```sh
jqfmt .
jqfmt --check policy.hujson
jqfmt --dialect jsonc settings.json
jqfmt --dialect hujson < policy.hujson
```
