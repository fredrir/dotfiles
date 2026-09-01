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

## Browser

Without `--to`, `--from`, or `--yes`, the remote filesystem opens in an inline browser.

### Command Behavior

| Command | Behavior                                                      |
| ------- | ------------------------------------------------------------- |
| `hpush` | Selects the open directory as its destination                 |
| `hpull` | Selects the highlighted entry, or a genuinely empty directory |

### Destination Rules

| Scenario                    | Behavior                                       |
| --------------------------- | ---------------------------------------------- |
| Missing `hpush` destination | Can be selected and is created by the transfer |
| Missing `hpull` source      | Cannot be selected                             |
| Unreadable `hpull` source   | Cannot be selected                             |

### Navigation Keys

| Key                  | Action                                                                    |
| -------------------- | ------------------------------------------------------------------------- |
| Arrow keys, `j`, `k` | Move                                                                      |
| Right, `l`           | Open a directory                                                          |
| Left, `h`            | Return to parent directory                                                |
| Page Up, Page Down   | Move through listings                                                     |
| Home, End            | Jump to start/end of listings                                             |
| `g`, `G`             | Move through longer listings                                              |
| `/`                  | Start case-insensitive filtering or accept path beginning with `/` or `~` |
| `r`                  | Refresh the open directory                                                |
| `?`                  | Show complete key guide                                                   |
| Escape               | Cancel                                                                    |
| Ctrl-C               | Interrupt                                                                 |
