# Command reference

`setup.sh` installs these commands from `scripts/` into `~/.local/bin`.
`<VALUE>` is required, `[VALUE]` is optional, and `...` means repeatable.
All Python commands accept `--help`.

## Utilities

| Command | Purpose and options |
| --- | --- |
| `count <DIRECTORY>` | Count direct entries. `-r`, `--recursive`: include all descendants; `-d`, `--no-hidden`: exclude hidden entries and subtrees. |
| `flatten <DIRECTORY...>` | Undo redundant nesting: while the directory holds exactly one entry and that entry is a directory, empty it into the target and remove it. Silent, and nothing can be overwritten. `-d`, `--deep`: bring every entry underneath up to the top and remove every directory under it, after printing the plan and asking; `-n`, `--dry-run`: show the plan and stop; `-y`, `--yes`: do not ask, which answers each name two entries want with the later one; `-v`, `--verbose`: name every move; `-a`, `--all`: list every row instead of the first 12 of a section. Symlinks are moved, never followed. `--deep` refuses `/` and the home directory. |
| `gdd [PATH...]` | Discard every change: tracked files back to `HEAD`, untracked files deleted. Prints the plan and asks first. `-n`, `--dry-run`: show the plan and stop; `-a`, `--all`: list every entry instead of the first 12 of a section; `-y`, `--yes`: do not ask. Ignored files and nested repositories are kept. The shell alias is backed by the `git-discard` executable. |
| `gpp <MESSAGE...>` | Stage everything from the repository root (`git add :/`), commit with the joined message, then push. |
| `path [TARGET=.]` | Print a repo-relative, home-relative, or absolute path. `-f`, `--full`: always print the absolute path. |
| `size [TARGET]` | Measure bytes; with no target, list the current directory. `-r`: list direct children; `-R`: list recursively; `-l`, `--lines`: count lines; `-L`, `--limit <DEPTH>`: limit `-R`; `-a`, `--all`: show hidden rows. |
| `hwire` | Measure the link to the other machine: round-trip latency, then a transfer each way. `-r`, `--route <cable\|tailscale>`: measure that route rather than the cable when it is up; `-b`, `--both`: measure every route that is up, one after the other; `-t`, `--time <SECONDS>`: transfer time per direction (default `1`); `-P`, `--streams <N>`: connections per direction (default `1`); `-n`, `--samples <N>`: round trips to time, at most (default `200`, and sampling also stops after half a second); `-l`, `--latency`: skip the transfers; `-u`, `--up` / `-d`, `--down`: one direction only; `--json`: machine-readable; `--at <ADDRESS:PORT>`: measure a `hwire serve` that is already listening instead of starting one over ssh, with `--token <HEX>` when it was given one. |
| `hwire serve` | The half `hwire` starts on the peer over ssh; usable by hand for any two machines that have the binary. `--bind <ADDRESS>` (default `0.0.0.0`); `--port <PORT>` (default `0`, which picks a free one and prints it); `--token <HEX>`: answer only the client presenting it, instead of anyone; `--idle <SECONDS>`: exit after this long with nothing connecting (default `15`, `0` waits forever). |
| `tardirs <ARCHIVE> [MAX_DEPTH]` | Print a tar archive's directory tree and direct-entry counts. |
| `clean-copy [--stdin]` | Normalize selected text and replace the clipboard; `--stdin` reads the text from stdin. |
| `cpa [-s\|--sensitive]` | Copy the local text clipboard to Archie; sensitive mode avoids clipboard history. |
| `cpas` | Sensitive shorthand for `cpa --sensitive`. |
| `acp` | Copy Archie's text clipboard to the local clipboard. |
| `power-menu` | Open the Hyprland power menu. |
| `confirm-exit` | Confirm, then exit Hyprland. |
| `update-readme-fastfetch` | Refresh the Fastfetch block in `README.md`. |

`count`, `flatten`, `gdd`, `gpp`, `hwire`, `path`, and `size` also accept `-h`/`--help`,
`-V`/`--version`, and `--completions <bash|elvish|fish|powershell|zsh>`.

