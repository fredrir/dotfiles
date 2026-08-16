# Spike 5 — Kill Convergence, tmux Passthrough, Stock CLI Surface (P0 feasibility)

Date: 2026-08-16. Host: macOS (Darwin 25.5.0).
wezterm `20260813-114614-18a44cb7` (fredrir fork) at `/opt/homebrew/bin/wezterm` + `/opt/homebrew/bin/wezterm-mux-server`.
tmux `3.7b` at `/opt/homebrew/bin/tmux`.
All scratch state under `/tmp/dmux-s5/` (archived to `scratchpad/spike5/artifacts/`). Isolated tmux server label `dmux-spike5` (killed at end). No live sockets in `~/.local/share/wezterm` were touched; the one socket file my processes created (`gui-sock-41234`, plus a transient accidental `sock` bind, see §0.1) was removed after its owning process died.

Invocation wrapper used for ALL wezterm cli calls (`/tmp/dmux-s5/wcli.sh`):

```sh
#!/bin/sh
# wcli.sh -- scratch-scoped wezterm cli wrapper (spike5)
exec env -u WEZTERM_PANE -u TMUX -u TMUX_PANE \
  WEZTERM_UNIX_SOCKET=/tmp/dmux-s5/sock \
  /opt/homebrew/bin/wezterm --config-file /tmp/dmux-s5/wez.lua cli --no-auto-start "$@"
```

## 0. Scratch mux server setup — and a socket-path gotcha

### 0.1 GOTCHA: `WEZTERM_UNIX_SOCKET` is NOT honored by `wezterm-mux-server --daemonize`

First attempt:

```
env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=/tmp/dmux-s5/sock \
  wezterm-mux-server --config-file /tmp/dmux-s5/wez.lua --daemonize
```

Result: the daemonized server bound the DEFAULT path `~/.local/share/wezterm/sock` (verified with lsof), ignoring the env var. The env var works for *clients*, but the server's listen path must come from config. Killed that server immediately.

**Rule for dmux**: mux server socket path MUST be pinned in config:

```lua
config.unix_domains = {
  { name = 'dmux-s5', socket_path = '/tmp/dmux-s5/sock' },
}
```

With that config (run foregrounded under our own supervision, no `--daemonize`), the server bound `/tmp/dmux-s5/sock` correctly (verified via lsof).

Full scratch mux config (`/tmp/dmux-s5/wez.lua`, final form incl. §2.1 mux-side probe handler):

```lua
-- scratch config for dmux spike5 mux server (Task 1 & 3)
local wezterm = require 'wezterm'
local config = wezterm.config_builder and wezterm.config_builder() or {}
config.default_prog = { '/bin/zsh', '-l' }
wezterm.on('user-var-changed', function(window, pane, name, value)
  local f = io.open('/tmp/dmux-s5/mux-uservar.log', 'a')
  f:write(string.format('MUX-SIDE user-var-changed pane=%s name=%s value=%s\n',
    pane and tostring(pane:pane_id()) or 'nil', tostring(name), tostring(value)))
  f:close()
end)
config.unix_domains = {
  { name = 'dmux-s5', socket_path = '/tmp/dmux-s5/sock' },
}
return config
```

Note: `wezterm-mux-server` immediately spawns one pane running `default_prog` in workspace `default` (pane 0 below). All tests scope by workspace name and never touched it — good scoping proof.

## 1. Task 1 — Bounded re-list/kill convergence (no atomic kill primitive)

### 1.0 The loop script (verbatim, `/tmp/dmux-s5/kill-workspace.sh`)

