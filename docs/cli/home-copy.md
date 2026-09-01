# hpush and hpull

## Commands

<!-- cli:commands:start -->
| Command | Description                                                         |
| ------- | ------------------------------------------------------------------- |
| `hpush` | Copies a path from this machine to the same place on the other one. |
| `hpull` | Copies a path from the other machine to the same place on this one. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                             |
| ----------------------- | --------------------------------------------------------------------------------------- |
| `-n`, `--dry-run`       | Reports what would be transferred and changes nothing.                                  |
| `-c`, `--checksum`      | Compares file contents instead of size and modification time.                           |
| `-a`, `--all`           | Includes files that .gitignore, .git, and the shared exclude list would otherwise skip. |
| `-y`, `--yes`           | Accepts the mirrored location without opening the browser or asking.                    |
| `-v`, `--verbose`       | Lists every transferred path instead of counting them.                                  |
| `--to <PATH>`           | Sets the directory on the other machine to copy into.                                   |
| `--from <PATH>`         | Sets the path on the other machine to copy out of.                                      |
| `-h`, `--help`          | Shows help for the selected command and exits.                                          |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                         |
| `-V`, `--version`       | Prints the version and exits.                                                           |
<!-- cli:flags:end -->