## `dmux`

Bare `dmux` opens a picker (or creates `main` when nothing runs); with
`--host` it attaches the peer the way the old ssa/ssm did. `dmux <REF>`
attaches an existing Space (a trailing `-w <WINDOW>` picks a window) and
`dmux -` toggles back to the previous one. `-H`, `--host <HOST>` points any
command at another host: `macie`/`archie` always, and any enrolled alias,
label, or HostUid once `DMUX_WEZ_FIRST=1` is on. Global `--format
<human|json>` chooses the shape of any command that has a bounded result; a
verb that has none — `con`, `new`, `keys`, `ssh`, `disconnect`, and the bare
picker — refuses `--format json` rather than ignoring it.

A target is a reference, not a listing position. A bare digit is the Space's
permanent `SpaceNo` (`2`, or `b2`/`b:2` for the peer, or the full
`dmux://<host-uid>/spaces/<space-uid>` URI), never row 2 of the last listing.
`--row <N>` is the one-release compatibility escape that still means "the Nth
line of `dmux ls`": it resolves to a stable ref and reports that ref first.
`--name <VALUE>` is the exact-name escape for a Space whose name is shaped
like a ref or spelled like a verb.

Four listing scopes, deliberately distinct — `--all-hosts` sets host breadth,
`--tree` sets hierarchy depth, and the two are independent:

| Scope | Shows |
| --- | --- |
| `dmux ls` | Spaces on one host (`--host`, default this machine), one line each, no children. |
| `dmux ls --tree` | the same host set, with each Space's live Groups and Splits indented beneath it. |
| `dmux ls --all-hosts` | every enrolled host, queried concurrently under bounded timeouts; an unavailable host is reported, not dropped. Conflicts with `--host`. |
| `dmux host ls` | the enrolled hosts and their routes only, never Spaces. |

