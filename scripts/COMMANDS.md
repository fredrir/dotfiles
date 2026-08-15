# Command reference

`setup.sh` installs these commands from `scripts/` into `~/.local/bin`.
`<VALUE>` is required, `[VALUE]` is optional, and `...` means repeatable.
All Python commands accept `--help`.

## Utilities

| Command | Purpose and options |
| --- | --- |
| `count <DIRECTORY>` | Count direct entries. `-r`, `--recursive`: include all descendants; `-d`, `--no-hidden`: exclude hidden entries and subtrees. |
| `gdd [PATH...]` | Discard every change: tracked files back to `HEAD`, untracked files deleted. Prints the plan and asks first. `-n`, `--dry-run`: show the plan and stop; `-a`, `--all`: list every entry instead of the first 12 of a section; `-y`, `--yes`: do not ask. Ignored files and nested repositories are kept. |
| `gpp <MESSAGE...>` | Run `git add .`, commit with the joined message, then push. |
| `path [TARGET=.]` | Print a repo-relative, home-relative, or absolute path. `-f`, `--full`: always print the absolute path. |
| `size [TARGET]` | Measure bytes; with no target, list the current directory. `-r`: list direct children; `-R`: list recursively; `-l`, `--lines`: count lines; `-L`, `--limit <DEPTH>`: limit `-R`; `-a`, `--all`: show hidden rows. |
| `tardirs <ARCHIVE> [MAX_DEPTH]` | Print a tar archive's directory tree and direct-entry counts. |
| `clean-copy [--stdin]` | Normalize selected text and replace the clipboard; `--stdin` reads the text from stdin. |
| `cpa [-s\|--sensitive]` | Copy the local text clipboard to Archie; sensitive mode avoids clipboard history. |
| `cpas` | Sensitive shorthand for `cpa --sensitive`. |
| `acp` | Copy Archie's text clipboard to the local clipboard. |
| `power-menu` | Open the Hyprland power menu. |
| `confirm-exit` | Confirm, then exit Hyprland. |
| `update-readme-fastfetch` | Refresh the Fastfetch block in `README.md`. |

`count`, `gdd`, `gpp`, `path`, and `size` also accept `-h`/`--help`,
`-V`/`--version`, and `--completions <bash|elvish|fish|powershell|zsh>`.

## `sysinfo`

`sysinfo [-p|--pretty] [-f|--full] [-hh|--health]` prints the machine summary.
`--pretty` uses the branded hardware view, `--full` includes the extended
inventory, and `--health` explains active findings. The flags can be combined.

Bare `sysinfo bench` opens an interactive menu.

| Command | Purpose and options |
| --- | --- |
| `sysinfo bench run` | Measure and store a run. `--tier <quick\|standard\|heavy>` (default `quick`); `--only <cpu,mem,cache,disk,gpu,thermal,workload>`; `--note <TEXT>`; repeatable `--tag <TAG>`; `--host <HOST>`; `--workdir <DIR>`; `--force`; `--no-save`; `--baseline`; `--json`. |
| `sysinfo bench show [SELECTOR]` | Show a stored run; omit the selector to pick one. `--json`: emit JSON. |
| `sysinfo bench list` | List clean runs. `--host <HOST>`; `--limit <N>` (default `20`, `0` for all); `--all`: include noisy and aborted runs. |
| `sysinfo bench health` | Report benchmark-history findings. `--host <HOST>`. |
| `sysinfo bench compare [LEFT] [RIGHT]` | Compare two runs; omit either selector to pick interactively. `--json`: emit JSON. |
| `sysinfo bench trend [SELECTOR] [METRIC]` | Show a metric over time; omit arguments to pick interactively. |
| `sysinfo bench baseline [show\|set\|clear] [SELECTOR]` | List baselines, set one from a run, or clear one for `HOST@EPOCH`. |
| `sysinfo bench document` | Regenerate `benchmarks/BENCHMARKS.md`. |
| `sysinfo bench prune` | Thin old runs. `--host <HOST>`; `--keep <N>` (default `12`); `--dry-run`; `--yes`. |

Selectors use forms such as `HOST`, `HOST@EPOCH`, and a run ID.

## `transcript`

Bare `transcript` opens an interactive menu.

| Command | Purpose and options |
| --- | --- |
| `transcript capture` | Archive clipboard text. `--provider <NAME>`; `--raw`: skip redaction; `--quiet`; `--fallback <FILE>`: use and remove a snapshot when the clipboard is empty. |
| `transcript import [SESSION.jsonl]` | Import a Claude Code or Codex session. `--latest`; `--limit <N>` (picker default `15`); `--raw`; `--tools`: include tool calls. |
| `transcript list` | List recent sessions. `--limit <N>` (default `15`). |
| `transcript add [PATH=.]` | Track a project. `--name <NAME>`; `--group <GROUP>`. |
| `transcript rm <NAME\|PATH>` | Stop tracking a project without removing existing notes. |
| `transcript migrate` | Move groups to configured destinations after confirmation. `-v`, `--verbose`: list every file. |
| `transcript sync` | Sync tracked sessions. `--dry-run`; `--raw`; `--quiet`; `--tools`. |

