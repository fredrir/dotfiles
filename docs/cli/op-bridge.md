# op-bridge

## Commands

<!-- cli:commands:start -->
| Command            | Description                                                                                 |
| ------------------ | ------------------------------------------------------------------------------------------- |
| `op-bridge`        | Answers archie's 1Password reads on macie after Touch ID.                                   |
| `op-bridge daemon` | Holds the tunnel to archie and answers its reads until stopped; run by launchd.             |
| `op-bridge setup`  | Installs the signed `~/Applications/op-bridge.app` on macie and restarts its launchd agent. |
| `op-bridge op`     | Runs as `op` on archie: reads go to macie, everything else to the real `op`.                |
<!-- cli:commands:end -->

## Flags

<!-- cli:flags:start -->
| Flag                    | Description                                                                                               |
| ----------------------- | --------------------------------------------------------------------------------------------------------- |
| `--vault <VAULT>`       | Selects a vault archie may read; repeat for more. Defaults to `Dev`.                                      |
| `--identity <NAME>`     | Selects the code signing identity by any unique part of its name. Defaults to `Developer ID Application`. |
| `-n`, `--dry-run`       | Shows the missing setup steps without applying them.                                                      |
| `-h`, `--help`          | Shows help for the selected command and exits.                                                            |
| `--completions <SHELL>` | Prints a shell completion script for the named shell and exits.                                           |
| `-V`, `--version`       | Prints the version and exits.                                                                             |
<!-- cli:flags:end -->

## Path

```
archie                                      macie
op read op://Dev/…                          ~/Applications/op-bridge.app (launchd)
  └─ ~/.local/bin/op                          ├─ vault allowlist
      └─ $XDG_RUNTIME_DIR/op-bridge.sock ═ssh -R═ ~/.local/state/op-bridge/broker.sock
                                              ├─ Touch ID: "send op://Dev/… to archie"
                                              └─ op read (1Password app)
```

| Name | Value |
| --- | --- |
| Vaults | `Dev`; `--vault` in `macos/launchd/com.fredrir.op-bridge.plist` |
| Prompt | Touch ID only; no password, no Apple Watch |
| Grant | 30 min per reference, from approval; memory only |
| Callers | the daemon's own `ssh` child; any other pid is rejected |
| Tunnel | `ssh archie`, its own connection; retried every 10 s |
| Log | reference and outcome, never the value |

## `op` on archie

| Call | Goes to |
| --- | --- |
| `op read [-n] op://Dev/…`, bridge up | macie |
| Touch ID declined | error, exit 1 |
| `op` failed on macie | `/usr/bin/op`, reason on stderr |
| another vault, bridge down, any other command | `/usr/bin/op` |

| Env | Default |
| --- | --- |
| `OP_BRIDGE_SOCKET` | `${XDG_RUNTIME_DIR:-/run/user/$UID}/op-bridge.sock` |
| `OP_BRIDGE_OP` | `/usr/bin/op` |
| `XDG_STATE_HOME` | `~/.local/state`; holds `op-bridge/broker.sock` on macie |

Bridge down, e.g. macie asleep; sign in before starting pi, which cannot prompt:

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