```sh
#!/bin/sh
# kill-workspace.sh <workspace> [max_rounds] -- bounded re-list/kill convergence loop (spike5)
WS="$1"; MAX="${2:-5}"
W=/tmp/dmux-s5/wcli.sh
round=0
while : ; do
  panes=$($W list --format json | /usr/bin/jq -r --arg ws "$WS" '.[] | select(.workspace==$ws) | .pane_id')
  count=$(printf '%s' "$panes" | grep -c '^[0-9]')
  echo "round=$round observed_count=$count panes=[$(printf '%s' "$panes" | tr '\n' ' ')]"
  if [ "$count" -eq 0 ]; then
    echo "RESULT=CONVERGED rounds_used=$round"
    exit 0
  fi
  round=$((round+1))
  if [ "$round" -gt "$MAX" ]; then
    echo "RESULT=NOT_CONVERGED bound=$MAX remaining_panes=[$(printf '%s' "$panes" | tr '\n' ' ')]"
    exit 3
  fi
  for p in $panes; do
    $W kill-pane --pane-id "$p" 2>/tmp/dmux-s5/kp-err.$$ ; rc=$?
    [ $rc -ne 0 ] && echo "  kill-pane pane_id=$p exit=$rc stderr=$(cat /tmp/dmux-s5/kp-err.$$)"
  done
  rm -f /tmp/dmux-s5/kp-err.$$
done
```

Exit contract: `0` = converged (workspace empty at last observation), `3` = bound hit, remaining pane ids printed.

### 1.1 Workspace construction (4 panes, 2 windows, one workspace)

```
p1=$(wcli spawn --new-window --workspace wsA -- /bin/sleep 600)   # -> pane 1 (window 1)
p2=$(wcli split-pane --pane-id $p1 --right  -- /bin/sleep 600)    # -> pane 2 (split)
p3=$(wcli split-pane --pane-id $p1 --bottom -- /bin/sleep 600)    # -> pane 3 (split)
p4=$(wcli spawn --new-window --workspace wsA -- /bin/sleep 600)   # -> pane 4 (window 2, models external multi-window resource)
```

`wcli list` before the run:

```
WINID TABID PANEID WORKSPACE SIZE  TITLE CWD
    0     0      0 default   80x24 ~     ...
    1     1      1 wsA       39x11 sleep ...
    1     1      3 wsA       39x12 sleep ...
    1     1      2 wsA       40x24 sleep ...
    2     2      4 wsA       80x24 sleep ...
```

### 1.2 (a) Clean convergence — PASS

`kill-workspace.sh wsA 5` (raw log `run-clean.log`):

```
round=0 observed_count=4 panes=[1 3 2 4]
round=1 observed_count=0 panes=[]
RESULT=CONVERGED rounds_used=1
script-exit=0
```

Post-run `wcli list` shows ONLY the default pane — workspace `wsA` fully disappeared from `cli list` after its last pane died (windows and tabs disappear implicitly with their last pane; there is no lingering empty window/workspace object):

```
WINID TABID PANEID WORKSPACE SIZE  TITLE CWD
    0     0      0 default   80x24 ~     ...
```

### 1.3 (b) kill-pane on an already-dead pane — clean error, exit 1

```
$ wcli kill-pane --pane-id 4        # pane 4 was killed in 1.2
ERROR  wezterm > unexpected response Ok(ErrorResponse(ErrorResponse { reason: "Error: no such pane 4" })); terminating
exit=1
$ wcli kill-pane --pane-id 999      # never existed
ERROR  wezterm > unexpected response Ok(ErrorResponse(ErrorResponse { reason: "Error: no such pane 999" })); terminating
exit=1
```

No hang, no side effects. So racing kills are safe: a pane killed by someone else between list and kill produces exit 1 + "no such pane", which the loop can treat as success-equivalent.

### 1.4 (c) Adversarial concurrent spawner

Adversary (verbatim, `/tmp/dmux-s5/adversary.sh`; `adversary-tight.sh` is identical with the `sleep 0.3` removed):

