# dotfile

## Commands

<!-- cli:commands:start -->
| Command                  | Description                                                                                         |
| ------------------------ | --------------------------------------------------------------------------------------------------- |
| `dotfile`                | Manages this repository's symlinks, packages, themes, secrets, and system files.                    |
| `dotfile sync`           | Installs the workstation commands, refreshes generated metadata, and reconciles `$HOME`.            |
| `dotfile dev`            | Tests and lints the repository.                                                                     |
| `dotfile dev test`       | Runs the selected test suites.                                                                      |
| `dotfile dev lint`       | Runs the selected linters.                                                                          |
| `dotfile dev check`      | Runs the selected linters and test suites.                                                          |
| `dotfile docs`           | Generates and checks CLI reference, keybindings, package documentation, and README previews.        |
| `dotfile secret`         | Keeps private material out of the repository.                                                       |
| `dotfile secret scan`    | Scans for leaked tokens, private values, and encryption invariants.                                 |
| `dotfile secret init`    | Creates this machine's age identity and prints its public key.                                      |
| `dotfile secret enroll`  | Adds a recipient or enrolls this machine when no key is provided.                                   |
| `dotfile secret revoke`  | Removes a recipient and gives every encrypted file a new data key.                                  |
| `dotfile secret roll`    | Replaces a recipient's key while keeping its label.                                                 |
| `dotfile secret rekey`   | Gives every encrypted file a new data key without changing recipients.                              |
| `dotfile secret keys`    | Lists the enrolled recipients.                                                                      |
| `dotfile secret sync`    | Regenerates `.sops.yaml` from `config/keys.dotfile`.                                                |
| `dotfile secret doctor`  | Checks identities, recipients, hooks, and encrypted files.                                          |
| `dotfile secret add`     | Encrypts a live file into the repository and keeps it in place.                                     |
| `dotfile secret edit`    | Opens a tracked secret in `$EDITOR` and reapplies it.                                               |
| `dotfile secret apply`   | Decrypts every tracked secret to its destination.                                                   |
| `dotfile secret status`  | Shows what each tracked secret looks like on this machine.                                          |
| `dotfile secret vars`    | Lists the names that secret templates can reference.                                                |
| `dotfile secret clean`   | Removes materialized secrets from their destinations.                                               |
| `dotfile system`         | Tracks root-owned files under `/etc` and installs them as root.                                     |
| `dotfile system status`  | Compares tracked system files with their installed versions.                                        |
| `dotfile system diff`    | Shows what would change on disk without modifying anything.                                         |
| `dotfile system install` | Installs tracked system files at their destinations as root.                                        |
| `dotfile system add`     | Copies a root-owned file into the repository.                                                       |
| `dotfile theme`          | Stamps selected theme profiles into generated configuration files.                                  |
| `dotfile theme sync`     | Regenerates every config from the selected theme profiles.                                          |
| `dotfile theme dry`      | Reports what theme generation would change without writing.                                         |
| `dotfile theme check`    | Validates every profile and resolved application color pair.                                        |
| `dotfile theme contrast` | Prints one or every profile's resolved contrast matrix.                                             |
| `dotfile theme status`   | Shows each group's resolved profile and whether generated files have drifted.                       |
| `dotfile theme preview`  | Previews a profile's palette, roles, fonts, and terminal colors.                                    |
| `dotfile theme gallery`  | Shows the shared picker, progress, and comparison components using a theme profile.                 |
| `dotfile theme switch`   | Assigns a profile globally, to a group, or to a package.                                            |
| `dotfile theme outputs`  | Prints the files owned by the theme generator.                                                      |
| `dotfile add`            | Moves a live config into the repository and symlinks it back.                                       |
| `dotfile remove`         | Moves a tracked path out of the repository and keeps it live.                                       |
| `dotfile doctor`         | Checks the profile's links, tools, fonts and packages; prints install commands for what is missing. |
| `dotfile format`         | Format configured files                                                                             |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                          | Description                                                                                         |
| ----------------------------- | --------------------------------------------------------------------------------------------------- |
| `-n`, `--dry-run`             | Plans without changing files or contacting the peer.                                                |
| `--override <GROUP=NAME>`     | Selects a machine override with `<group>=<name\|none>`.                                             |
| `--force`                     | Resolves local edits from the repository and discards remote edits with `--push`.                   |
| `--resolve <RESOLVE>`         | Chooses `skip`, `repo`, or `live` for locally edited merged configs.                                |
| `-p`, `--push`                | Pushes commits, then pulls and syncs the peer.                                                      |
| `--to <HOST>`                 | Selects the peer and implies `--push`.                                                              |
| `-v`, `--verbose`             | Shows detailed sync actions or live development commands, output, and timings.                      |
| `--commands-only`             | Installs the workstation commands and stops.                                                        |
| `--native-only`               | Installs the compiled commands only and stops.                                                      |
| `--rebuild`                   | Rebuilds every command even when its sources are unchanged.                                         |
| `--docs`                      | Regenerate documentation in docs/ (normally handled by pre-commit)                                  |
| `-p`, `--pkg <TARGET>`        | Selects development targets or names an added config, secret, or system package.                    |
| `-l`, `--lang <LANGUAGE>`     | Selects languages; repeat or comma-separate.                                                        |
| `--changed <REF>`             | Selects affected packages and dependents from working changes or --changed=REF.                     |
| `--python-workers <N>`        | Caps Python workers within the total worker budget; defaults to four.                               |
| `-j`, `--jobs <N>`            | Limits the total worker budget; defaults to CPU count.                                              |
| `--concurrency <N>`           | Limits simultaneous development tasks; defaults to two.                                             |
| `--only <ONLY>`               | Selects documentation outputs; repeat or comma-separate.                                            |
| `--check`                     | Reports stale documentation without writing; exits 1 on drift or missing metadata.                  |
| `--diff`                      | Show unified changes without writing                                                                |
| `--json`                      | Print the change report as JSON                                                                     |
| `--staged`                    | Scans the content staged for commit.                                                                |
| `--commits <COMMITS>`         | Scans blobs added within revision-list ranges; repeat to combine ranges into one review.            |
| `--review`                    | Inspects findings and remembers accepted file contents locally; changed contents need review again. |
| `--no-canaries`               | Skips the private-value tier of secret scanning.                                                    |
| `--all`                       | Shows every finding or file location instead of summarized output.                                  |
| `--using <USING>`             | Uses the selected identity file for recipient and re-encryption operations.                         |
| `--rewrap`                    | Updates the recipients on every encrypted file during secret sync.                                  |
| `--shared`                    | Places an added file in the shared package group.                                                   |
| `--linux`                     | Places an added file in the `linux/common` package group.                                           |
| `--arch`                      | Places an added file in the `linux/arch` package group.                                             |
| `--ubuntu`                    | Places an added file in the `linux/ubuntu` package group.                                           |
| `--kde`                       | Places an added file in the `linux/kde` package group.                                              |
| `--hyprland`                  | Places an added file in the `linux/hyprland` package group.                                         |
| `--macos`                     | Places an added file in the `macos` package group.                                                  |
| `--marker`                    | Forces the `.secret` package marker on or off.                                                      |
| `--no-marker`                 | Prevents creating the .secret package marker.                                                       |
| `--unused`                    | Lists only variable names that no secret template references.                                       |
| `--yes`                       | Installs system files without asking for confirmation.                                              |
| `--group <GROUP>`             | Selects the package group for an added system file.                                                 |
| `--server`                    | Places an added config in the `linux/server` package group.                                         |
| `--description <DESCRIPTION>` | Adds a package description to `PACKAGES.md`.                                                        |
| `-h`, `--help`                | Shows help for the selected command and exits.                                                      |
| `--completions <SHELL>`       | Prints a shell completion script for the named shell and exits.                                     |
| `-V`, `--version`             | Prints the version and exits.                                                                       |
<!-- cli:flags:end -->
