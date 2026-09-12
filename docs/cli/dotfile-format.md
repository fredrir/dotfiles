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

## Shell formatting

| Files | Formatter | Check mode |
| --- | --- | --- |
| `.sh`, `.bash`, Bash startup files, `.profile` | Shuck | Formatting check and lint |
| `.zsh`, `.zshrc`, `.zshenv`, `.zprofile`, `.zlogin`, `.zlogout` | Shuck | Formatting check and lint |

Shuck reads project configuration or `~/.config/shuck/shuck.toml`; the shared default uses two spaces.
`dotfile-format` enables Shuck's experimental formatter for its formatting subprocesses.
Neovim uses `shuck server` for inline diagnostics, navigation, and formatting on save.
Bash Language Server supplies shell completion only; its diagnostics and formatting are disabled.
Syntax diagnostics are enabled explicitly; Shuck 0.2.2 still misses some malformed arithmetic expressions.