## `dotfile`

Bare `dotfile` prints help.

| Command | Purpose and options |
| --- | --- |
| `dotfile link [PROFILE]` | Link a profile into `$HOME`. `-n`, `--dry-run`; repeatable `--override <GROUP>=<NAME\|none>`. |
| `dotfile add <PATH>` | Adopt and link a live config. Placement: `--shared`, `--linux`, `--arch`, `--ubuntu`, `--kde`, `--hyprland`, `--server`, or `--macos`; `--pkg <NAME>`; `--description <TEXT>`/`--desc <TEXT>`. |
| `dotfile remove <PATH>` | Stop tracking a path while keeping it live. |
| `dotfile packages` | Regenerate `config/packages.dotfile` and `PACKAGES.md`. |
| `dotfile format [PATH...]` | Format tracked or selected `.conf` files. `--stdin <NAME>`: format stdin as that filename. |
| `dotfile sync` | Run setup non-interactively and skip unchanged steps. |
| `dotfile status [PROFILE]` | Show link state for a profile. |
| `dotfile check [PROFILE]` | Check links, tools, and packages. `--all`: show every finding. |

### `dotfile secret`

Bare `dotfile secret` prints help.

| Command | Purpose and options |
| --- | --- |
| `dotfile secret scan [PATH...]` | Scan for leaked or invalid secret material. `--staged`; `--commits <RANGE>`; `--no-canaries`; `--all`. |
| `dotfile secret init` | Create this machine's age identity and print its public key. |
| `dotfile secret enroll <LABEL> [KEY]` | Add a recipient; omit the key to use this machine. `--using <IDENTITY>`. |
| `dotfile secret revoke <LABEL>` | Remove a recipient and rotate file data keys. |
| `dotfile secret roll <LABEL> [KEY]` | Replace a recipient key. `--using <IDENTITY>`. |
| `dotfile secret rekey` | Rotate file data keys without changing recipients. `--using <IDENTITY>`. |
| `dotfile secret keys` | List enrolled recipients. |
| `dotfile secret sync` | Regenerate `.sops.yaml`. `--rewrap`: update every encrypted file; `--using <IDENTITY>`. |
| `dotfile secret doctor` | Check identities, recipients, hooks, and encrypted files. `--all`: include file locations. |
| `dotfile secret add <PATH>` | Encrypt and track a live file. `--pkg <NAME>`; placement: `--shared`, `--linux`, `--arch`, `--ubuntu`, `--kde`, `--hyprland`, or `--macos`; `--marker`/`--no-marker`. |
| `dotfile secret edit <PATH>` | Edit a tracked secret and re-apply it. |
| `dotfile secret apply` | Materialize tracked secrets. `-n`, `--dry-run`; `--force`: overwrite locally edited destinations. |
| `dotfile secret status` | Show rendered secret state. |
| `dotfile secret vars` | List template variable names. `--unused`: list unreferenced names only. |
| `dotfile secret clean` | Remove materialized secrets. `-n`, `--dry-run`. |

### `dotfile system`

Bare `dotfile system` prints help.

| Command | Purpose and options |
| --- | --- |
| `dotfile system status` | Compare tracked root-owned files with installed files. |
| `dotfile system diff [PATH]` | Show pending changes, optionally filtered by path. |
| `dotfile system install` | Install tracked files as root. `-n`, `--dry-run`; `--yes`: skip confirmation. |
| `dotfile system add <PATH> --pkg <NAME>` | Copy a root-owned file into the repo. `--group <GROUP>` (default `linux/arch`). |

### `dotfile theme`

Bare `dotfile theme` opens an interactive menu.

| Command | Purpose and options |
| --- | --- |
| `dotfile theme apply` | Regenerate all owned configs. |
| `dotfile theme check` | Report drift without writing; exit non-zero on changes. |
| `dotfile theme status` | Show resolved profiles and drift. |
| `dotfile theme show [PROFILE]` | Preview a profile; omit it to pick one. |
| `dotfile theme switch [PROFILE] [SCOPE]` | Assign and apply a profile. Scope is `everything`, a group, or `group/package`; the default is `shared`. |
| `dotfile theme outputs` | Print owned files. `--stageable`: only files safe to auto-stage. |

### Hidden helpers

| Command | Purpose and options |
| --- | --- |
| `dotfile help` | Print `dotfile` help. |
| `dotfile profiles` | Print environment profile names. `--relevant`: only profiles matching this host. |
| `dotfile theme profiles` | Print theme profile names. |

## Native support commands

These are installed for `sysinfo`; they are normally called by it.

| Command | Purpose and options |
| --- | --- |
| `bench-workloads --list` | List the available native workloads. |
| `bench-workloads --version` | Print the workload runner version. |
| `bench-workloads cpu` | Run the CPU workload. `--threads <N>` (default `1`, `0` means all logical cores); `--iterations <N>` (default `800000000` per thread). |
| `bench-workloads memory` | Run the memory workload. `--op <read\|write>` (default `read`); `--mib <N>` (default `256`); `--passes <N>` (default `128`). |
| `sysinfo-collect` | Emit fastfetch-shaped system JSON. `--version`: print its version instead. |