```sh
#!/bin/sh
# adversary.sh -- concurrently spawns a new window/pane into workspace wsA every ~0.3s
rm -f /tmp/dmux-s5/adversary.stop
: > /tmp/dmux-s5/adversary.log
while [ ! -f /tmp/dmux-s5/adversary.stop ]; do
  id=$(/tmp/dmux-s5/wcli.sh spawn --new-window --workspace wsA -- /bin/sleep 600 2>>/tmp/dmux-s5/adversary.log)
  echo "$(date +%H:%M:%S) spawned pane_id=$id" >> /tmp/dmux-s5/adversary.log
  sleep 0.3
done
echo "adversary stopped" >> /tmp/dmux-s5/adversary.log
```

#### 1.4.1 SLOW adversary (0.3s cadence): loop can WIN the race — and that "success" is only point-in-time

Rebuilt wsA (panes 5,6,7,8), started the slow adversary, ran `kill-workspace.sh wsA 5` (raw log `run-adversarial.log`):

```
round=0 observed_count=16 panes=[15 5 7 6 18 13 14 16 10 19 17 20 8 12 9 11]
round=1 observed_count=0 panes=[]
RESULT=CONVERGED rounds_used=1
```

The kill burst (16 `kill-pane` calls, ~15ms each over the local socket) finished inside the adversary's 0.3s sleep window, so re-list observed 0. **BUT** the adversary was still alive; seconds later:

```
wsA-panes-now=46          # jq count of wsA panes in cli list json
```

**Finding (TOCTOU)**: "converged" from the loop means only "workspace empty at observation time". A live concurrent spawner recreates the workspace immediately after exit 0. A bounded loop cannot promise post-exit absence; dmux must treat convergence as point-in-time and either (a) own/fence all spawners for the workspace, or (b) re-verify after a grace period.

#### 1.4.2 TIGHT adversary (no sleep): bound hit, honest failure report — PASS

Tight adversary running, wsA at 46+ panes and growing, `kill-workspace.sh wsA 5` (raw log `run-adversarial-tight.log`; pane lists elided here, full lists in the log):

| round | observed_count |
|------:|---------------:|
| 0 | 227 |
| 1 | 114 |
| 2 | 58 |
| 3 | 29 |
| 4 | 15 |
| 5 | 8 |

```
RESULT=NOT_CONVERGED bound=5 remaining_panes=[465 468 466 464 470 469 ...]
```

- Loop terminated at exactly the bound (5 rounds), did NOT loop forever.
- It REPORTED the remaining pane ids and did NOT report success.
- Separately verified the failure exit code: a run with `MAX=0` against a 1-pane workspace returned shell exit **3**.
- Count decreasing per round shows kill (cheap RPC) outpaces spawn-new-window (heavy), but with continuous spawning the count never reaches 0 inside the bound.

#### 1.4.3 (d) Rerun after stopping adversary — converges — PASS

Adversary stopped via stop-file (log tail: `adversary stopped`; ~750 total spawns during the fight). Rerun (raw log `run-post-adversary.log`):

```
round=0 observed_count=383 panes=[650 738 480 ... 594]   # full 383-id list in raw log
round=1 observed_count=0 panes=[]
RESULT=CONVERGED rounds_used=1
script-exit=0
```

Post-run `cli list`: only default pane 0. 383 panes/windows killed in a single round; every kill succeeded.

### 1.5 Convergence matrix (summary)

| Scenario | Rounds used | Result | Exit |
|---|---|---|---|
| Clean, 4 panes / 2 windows | 1 | CONVERGED, workspace gone from `cli list` | 0 |
| kill-pane on dead/nonexistent pane | n/a | "no such pane" stderr, no hang | 1 |
| Slow adversary (0.3s) | 1 | CONVERGED at observation time, workspace re-created seconds later (TOCTOU) | 0 |
| Tight adversary (no sleep) | bound=5 hit | NOT_CONVERGED, remaining ids reported (227→114→58→29→15→8) | 3 |
| Rerun after adversary stopped (383 panes) | 1 | CONVERGED | 0 |

## 2. Task 2 — tmux passthrough of WezTerm user-var OSC

### 2.1 User-var observability: GUI vs mux vs `cli list`

