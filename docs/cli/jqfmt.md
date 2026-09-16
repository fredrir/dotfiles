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
| `--check`               | Reports what is not laid out instead of writing it; with --editor, a repair is a finding too.                 |
| `-v`, `--verbose`       | Names every file as it is formatted.                                                                          |
| `-q`, `--quiet`         | Reports nothing but failures.                                                                                 |
| `-h`, `--help`          | Shows help for the selected command and exits.                                                                |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                                               |
| `-V`, `--version`       | Prints the version and exits.                                                                                 |
<!-- cli:flags:end -->
