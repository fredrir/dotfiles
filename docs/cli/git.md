# Git CLI

## Commands

<!-- cli:commands:start -->
| Command | Description                                                                                                 |
| ------- | ----------------------------------------------------------------------------------------------------------- |
| `gdd`   | Discards tracked and untracked working-tree changes while preserving ignored files and nested repositories. |
| `gget`  | Downloads a file or directory from a GitHub repository into the current directory.                          |
| `gppf`  | Stages everything, commits with the supplied message, and pushes the commit.                                |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                      | Description                                                                                                 |
| ------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `-n`, `--dry-run`         | Shows what `gdd` would discard without changing the working tree.                                           |
| `-a`, `--all`             | Makes `gdd` list every entry instead of truncating its sections, and includes dotfiles in a `gget` listing. |
| `-y`, `--yes`             | Skips confirmation before discarding changes or replacing a download.                                       |
| `-f`, `--fredrir`         | Reads the `gget` target as a repository owned by `fredrir`.                                                 |
| `-b`, `--branch <BRANCH>` | Selects the branch or tag from which `gget` downloads.                                                      |
| `-l`, `--list`            | Prints the contents of the `gget` target instead of downloading it.                                         |
| `-h`, `--help`            | Shows help for the selected command and exits.                                                              |
| `--completions <SHELL>`   | Prints a shell completion script for the named shell and exits.                                             |
| `-V`, `--version`         | Prints the version and exits.                                                                               |
<!-- cli:flags:end -->