Probe (headless scratch mux server, `user-var-changed` handler in mux config, no GUI attached): spawned a `zsh -f` pane, drove it with `cli send-text --no-paste`, made it emit a raw SetUserVar OSC:

```
printf "\033]1337;SetUserVar=dmux_muxprobe=aGVsbG8tbXV4\007"
```

Results:

- **mux-side Lua**: `wezterm.on('user-var-changed', ...)` in the mux-server config did NOT fire (log file never created), even though `get-text` proves the pane consumed the sequence (the escape was parsed, not shown as literal bytes). On this build, headless wezterm-mux-server does not deliver `user-var-changed` to config Lua.
- **`cli list --format json`**: pane objects carry NO user-var field. Full key set observed: `window_id, tab_id, pane_id, workspace, size, title, cwd, cursor_x, cursor_y, cursor_shape, cursor_visibility, left_col, top_row, tab_title, window_title, is_active, is_zoomed, tty_name`. Same schema when listing via the GUI socket.
- **GUI-side Lua**: `user-var-changed` fires reliably (see below).

**Conclusion**: the ONLY observation channel for user vars is a GUI-side `user-var-changed` handler. dmux marker consumption must live in GUI config (or a fork primitive would have to add mux-side delivery / cli exposure).

### 2.2 Scratch GUI harness

