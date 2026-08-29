# dotfile

## Commands

<!-- cli:commands:start -->
| Command                  | Description                                                                      |
| ------------------------ | -------------------------------------------------------------------------------- |
| `dotfile`                | Manages this repository's symlinks, packages, themes, secrets, and system files. |
| `dotfile add`            | Moves a live config into the repository and symlinks it back.                    |
| `dotfile remove`         | Moves a tracked path out of the repository and keeps it live.                    |
| `dotfile docs`           | Regenerates the command tables in `docs/cli` from the tools themselves.          |
| `dotfile packages`       | Regenerates `config/packages.dotfile` and `PACKAGES.md`.                         |
| `dotfile sync`           | Reconciles `$HOME` with a profile by linking, merging, and applying secrets.     |
| `dotfile status`         | Shows link state for every file in a profile.                                    |
| `dotfile check`          | Checks a profile's links, required tools, and packages.                          |
| `dotfile secret`         | Keeps private material out of the repository.                                    |
| `dotfile secret scan`    | Scans for leaked tokens, private values, and encryption invariants.              |
| `dotfile secret init`    | Creates this machine's age identity and prints its public key.                   |
| `dotfile secret enroll`  | Adds a recipient or enrolls this machine when no key is provided.                |
| `dotfile secret revoke`  | Removes a recipient and gives every encrypted file a new data key.               |
| `dotfile secret roll`    | Replaces a recipient's key while keeping its label.                              |
| `dotfile secret rekey`   | Gives every encrypted file a new data key without changing recipients.           |
| `dotfile secret keys`    | Lists the enrolled recipients.                                                   |
| `dotfile secret sync`    | Regenerates `.sops.yaml` from `config/keys.dotfile`.                             |
| `dotfile secret doctor`  | Checks identities, recipients, hooks, and encrypted files.                       |
| `dotfile secret add`     | Encrypts a live file into the repository and keeps it in place.                  |
| `dotfile secret edit`    | Opens a tracked secret in `$EDITOR` and reapplies it.                            |
| `dotfile secret apply`   | Decrypts every tracked secret to its destination.                                |
| `dotfile secret status`  | Shows what each tracked secret looks like on this machine.                       |
| `dotfile secret vars`    | Lists the names that secret templates can reference.                             |
| `dotfile secret clean`   | Removes materialized secrets from their destinations.                            |
| `dotfile system`         | Tracks root-owned files under `/etc` and installs them as root.                  |
| `dotfile system status`  | Compares tracked system files with their installed versions.                     |
| `dotfile system diff`    | Shows what would change on disk without modifying anything.                      |
| `dotfile system install` | Installs tracked system files at their destinations as root.                     |
| `dotfile system add`     | Copies a root-owned file into the repository.                                    |
| `dotfile theme`          | Stamps selected theme profiles into generated configuration files.               |
| `dotfile theme sync`     |                                                                                  |
| `dotfile theme dry`      |                                                                                  |
| `dotfile theme status`   | Shows each group's resolved profile and whether generated files have drifted.    |
| `dotfile theme show`     | Previews a profile's palette, roles, fonts, and terminal colors.                 |
| `dotfile theme switch`   | Assigns a profile globally, to a group, or to a package.                         |
| `dotfile theme outputs`  | Prints the files owned by the theme generator.                                   |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                             | Description                                                                                |
| -------------------------------- | ------------------------------------------------------------------------------------------ |
| `--shared`                       | Places an added file in the shared package group.                                          |
| `--linux`                        | Places an added file in the `linux/common` package group.                                  |
| `--arch`                         | Places an added file in the `linux/arch` package group.                                    |
| `--ubuntu`                       | Places an added file in the `linux/ubuntu` package group.                                  |
| `--kde`                          | Places an added file in the `linux/kde` package group.                                     |
| `--hyprland`                     | Places an added file in the `linux/hyprland` package group.                                |
| `--server`                       | Places an added config in the `linux/server` package group.                                |
| `--macos`                        | Places an added file in the `macos` package group.                                         |
| `--pkg <TEXT>`                   | Selects the package name when adding a config, secret, or system file.                     |
| `--description`, `--desc <TEXT>` | Adds a package description to `PACKAGES.md`.                                               |
| `--check`                        | Reports documentation drift instead of writing the tables.                                 |
| `-n`, `--dry-run`                | Reports actions without changing files.                                                    |
| `--override <TEXT>`              | Selects a machine override with `<group>=<name\|none>`.                                    |
| `--force`                        | Forces repository resolution during sync or overwrites locally edited secret destinations. |
| `--resolve <TEXT>`               | Selects `skip`, `repo`, or `live` resolution for locally changed configs.                  |
| `-p`, `--push`                   | Pushes changes, then pulls and syncs the other machine.                                    |
| `--to <TEXT>`                    | Selects the machine targeted by `--push`.                                                  |
| `--all`                          | Shows every finding or file location instead of summarized output.                         |
| `--staged`                       | Scans the content staged for commit.                                                       |
| `--commits <TEXT>`               | Scans blobs added within a revision-list range.                                            |
| `--no-canaries`                  | Skips the private-value tier of secret scanning.                                           |
| `--using <TEXT>`                 | Uses the selected identity file for recipient and re-encryption operations.                |
| `--rewrap`                       | Updates the recipients on every encrypted file during secret sync.                         |
| `--marker`, `--no-marker`        | Forces the `.secret` package marker on or off.                                             |
| `--unused`                       | Lists only variable names that no secret template references.                              |
| `--yes`                          | Installs system files without asking for confirmation.                                     |
| `--group <TEXT>`                 | Selects the package group for an added system file.                                        |
| `--help`                         | Shows help for the selected command and exits.                                             |
| `--completions <SHELL>`          | Prints a shell completion script for the named shell and exits.                            |
<!-- cli:flags:end -->
