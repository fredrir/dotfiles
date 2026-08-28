# size

## Commands

<!-- cli:commands:start -->
| Command | Description                                              |
| ------- | -------------------------------------------------------- |
| `size`  | Reports sizes and line counts for files and directories. |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                       | Description                                                           |
| -------------------------- | --------------------------------------------------------------------- |
| `-r`                       | Lists the immediate contents of the directory.                        |
| `-R`                       | Lists the contents of the directory recursively.                      |
| `-l`, `--lines`            | Counts lines instead of bytes.                                        |
| `-A`, `--apparent`         | Measures logical lengths rather than the space actually taken up.     |
| `-L`, `--limit <DEPTH>`    | Limits how deep the recursive listing goes.                           |
| `-a`, `--all`              | Includes hidden entries in listings, which totals count either way.   |
| `-i`, `--ignore <PATTERN>` | Leaves matching entries out of both the listing and the totals.       |
| `-x`, `--one-file-system`  | Stays on the filesystem the target sits on, leaving other mounts out. |
| `-h`, `--help`             | Shows help for the selected command and exits.                        |
| `--completions <SHELL>`    | Prints a shell completion script for the named shell and exits.       |
| `-V`, `--version`          | Prints the version and exits.                                         |
<!-- cli:flags:end -->
