# Git CLI

## Commands

| Command       | Description                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------------- |
| `git-discard` | Discards tracked and untracked working-tree changes while preserving ignored files and nested repositories. |
| `gget`        | Downloads a file or directory from a GitHub repository into the current directory.                          |
| `gpp`         | Stages everything, commits with the supplied message, and pushes the commit.                                |

## Flags

| Flag                      | Description                                                           |
| ------------------------- | --------------------------------------------------------------------- |
| `-n`, `--dry-run`         | Shows what `gdd` would discard without changing the working tree.     |
| `-a`, `--all`             | Makes `gdd` list every entry instead of truncating its sections.      |
| `-y`, `--yes`             | Skips confirmation before discarding changes or replacing a download. |
| `-f`, `--fredrir`         | Reads the `gget` target as a repository owned by `fredrir`.           |
| `-b`, `--branch <BRANCH>` | Selects the branch or tag from which `gget` downloads.                |
| `--completions <SHELL>`   | Prints shell completions for the selected shell and exits.            |
| `-h`, `--help`            | Prints command help and exits.                                        |
| `-V`, `--version`         | Prints the command version and exits.                                 |
