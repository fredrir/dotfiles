# op-bridge

## Commands

<!-- cli:commands:start -->
| Command            | Description                                                                                 |
| ------------------ | ------------------------------------------------------------------------------------------- |
| `op-bridge`        | Serves 1Password reads to archie and macie from macie, by vault tier.                       |
| `op-bridge daemon` | Holds the tunnel to archie and serves reads until stopped; run by launchd.                  |
| `op-bridge reload` | Clears the daemon's memory and refetches every `Dev` reference; run after rotating a key.   |
| `op-bridge setup`  | Installs the signed `~/Applications/op-bridge.app` on macie and restarts its launchd agent. |
| `op-bridge op`     | Runs as `op`: reads go to the daemon, everything else to the real `op`.                     |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                     | Description                                                                                               |
| ------------------------ | --------------------------------------------------------------------------------------------------------- |
| `--vault <VAULT>`        | Selects a vault served without Touch ID until macie sleeps; repeat for more. Defaults to `Dev`.           |
| `--prompt-vault <VAULT>` | Selects a vault that needs Touch ID per reference; repeat for more.                                       |
| `--identity <NAME>`      | Selects the code signing identity by any unique part of its name. Defaults to `Developer ID Application`. |
| `-n`, `--dry-run`        | Shows the missing setup steps without applying them.                                                      |
| `-h`, `--help`           | Shows help for the selected command and exits.                                                            |
| `--completions <SHELL>`  | Prints a shell completion script for the named shell and exits.                                           |
| `-V`, `--version`        | Prints the version and exits.                                                                             |
<!-- cli:flags:end -->

## Path

```
archie                                      macie
op read op://Dev/…                          ~/Applications/op-bridge.app (launchd)
  └─ ~/.local/bin/op                          ├─ tier by vault
      └─ $XDG_RUNTIME_DIR/op-bridge.sock ═ssh -R═ ~/.local/state/op-bridge/broker.sock
                                              ├─ Secure: Touch ID "send op://Secure/… to archie"
op read op://Dev/… (macie) ───────────────────┤
                                              └─ op read (1Password app)
```

## Tiers

| Name | `Dev` (`--vault`) | `Secure` (`--prompt-vault`) |
| --- | --- | --- |
| op-bridge Touch ID | never | per reference |
| Held in memory | until macie sleeps | 30 min, or until macie sleeps |
| Callers | archie and macie | archie and macie |
| Refill | after startup or wake, once the screen is unlocked | never |

| Name | Value |
| --- | --- |
| Configured | `macos/launchd/com.fredrir.op-bridge.plist` |
| Sleep | wall clock more than 30 s ahead of the monotonic clock |
| Refill list | `~/.local/state/op-bridge/known.json`; references only, never values |
| Refill prompt | 1Password's own, if its CLI session lapsed; one per wake |
| Rotated key | `op-bridge reload` on either host |
| Reload, daemon unreachable | exit 3; nothing cached |
| Tunnel | `ssh archie`, its own connection; retried every 10 s |
| Log | reference and outcome, never the value |

## `op`

`~/.local/bin/op` on both hosts, ahead of the real `op` in `PATH`.

| Call | Goes to |
| --- | --- |
| `op read [-n] op://Dev/…`, bridge up | the daemon |
| Touch ID declined | error, exit 1 |
| `op` failed on macie, vault not served | the real `op`, reason on stderr |
| bridge down, any other command | the real `op` |

| Env | Default |
| --- | --- |
| `OP_BRIDGE_SOCKET` | macie: `~/.local/state/op-bridge/broker.sock`; archie: `${XDG_RUNTIME_DIR:-/run/user/$UID}/op-bridge.sock` |
| `OP_BRIDGE_OP` | macie: `/opt/homebrew/bin/op`; archie: `/usr/bin/op` |
| `XDG_STATE_HOME` | `~/.local/state` |

Bridge down, e.g. macie asleep; sign in on archie before starting pi, which cannot prompt:

```console
$ op account add          # once
$ eval $(op signin)
```

## pi

`auth.json` commands run once per pi process; `models.json` commands run on every request.

```json
{
  "deepseek": { "type": "api_key", "key": "!op read op://Dev/deepseek/credential" }
}
```

| Name | Value |
| --- | --- |
| File | `~/.pi/agent/auth.json` on both hosts |
| Custom providers | move `apiKey` from `models.json` to an `auth.json` entry |
| Timeout | 10 s; a missed prompt leaves the key unset until pi restarts |
| `pi -p` from agents | silent for `Dev` while macie is awake |
| `openai-codex` | OAuth; pi rewrites it, stays in `auth.json` |

## Setup

Once on macie after `dotfile sync`, and again after a rebuild:

```console
$ op-bridge setup --dry-run
$ op-bridge setup
```

| Step | Value |
| --- | --- |
| App | `~/Applications/op-bridge.app`, `com.fredrir.op-bridge`, signed with `--identity` |
| Daemon | `~/Library/LaunchAgents/com.fredrir.op-bridge.plist`, runs the app's executable |
| Permission | Privacy & Security → Files & Folders → op-bridge → Data shared by 1Password |
| Why signed | the permission follows the signing team and bundle ID; an ad-hoc build loses it on rebuild |

## Operations

| Name | Value |
| --- | --- |
| Restart | `launchctl kickstart -k gui/$(id -u)/com.fredrir.op-bridge` |
| Log | `~/Library/Logs/op-bridge.log` |
| Bridge up | `test -S "$XDG_RUNTIME_DIR/op-bridge.sock"` on archie |