Scratch GUI (own config, own class `dmux-spike5-gui`, pane runs the isolated tmux client directly; the GUI's own `gui-sock-<pid>` was the only file it created and it was removed after kill). `/tmp/dmux-s5/gui.lua`:

```lua
-- scratch GUI config for dmux spike5 (Task 2: user-var passthrough observation)
local wezterm = require 'wezterm'
local config = wezterm.config_builder and wezterm.config_builder() or {}
-- pane runs the tmux client for the isolated spike server directly
config.default_prog = { '/opt/homebrew/bin/tmux', '-L', 'dmux-spike5', 'new-session', '-A', '-s', 'spike' }
config.window_close_confirmation = 'NeverPrompt'
wezterm.on('user-var-changed', function(window, pane, name, value)
  local f = io.open('/tmp/dmux-s5/uservar.log', 'a')
  f:write(string.format('%s GUI user-var-changed pane=%s name=%s value=%s\n',
    os.date('%H:%M:%S'), tostring(pane:pane_id()), tostring(name), tostring(value)))
  f:close()
end)
return config
```

Launch: `env -u WEZTERM_PANE -u TMUX -u TMUX_PANE wezterm --config-file /tmp/dmux-s5/gui.lua start --class dmux-spike5-gui`. All in-tmux typing driven externally via `tmux -L dmux-spike5 send-keys -t spike:...`.

Emitters (verbatim):

```sh
#!/bin/sh
# emit-wrapped.sh <name> <value> -- tmux-wrapped SetUserVar (DCS tmux; passthrough)
# byte sequence: ESC P t m u x ; ESC ESC ] 1 3 3 7 ; SetUserVar=<name>=<b64> BEL ESC \
name="${1:-dmux_test}"; val="${2:-payload}"
b64=$(printf '%s' "$val" | /usr/bin/base64)
printf '\033Ptmux;\033\033]1337;SetUserVar=%s=%s\007\033\\' "$name" "$b64"
```

```sh
#!/bin/sh
# emit-plain.sh <name> <value> -- UNwrapped OSC 1337 SetUserVar (control: should NOT escape tmux)
name="${1:-dmux_plain}"; val="${2:-payload}"
b64=$(printf '%s' "$val" | /usr/bin/base64)
printf '\033]1337;SetUserVar=%s=%s\007' "$name" "$b64"
```

```sh
#!/bin/sh
# emit-title-wrapped.sh <title> -- tmux-wrapped OSC 2 (set window title) passthrough
printf '\033Ptmux;\033\033]2;%s\007\033\\' "${1:-DMUX-TITLE-TEST}"
```

### 2.3 tmux 3.7b `allow-passthrough` defaults

- **Compiled-in default is `off`**: vanilla server probe `tmux -L <label> -f /dev/null start-server \; show -g allow-passthrough` → `allow-passthrough off`.
- **This user's environment overrides it to `on`**: `/Users/fredrir/dotfiles/shared/tmux/00-core.conf:11: set -g allow-passthrough on` (sourced even by `-L`-isolated servers because `-L` only changes the socket, not config resolution). First emit test landed "at default" because of this — re-tested with the option explicitly `off`.
- man 3.7b: `allow-passthrough [on | off | all]` — "If set to **on**, passthrough sequences will be allowed **only if the pane is visible**. If set to **all**, they will be allowed even if the pane is invisible."

### 2.4 Passthrough test matrix (GUI log = ground truth)

| case | allow-passthrough | pane visible | sequence | landed in GUI? |
|---|---|---|---|---|
| (a) | **off** | yes | wrapped SetUserVar | **NO** (log empty) |
| (b) | **on** | yes | wrapped SetUserVar | **YES** — `GUI user-var-changed pane=0 name=dmux_test value=val-when-ON` |
| (c) | on | yes | **plain unwrapped** OSC 1337 | **NO** (tmux consumes/discards unknown OSC; never reaches outer terminal) |
| (e1) | on | **no** (window 2, not selected) | wrapped SetUserVar | **NO** — silently dropped, and NOT buffered (never arrived later) |
| (e2) | **all** | no | wrapped SetUserVar | **YES** — `name=dmux_vis value=from-invisible-ALL` |
| (d) | all | yes | wrapped OSC 2 title | **YES** — outer pane/window title became `DMUX-TITLE-VIA-PASSTHROUGH` (observed via `cli list --format json` on the GUI's socket; bypasses tmux's own title state) |

Raw GUI log lines:

```
13:41:47 GUI user-var-changed pane=0 name=dmux_test value=val-default-state   (user-config on, pre-matrix probe)
13:44:46 GUI user-var-changed pane=0 name=dmux_test value=val-when-ON
13:45:04 GUI user-var-changed pane=0 name=dmux_vis value=from-invisible-ALL
```

### 2.5 FROZEN PASSTHROUGH RECIPE

Required tmux option (dmux must enforce, not assume): `set -g allow-passthrough all`
(`on` is insufficient for markers emitted from non-visible tmux windows/panes — they are silently and permanently dropped, not queued. `off` — the tmux compiled-in default — blocks everything.)

Byte sequence (name `dmux_test`, value `hello` → base64 `aGVsbG8=`):

```
ESC P t m u x ;  ESC ESC ] 1 3 3 7 ; S e t U s e r V a r = <name> = <base64(value)>  BEL  ESC \
```

Hexdump of `emit-wrapped.sh dmux_test hello`:

```
00000000: 1b50 746d 7578 3b1b 1b5d 3133 3337 3b53  .Ptmux;..]1337;S
00000010: 6574 5573 6572 5661 723d 646d 7578 5f74  etUserVar=dmux_t
00000020: 6573 743d 6147 5673 6247 383d 071b 5c    est=aGVsbG8=..\
```

Shell one-liner:

```sh
printf '\033Ptmux;\033\033]1337;SetUserVar=%s=%s\007\033\\' "$NAME" "$(printf '%s' "$VALUE" | base64)"
```

Rules: wrap in DCS `ESC P tmux ;` ... `ESC \`; every ESC inside the payload is doubled (hence `1b 1b` before `]1337`); OSC payload terminated with BEL (0x07) — the doubled-ESC rule makes an ESC-\\ OSC terminator awkward, BEL avoids it; value MUST be base64. Title passthrough works identically with OSC 2 payload (`1b 1b 5d 32 3b ... 07`).

## 3. Task 3 — Stock CLI surface inventory (fork build 20260813-114614-18a44cb7)

### 3.1 Full `wezterm cli --help` (verbatim)

```
Interact with experimental mux server

Usage: wezterm cli [OPTIONS] <COMMAND>

