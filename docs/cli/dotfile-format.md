# dotfile format

## Commands

<!-- cli:commands:start -->
| Command          | Description                                                       |
| ---------------- | ----------------------------------------------------------------- |
| `dotfile-format` | Formats a tree by handing each language to the tool that owns it. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                      |
| ----------------------- | -------------------------------------------------------------------------------- |
| `--check`               | Verifies formatting and runs each language's linter instead of writing anything. |
| `-a`, `--add`           | Offers this repository's tool configuration to the target, asking per file.      |
| `-s`, `--sync`          | Replaces the tool configuration the target already has, without asking.          |
| `-v`, `--verbose`       | Names every file as it is formatted.                                             |
| `-q`, `--quiet`         | Reports nothing but failures.                                                    |
| `-h`, `--help`          | Shows help for the selected command and exits.                                   |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                  |
| `-V`, `--version`       | Prints the version and exits.                                                    |
<!-- cli:flags:end -->