| Command | Purpose and options |
| --- | --- |
| `dmux ls` | List Spaces as `REF NAME BACKEND HOST GROUPS SPLITS SERVER CLIENT ROUTE STATE`; unmanaged native resources appear too, with `-` for a ref. `--backend <wez\|tmux>`: one backend only; `--tree`, `--all-hosts`: as above; `--format json`: the versioned envelope. Deprecated: `--tmux` / `--wez` (say `--backend`, and contradicting the two is an error) and `--json` (the bare legacy row array, whose `index` numbers the rows actually printed). Alias `list`. |
| `dmux con <REF>` | Attach ("continue") an existing Space; it never invents one, so a typo leaves nothing behind. `--name <VALUE>`: exact logical name instead of a ref; `--backend <wez\|tmux>`: require that backend and never fall back; `--group <GROUP_REF>` / `--split <SPLIT_REF>`: focus that epoch-qualified child after connecting; `--launch-gui`: start a managed GUI and attach only to an existing Wez Space; `-w`, `--window <WINDOW>`: select a window; `-A`, `--create`: create like `dmux new` when it does not exist. Aliases `attach`, `a`. |
| `dmux new <NAME>` | Create a Space if needed, then attach it. `--backend <auto\|wez\|tmux>`: creation policy (automatic when omitted); `--dir <PATH>`: working directory; `--no-connect`: create or select without presenting; `--allow-name-collision`: permit creation beside one selectable opposite-backend match; `--launch-gui`: as for `con`; a command may follow `--`. |
| `dmux disconnect` | Hand the invoking client back without removing owner panes; the Space keeps running. `--domain`: detach the whole current imported Wez domain. Rejects `--host` — it acts on the local client only. Alias `detach`. |
| `dmux rm <REF...>` | Remove Spaces after a `[y/N]` prompt on stderr. `--all`: every Space on exactly one host, `--backend`-filtered if asked — it sweeps Wez Spaces as well as tmux sessions, and only the pre-gate tmux path spares the session this client is in; `--name <VALUE>`; `--row <N>`, repeatable; `--backend <wez\|tmux>`; `-w`, `--window <WINDOW>`: remove one window instead; `-y`, `--yes`: do not ask, required without a terminal and required outright under `--format json`, which otherwise answers one exit-5 document and changes nothing. Aliases `kill`, `delete`. |
| `dmux rename <OLD> <NEW>` | Rename a Space — a tmux session while the Wez-first flag is off. With `--name <VALUE>` or `--row <N>` naming the target, the single positional is the new name instead. `--backend <wez\|tmux>`; `--allow-name-collision`: permit a name one opposite-backend Space already holds. |
| `dmux adopt <NATIVE_REF>` | Bring one unmanaged resource that `dmux ls` listed under management. `NATIVE_REF` is that row's opaque `native:<backend>:<token>`, re-resolved in a fresh scan and never handed to a backend as a command. `--name <NAME>`: logical name for the adopted Space (default: its native name). |
| `dmux group <SUB>` | Groups — wezterm tabs, tmux windows — of a managed Space. `ls [SPACE]` (defaults to this pane's Space; deprecated `--json`); `new <SPACE> [--dir <PATH>] [--no-connect] [-- <CMD>...]`; `rename <GROUP> <NEW_NAME>`, a title and nothing else; `rm <GROUP...>` with `-y`, never the last Group — that is `dmux rm`; `con <GROUP>` presents one. |
| `dmux split <SUB>` | Splits — panes — of a Group. `ls [GROUP]` (defaults to this pane's Group; deprecated `--json`); `new <GROUP> [--direction <left\|right\|up\|down>] [--percent <1-99>] [--dir <PATH>] [--no-connect] [-- <CMD>...]`; `rm <SPLIT...>` with `-y`, never the last Split — that is `dmux group rm`; `con <SPLIT>` presents one. |
| `dmux context stamp <SPACE>` | Acknowledge this pane's marker for an adopted Space: derive the epoch-qualified refs from the pane environment, record the stamp, and report how many panes are still pending. |
| `dmux repair normalize [TOKEN...]` | Preview, then merge multi-window Wez resources to one window each: deterministic pane-preserving plans, confirmed before any mutation, and a failure stays quarantined per target. Tokens restrict the scope; default is every detected multi-window resource. `-y`, `--yes`; deprecated `--json`. |
| `dmux repair reconcile [SPACE...]` | Preview, then resolve the journal rows a crashed holder stranded, each through the frozen decision table; a row a live process still owns is listed and left alone. Spaces restrict the scope; default is every stranded row. `-y`, `--yes`. |
| `dmux recovery <SUB>` | Guarded Wez mux recovery, always executed at the backend owner and qualified with the exact backend-instance/epoch pair, so a restart between inspection and mutation is a stale-target refusal. `status`; `resume`; `abort` with `-y`, `--yes`. |
| `dmux ssh <TARGET>` | Enroll a host over SSH and open an interactive session on it. |
| `dmux host <SUB>` | `ls`: enrolled hosts and their routes (deprecated `--json`); `label <HOST> <NEW_LABEL>`: set a friendly label; `forget <HOST>` with `-y`: disable a host's routes and tombstone its refs, never the local host, and re-enrolling reactivates it. `<HOST>` is an alias, a current label, or a HostUid. |
| `dmux keys` | Show the live wezterm and tmux key bindings. `--man`: render as a man page; `--tmux` / `--wez`: only one table. |
| `dmux doctor` | Probe the environment transport selection depends on. `--format json` for the envelope; deprecated `--json` for the bare probe object. |
| `dmux migrate` | The one-time cutover that brings existing sessions and workspaces under management. `--commit`: apply the printed plan, which is otherwise only previewed; `-y`, `--yes`. Not implemented — it refuses instead of acting. |

`DMUX_WEZ_FIRST=1` gates the Wez-first behaviour, and it is unset by default.
While it is off, `ls` refuses `--all-hosts`/`--backend`/`--tree`/`--format`
and falls back to the legacy merged wezterm+tmux listing, whose row numbers
are assigned over the merged set before `--tmux`/`--wez` filter; `con` refuses
`--name`/`--backend`/`--group`/`--split`/`--launch-gui`; `new` refuses
`--backend`/`--no-connect`/`--allow-name-collision`/`--launch-gui`; `rm` and
`rename` refuse `--name`/`--row`/`--backend`/`--format` (and
`--allow-name-collision`); and `adopt` and `migrate` refuse outright.
`group`, `split`, `context`, `repair`, `recovery`, `ssh`, and `host` are not
gated. Two things the errors name do not exist yet: `migrate` answers
"migrate is not implemented yet" even under the flag, and `dmux repair
rebind` — the remedy for asserting dmux identity over an unmanaged resource —
is unimplemented, so `repair reconcile` refuses that case and names the route
that works today (rename the resource off the reserved name, reconcile again,
then `adopt` it back).

Bounded commands share one exit table: `0` success, `1` operation failure,
`2` usage, `3` not found, `4` conflict, `5` confirmation required, `6`
unavailable, `7` partial. Under `--format json` each of them prints exactly
one envelope on stdout and nothing else: `schema_version`, `ok`, `action`,
`result`, `errors`, `authority_revision`. A command's own older `--json` is
deprecated for one release and keeps emitting its bare legacy payload, with
the migration hint on stderr.

`dmux` also accepts `-h`/`--help`, `-v`/`--version`, and
`--completions <bash|elvish|fish|powershell|zsh>`. `dmx` is a shell alias for
it. `ssa` and `ssm` forward to `dmux --host archie` and `dmux --host macie`
under one narrow rule: a lone bare word that is not a dmux verb becomes
`new <word>` — create-or-connect — while everything else forwards verbatim,
so `ssa ls` lists rather than creating a Space called `ls`, and a Space whose
name collides with a verb is reached by spelling the verb (`ssa new ls`). The
verb allowlist lives in `shared/zsh/conf.d/91-tmux-attach.zsh` and is
re-derived from the built binary by
`the_wrapper_verb_allowlist_matches_the_cli` in
`scripts/rust/crates/dmux/tests/cli.rs`, so it cannot drift unnoticed.

## `dmux-rollout`

`dmux-rollout` drives one exact, resumable dmux/WezTerm release. Its private
manifest and append-only journal remember build hashes, service PIDs/epochs,
socket inodes, Space UIDs, checkpoints, and rollback files. Re-running a
completed step verifies its evidence instead of repeating the mutation.

| Command | Purpose and options |
| --- | --- |
| `dmux-rollout plan` | Freeze pushed dotfiles and WezTerm commits. `--dotfiles-ref`, `--wezterm-ref`, `--release-id`, `--smoke-name`; an existing smoke may be adopted only with both `--smoke-space-uid` and `--smoke-host-uid`. |
| `dmux-rollout build` | Test/build Mac artifacts from clean detached worktrees and record exact hashes. |
| `dmux-rollout deploy-mac` | Back up and atomically install dmux/WezTerm, re-sign the app, restart only the exact launchd service, and verify a new PID/epoch/socket. `--approve-space <UID>` explicitly permits a pre-existing live Space. |
| `dmux-rollout stage-archie` | Build Archie user binaries and packages in exact remote worktrees, record hashes and rollback archives, then print—but never run—the exact interactive `sudo pacman -U` command. |
| `dmux-rollout resume` | Detect that the staged packages were installed, atomically install Archie user binaries, restart the exact user service, and verify it. Exits 4 with the required pacman command while paused. |
| `dmux-rollout verify` | Reuse one journaled smoke identity through cold presentation, reconnect, managed lifecycle, service recovery, explicit removal, and two-host checks. |
| `dmux-rollout rollback` | Restore only recorded binaries/packages and exact service environment. Registry rows, tombstones, recovery manifests, and user state are deliberately preserved. Exits 4 at Archie's interactive package step. |
| `dmux-rollout status [--json]` | Show the active release and its completed checkpoints or the full manifest. |

Use global `--release <ID>` to operate on a non-active release. The runner
never uses broad process kills, never builds a dirty worktree, refuses
unapproved panes or stale hashes/epochs, and does not infer a replacement
target from process names or ordinals.

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