Commands:
  list                     list windows, tabs and panes
  list-clients             list clients
  proxy                    start rpc proxy pipe
  tlscreds                 obtain tls credentials
  move-pane-to-new-tab     Move a pane into a new tab
  split-pane               split the current pane.
                           Outputs the pane-id for the newly created pane on success
  spawn                    Spawn a command into a new window or tab
                           Outputs the pane-id for the newly created pane on success
  send-text                Send text to a pane as though it were pasted. If bracketed paste mode is enabled in the pane, then the text will be sent as a bracketed paste
  get-text                 Retrieves the textual content of a pane and output it to stdout
  activate-pane-direction  Activate an adjacent pane in the specified direction
  get-pane-direction       Determine the adjacent pane in the specified direction
  kill-pane                Kill a pane
  activate-pane            Activate (focus) a pane
  adjust-pane-size         Adjust the size of a pane directionally
  activate-tab             Activate a tab
  set-tab-title            Change the title of a tab
  set-window-title         Change the title of a window
  rename-workspace         Rename a workspace
  zoom-pane                Zoom, unzoom, or toggle zoom state
  help                     Print this message or the help of the given subcommand(s)

Options:
      --no-auto-start  Don't automatically start the server
      --prefer-mux     Prefer connecting to a background mux server. The default is to prefer connecting to a running wezterm gui instance
      --class <CLASS>  When connecting to a gui instance, if you started the gui with `--class SOMETHING`, you should also pass that same value here in order for the client to find the correct gui instance
  -h, --help           Print help
```

This is the STOCK upstream cli surface — the fork adds no cli subcommands. Full per-subcommand `--help` captures archived at `scratchpad/spike5/help-*.txt`; the load-bearing ones:

- `rename-workspace [OPTIONS] <NEW_WORKSPACE>` — options: `--workspace <WORKSPACE>` (select source by NAME), `--pane-id <PANE_ID>` (resolve workspace from a pane; default from `$WEZTERM_PANE`). No window-id option, no compare-and-swap.
- `spawn` — `--pane-id`, `--domain-name`, `--window-id` (spawn tab into existing window; "Cannot be used with --workspace or --new-window"), `--new-window`, `--cwd`, `--workspace <WORKSPACE>` ("When creating a new window... Requires --new-window").
- `split-pane` — `--pane-id`, direction flags, `--top-level`, `--cells/--percent`, `--cwd`, `--move-pane-id` (move existing pane into the new split). No workspace option.
- `kill-pane` — `--pane-id` only.
- `list` / `list-clients` — `--format table|json`.
- `set-window-title` — `--window-id` or `--pane-id` + `<TITLE>`.
- `set-tab-title` — `--tab-id` or `--pane-id` + `<TITLE>`.
- `activate-pane` — `--pane-id`. `activate-tab` — `--tab-id | --tab-index | --tab-relative [--no-wrap] | --pane-id`.
- `move-pane-to-new-tab` — `--pane-id`, `--window-id`, `--new-window`, `--workspace <WORKSPACE>` ("If creating a new window, override the default workspace name").
- `set-pane-workspace` — DOES NOT EXIST (`error: unrecognized subcommand`, exit 2).

### 3.2 `rename-workspace` observed semantics (scratch server, live tests)

Setup: windows 844+845 in workspace `renA`, window 846 in `renB`.

| test | command | exit | observed effect |
|---|---|---|---|
| R1 shared name | `rename-workspace --workspace renA renX` | 0 | **BOTH** windows 844+845 renamed to renX. Name-scoped: renames every window carrying the name. |
| R2 missing source | `rename-workspace --workspace nosuchws foo` | **0** | **Silent no-op.** No error, no change. Failure is undetectable from exit code. |
| R3 target collision | `rename-workspace --workspace renX renB` | 0 | **Silent merge**: renX windows joined existing renB (3 windows now share renB). No collision error. |
| R4 by pane-id | `rename-workspace --pane-id 850 renSolo` | 0 | `--pane-id` merely RESOLVES the pane's workspace NAME, then renames globally: ALL 3 renB windows (incl. 844/845, unrelated to pane 850's window) moved to renSolo. NOT pane/window-scoped. |

### 3.3 Workspace-assignment surface: conclusion for the fork-primitive decision

- **No compare-and-swap anywhere**: rename is fire-and-forget, exit 0 even on missing source; concurrent renames/merges are silent.
- **No window-id-scoped workspace verb**: the only ways stock CLI can place a window in a workspace are at window creation (`spawn --new-window --workspace X`, `move-pane-to-new-tab --new-window --workspace X`). There is NO verb to re-assign an EXISTING window to a different workspace; `rename-workspace` moves the entire name-group only.
- Combined with §2.1 (no mux-side user-var delivery, no user-vars in `cli list`), the stock surface cannot express: per-window workspace moves, CAS rename, or marker readback without a GUI. These are the concrete gaps a fork primitive would close.

## 4. Risks / unknowns

1. **TOCTOU on kill convergence** (§1.4.1): bounded loop exit 0 is point-in-time truth only. dmux needs spawn fencing or post-kill re-verification; the bound (N=5) is otherwise sound — honest exit 3 + remaining ids under continuous adversarial spawn.
2. **`kill-pane` races are benign** (exit 1, "no such pane"), but the loop must not treat exit 1 as fatal.
3. **`allow-passthrough all` is mandatory** for markers from invisible tmux panes; `on` (what the user's dotfiles currently set) silently drops them with no buffering. dmux must set/verify `all` per session, not trust ambient config. Compiled default is `off`.
4. **User-var observation requires a GUI**: headless mux-server does not fire `user-var-changed` into config Lua and `cli list` json has no user-var field on this build. Marker-driven flows die if no GUI is attached — either accept that or add a fork primitive (mux-side event delivery or user-vars in `cli list` json).
5. **`WEZTERM_UNIX_SOCKET` ignored by `wezterm-mux-server --daemonize`** — server socket must be pinned via `unix_domains[].socket_path` in config or a scratch/managed server can bind the user's default `~/.local/share/wezterm/sock` (observed live).
6. **rename-workspace silent-success semantics** (§3.2) make error handling impossible at the CLI layer: adoption/rename logic based on it must pre-verify via `cli list` json and re-verify after, accepting the race window, or use a fork primitive.
7. Untested here: passthrough through NESTED tmux (tmux-in-tmux needs double wrapping), and wezterm's `cli send-text` interaction with tmux copy-mode. Title passthrough overwrites the outer title outside tmux's knowledge — tmux will clobber it on next refresh if its own title machinery is active.

## 5. Artifacts

- Scripts/configs/logs (verbatim copies): `scratchpad/spike5/artifacts/` — `wcli.sh`, `kill-workspace.sh`, `adversary.sh`, `adversary-tight.sh`, `wez.lua`, `gui.lua`, `emit-wrapped.sh`, `emit-plain.sh`, `emit-title-wrapped.sh`, `run-clean.log`, `run-adversarial.log`, `run-adversarial-tight.log`, `run-post-adversary.log` (full 383-pane listing), `adversary.log` (757 lines, every spawned pane id), `uservar.log`, `wrapped-bytes.txt`.
- Full CLI help captures: `scratchpad/spike5/help-*.txt` (all 17 subcommands + top level), combined key set in `help-key-combined.txt`.
- PIDs used: `scratchpad/spike5/pids.txt`.
- Teardown verified: `tmux -L dmux-spike5 kill-server` done ("no server running" on re-probe); scratch GUI (pid 41234) and scratch mux server (pid 40944) killed; no `dmux-spike5`/`dmux-s5` processes remain; `gui-sock-41234` and `/tmp/dmux-s5/sock` files removed; vanilla probe server `dmux-spike5v` self-killed. Pre-existing sockets and the parallel spike agents' processes (spike1/3/4) untouched.
