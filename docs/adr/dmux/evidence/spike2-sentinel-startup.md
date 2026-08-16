# Spike 2 — service/sentinel startup model (dmux plan §15.1 / §15.3)

- Date: 2026-08-16 (UTC timestamps in logs)
- Host: macOS (Darwin 25.5.0), user `fredrir`
- Binary: `wezterm 20260813-114614-18a44cb7` (fredrir fork) at `/opt/homebrew/bin/wezterm` / `/opt/homebrew/bin/wezterm-mux-server`
- Live WezTerm GUI (PID 9640, `gui-sock-9640`) was running the whole time and was never targeted; all deliberate traffic used `WEZTERM_UNIX_SOCKET=/tmp/dmux-s2/sock`.
- Scratch: `/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike2/` (`$SCRATCH` below)
- Socket: `/tmp/dmux-s2/sock` (17 bytes, well under the 104-byte sun_path limit; `/tmp/dmux-s2` created for this)
- Sibling spike agents were running their own mux-servers under `/tmp/dmux-s1`, `/tmp/dmux-s3..s5`, `scratchpad/spike3/4`; none were touched. PID ledger: `$SCRATCH/pids.txt`.

## Verdict summary

| Task | Result |
|---|---|
| 1. Sentinel-only startup, both start paths | PASS — exactly 1 pane, sentinel workspace, no default shell (foreground AND `--daemonize`) |
| 2. Exactly-once per start; fresh epoch on restart | PASS — one `BEGIN` per server PID; 4 starts → 4 distinct epochs |
| 3a. In-process `wezterm.mux.all_windows()` during mux-startup | PASS — works before and after spawns |
| 3b. Restore-all-shape (3 windows, distinct ws/cwd/args) in-process | PASS — instantaneous, correct cwds, before any client attach |
| 3c. Recursive `wezterm cli` self-call from mux-startup | HANGS (killed by 5s watchdog, rc=137, zero output) — in-process-only rule CONFIRMED |
| 4. Auto-start knobs | `--no-auto-start` is the ONLY effective CLI knob; `no_serve_automatically=true` does NOT stop CLI auto-spawn; auto-spawn does not even propagate `--config-file` (spawns a DEFAULT-config server on the DEFAULT socket) |
| 5. Duplicate server start, same socket | SILENT SOCKET STEAL — no bind error; new server takes the path, old server orphaned; killing the thief does not heal the original |
| 6. Sentinel visible in `cli list` | PASS — `workspace` field carries the full `dmux:system:<epoch>` string |

---

## Frozen config that worked (verbatim)

`$SCRATCH/spike2.lua`:

```lua
-- dmux P0 spike2: sentinel startup model (plan §15.1 / §15.3)
local wezterm = require 'wezterm'

local SCRATCH = '/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike2'
local SOCK = '/tmp/dmux-s2/sock'

local function now()
  return os.date '!%Y-%m-%dT%H:%M:%SZ'
end

local function proc_pid()
  if wezterm.procinfo and wezterm.procinfo.pid then
    local ok, p = pcall(wezterm.procinfo.pid)
    if ok then
      return tostring(p)
    end
  end
  return '?'
end

local function log(msg)
  local f = io.open(SCRATCH .. '/mux-startup.log', 'a')
  if f then
    f:write(string.format('%s [pid=%s] %s\n', now(), proc_pid(), msg))
    f:close()
  end
end

local config = wezterm.config_builder()

config.unix_domains = {
  {
    name = 'dmux-spike2',
    socket_path = SOCK,
    no_serve_automatically = true,
  },
}

-- Identifiable marker: if WezTerm ever spawns its unmanaged default shell,
-- it will run this and show up in cli list under workspace "default".
config.default_prog = { '/bin/sh', '-c', 'echo UNMANAGED-DEFAULT-SHELL; exec /bin/sleep 600' }

-- Keep --daemonize artifacts out of ~/.local/share/wezterm (live GUI owns that dir).
config.daemon_options = {
  pid_file = '/tmp/dmux-s2/daemon.pid',
  stdout = SCRATCH .. '/daemon-stdout.log',
  stderr = SCRATCH .. '/daemon-stderr.log',
}

wezterm.on('mux-startup', function()
  local epoch = os.getenv 'DMUX_SERVER_EPOCH' or 'EPOCH-ENV-MISSING'
  local phase = os.getenv 'DMUX_SPIKE_PHASE' or '1'
  log('mux-startup BEGIN epoch=' .. epoch .. ' phase=' .. phase)

  local function write_descriptor(state, extra)
    local f = io.open(SCRATCH .. '/runtime-descriptor.json', 'w')
    if f then
      f:write(string.format('{"state":"%s","epoch":"%s","pid":%s,"socket":"%s","written_at":"%s"%s}\n',
        state, epoch, proc_pid(), SOCK, now(), extra or ''))
      f:close()
    end
  end
  write_descriptor 'starting'

  -- Task 3a: in-process inspection during startup, before any spawn.
  local ok_ins, wins = pcall(wezterm.mux.all_windows)
  log('all_windows pre-spawn ok=' .. tostring(ok_ins) .. ' count=' .. tostring(ok_ins and #wins or -1))

  -- Reserved sentinel (stand-in for `dmux _mux-idle`).
  local tab, pane, window = wezterm.mux.spawn_window {
    workspace = 'dmux:system:' .. epoch,
    args = { '/bin/sh', '-c', 'trap "" TERM; while :; do sleep 3600; done' },
  }
  log(string.format('sentinel spawned window_id=%s tab_id=%s pane_id=%s workspace=%s',
    tostring(window:window_id()), tostring(tab:tab_id()), tostring(pane:pane_id()),
    tostring(window:get_workspace())))

  if phase == '3' then
    -- Task 3b: restore-all-shape creation in-process before any client attaches.
    local shape = {
      { ws = 'dmux:u:alpha', cwd = '/tmp', args = { '/bin/sh', '-c', 'echo alpha; exec sleep 3600' } },
      { ws = 'dmux:u:beta', cwd = '/', args = { '/bin/sh', '-c', 'echo beta; exec sleep 3600' } },
      { ws = 'dmux:u:gamma', cwd = os.getenv 'HOME' or '/', args = { '/bin/zsh', '-c', 'echo gamma; exec sleep 3600' } },
    }
    for _, s in ipairs(shape) do
      local t2, p2, w2 = wezterm.mux.spawn_window { workspace = s.ws, cwd = s.cwd, args = s.args }
      log(string.format('restore-shape spawned ws=%s window_id=%s pane_id=%s cwd=%s',
        s.ws, tostring(w2:window_id()), tostring(p2:pane_id()), s.cwd))
    end
    local ok2, wins2 = pcall(wezterm.mux.all_windows)
    if ok2 then
      for _, w in ipairs(wins2) do
        log('post-spawn window id=' .. tostring(w:window_id()) .. ' ws=' .. tostring(w:get_workspace()))
      end
      log('all_windows post-spawn count=' .. tostring(#wins2))
    end

    -- Task 3c: bounded recursive `wezterm cli` probe against our OWN socket.
    local t0 = os.time()
    log 'recursive-cli probe START'
    local ph = io.popen('/bin/sh ' .. SCRATCH .. '/probe.sh 2>&1')
    local out = ph and ph:read '*a' or 'POPEN-FAILED'
    if ph then
      ph:close()
    end
    local t1 = os.time()
    log('recursive-cli probe END elapsed=' .. tostring(t1 - t0) .. 's output=[' .. out:gsub('%s+$', '') .. ']')
  end

  write_descriptor('ready',
    ',"sentinel_window_id":' .. tostring(window:window_id())
      .. ',"sentinel_tab_id":' .. tostring(tab:tab_id())
      .. ',"sentinel_pane_id":' .. tostring(pane:pane_id()))
  log('mux-startup END epoch=' .. epoch)
end)

return config
```

`$SCRATCH/probe.sh` (the bounded recursive-CLI wrapper, verbatim):

```sh
#!/bin/sh
# Bounded recursive-CLI probe, executed via io.popen from INSIDE mux-startup.
# Attempts `wezterm cli --no-auto-start list` against the server's OWN socket.
# A watchdog kills the CLI after 5s so the handler can never hang forever.
CFG=/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike2/spike2.lua
T0=$(date +%s)
env WEZTERM_UNIX_SOCKET=/tmp/dmux-s2/sock /opt/homebrew/bin/wezterm --config-file "$CFG" \
  cli --no-auto-start list --format json \
  > /tmp/dmux-s2/probe-out.json 2> /tmp/dmux-s2/probe-err.txt &
CPID=$!
( sleep 5; kill -9 "$CPID" 2>/dev/null && echo "watchdog-killed cpid=$CPID at $(date +%s)" > /tmp/dmux-s2/probe-watchdog.txt ) > /dev/null 2>&1 &
WPID=$!
wait "$CPID"
RC=$?
kill "$WPID" 2>/dev/null
T1=$(date +%s)
echo "probe rc=$RC elapsed=$((T1 - T0))s"
```

All shell-side CLI verification calls used this exact shape:

```sh
env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=/tmp/dmux-s2/sock \
  /opt/homebrew/bin/wezterm --config-file $SCRATCH/spike2.lua cli --no-auto-start list --format json
```

---

## Task 1 — sentinel-only startup on both start paths

### Path A: foreground `wezterm-mux-server`

Launch (epoch generated by `uuidgen`, i.e. the "service launcher" supplying env):

```sh
env -u WEZTERM_PANE -u WEZTERM_UNIX_SOCKET -u TMUX -u TMUX_PANE \
  DMUX_SERVER_EPOCH=2aa0bf63-6f41-4166-8a96-058afce57ccd DMUX_SPIKE_PHASE=1 \
  /opt/homebrew/bin/wezterm-mux-server --config-file $SCRATCH/spike2.lua   # PID 35494
```

Full `cli list --format json` (`$SCRATCH/listA-1.json`) — exactly ONE pane, no `default` workspace row, no `UNMANAGED-DEFAULT-SHELL`:

```json
[
  {
    "window_id": 0,
    "tab_id": 0,
    "pane_id": 0,
    "workspace": "dmux:system:2aa0bf63-6f41-4166-8a96-058afce57ccd",
    "size": { "rows": 24, "cols": 80, "pixel_width": 640, "pixel_height": 384, "dpi": 0 },
    "title": "bash",
    "cwd": "file:///Users/fredrir/",
    "cursor_x": 0, "cursor_y": 0,
    "cursor_shape": "Default", "cursor_visibility": "Visible",
    "left_col": 0, "top_row": 0,
    "tab_title": "", "window_title": "",
    "is_active": true, "is_zoomed": false,
    "tty_name": "/dev/ttys010"
  }
]
```

(The `"title": "bash"` is WezTerm's initial guess; the pane actually runs the `/bin/sh` sentinel loop.)

Runtime descriptor written by the handler (`$SCRATCH/runtime-descriptor.json`):

```json
{"state":"ready","epoch":"2aa0bf63-6f41-4166-8a96-058afce57ccd","pid":35494,"socket":"/tmp/dmux-s2/sock","written_at":"2026-08-16T11:36:40Z","sentinel_window_id":0,"sentinel_tab_id":0,"sentinel_pane_id":0}
```

`pid` matches the actual server PID 35494 → `wezterm.procinfo.pid()` works inside mux-startup. `DMUX_SERVER_EPOCH` from the launcher environment reached the Lua handler → env propagation proven. The handler also transitions the descriptor `starting` → `ready` (both writes observed; final state `ready`).

### Path B: `wezterm-mux-server --daemonize`

```sh
env -u WEZTERM_PANE -u WEZTERM_UNIX_SOCKET -u TMUX -u TMUX_PANE \
  DMUX_SERVER_EPOCH=d3265082-001c-4818-836c-bcfd16dfea7f DMUX_SPIKE_PHASE=1 \
  /opt/homebrew/bin/wezterm-mux-server --daemonize --config-file $SCRATCH/spike2.lua
# exit=0; daemon re-execs itself as: wezterm-mux-server --pid-file-fd 3 --config-file ... (PID 35731)
# /tmp/dmux-s2/daemon.pid contains 35731 (daemon_options.pid_file honored)
```

`cli list` (`$SCRATCH/listB-1.json`): again exactly ONE pane —

```text
panes: 1  [(pane_id 0, 'dmux:system:d3265082-001c-4818-836c-bcfd16dfea7f', title 'bash')]
```

Descriptor updated to the new pid/epoch:

```json
{"state":"ready","epoch":"d3265082-001c-4818-836c-bcfd16dfea7f","pid":35731,"socket":"/tmp/dmux-s2/sock","written_at":"2026-08-16T11:37:23Z","sentinel_window_id":0,"sentinel_tab_id":0,"sentinel_pane_id":0}
```

Notes:
- Env vars survive daemonization (the daemon re-exec keeps the environment).
- `config.daemon_options = { pid_file, stdout, stderr }` fully redirects daemon artifacts away from `~/.local/share/wezterm` — required so a dmux-owned daemonized server can never collide with the live GUI's runtime dir.
- Path B rebound over the stale socket file left by killed server A without complaint (see Task 5 for the dark side of that).

**Conclusion T1: on both start paths the mux-startup-created sentinel is the only pane; WezTerm spawned no unmanaged default shell.** (The mechanism: the default-program spawn is suppressed when mux-startup leaves the mux non-empty.)

## Task 2 — exactly once per start, fresh epoch per restart

Complete raw `$SCRATCH/mux-startup.log` after all four spike2 server starts (A foreground, B daemonized, C foreground phase-3, D duplicate-start test):

```text
2026-08-16T11:36:40Z [pid=35494] mux-startup BEGIN epoch=2aa0bf63-6f41-4166-8a96-058afce57ccd phase=1
2026-08-16T11:36:40Z [pid=35494] all_windows pre-spawn ok=true count=0
2026-08-16T11:36:40Z [pid=35494] sentinel spawned window_id=0 tab_id=0 pane_id=0 workspace=dmux:system:2aa0bf63-6f41-4166-8a96-058afce57ccd
2026-08-16T11:36:40Z [pid=35494] mux-startup END epoch=2aa0bf63-6f41-4166-8a96-058afce57ccd
2026-08-16T11:37:23Z [pid=35731] mux-startup BEGIN epoch=d3265082-001c-4818-836c-bcfd16dfea7f phase=1
2026-08-16T11:37:23Z [pid=35731] all_windows pre-spawn ok=true count=0
2026-08-16T11:37:23Z [pid=35731] sentinel spawned window_id=0 tab_id=0 pane_id=0 workspace=dmux:system:d3265082-001c-4818-836c-bcfd16dfea7f
2026-08-16T11:37:23Z [pid=35731] mux-startup END epoch=d3265082-001c-4818-836c-bcfd16dfea7f
2026-08-16T11:37:47Z [pid=35833] mux-startup BEGIN epoch=25f8ef7e-93db-4bc1-8ea0-a8de0f24e192 phase=3
2026-08-16T11:37:47Z [pid=35833] all_windows pre-spawn ok=true count=0
2026-08-16T11:37:47Z [pid=35833] sentinel spawned window_id=0 tab_id=0 pane_id=0 workspace=dmux:system:25f8ef7e-93db-4bc1-8ea0-a8de0f24e192
2026-08-16T11:37:47Z [pid=35833] restore-shape spawned ws=dmux:u:alpha window_id=1 pane_id=1 cwd=/tmp
2026-08-16T11:37:47Z [pid=35833] restore-shape spawned ws=dmux:u:beta window_id=2 pane_id=2 cwd=/
2026-08-16T11:37:47Z [pid=35833] restore-shape spawned ws=dmux:u:gamma window_id=3 pane_id=3 cwd=/Users/fredrir
2026-08-16T11:37:47Z [pid=35833] post-spawn window id=2 ws=dmux:u:beta
2026-08-16T11:37:47Z [pid=35833] post-spawn window id=1 ws=dmux:u:alpha
2026-08-16T11:37:47Z [pid=35833] post-spawn window id=3 ws=dmux:u:gamma
2026-08-16T11:37:47Z [pid=35833] post-spawn window id=0 ws=dmux:system:25f8ef7e-93db-4bc1-8ea0-a8de0f24e192
2026-08-16T11:37:47Z [pid=35833] all_windows post-spawn count=4
2026-08-16T11:37:47Z [pid=35833] recursive-cli probe START
2026-08-16T11:37:52Z [pid=35833] recursive-cli probe END elapsed=5s output=[...probe.sh: line 13: 35842 Killed: 9  env WEZTERM_UNIX_SOCKET=/tmp/dmux-s2/sock /opt/homebrew/bin/wezterm --config-file "$CFG" cli --no-auto-start list --format json > /tmp/dmux-s2/probe-out.json 2> /tmp/dmux-s2/probe-err.txt
...probe.sh: line 17: 35843 Terminated: 15  ( sleep 5; kill -9 "$CPID" ... )
probe rc=137 elapsed=5s]
2026-08-16T11:37:52Z [pid=35833] mux-startup END epoch=25f8ef7e-93db-4bc1-8ea0-a8de0f24e192
2026-08-16T11:38:31Z [pid=36955] mux-startup BEGIN epoch=6961cda6-09b8-4835-a228-84755a566069 phase=1
2026-08-16T11:38:31Z [pid=36955] all_windows pre-spawn ok=true count=0
2026-08-16T11:38:31Z [pid=36955] sentinel spawned window_id=0 tab_id=0 pane_id=0 workspace=dmux:system:6961cda6-09b8-4835-a228-84755a566069
2026-08-16T11:38:31Z [pid=36955] mux-startup END epoch=6961cda6-09b8-4835-a228-84755a566069
```

- Exactly one `BEGIN` per server PID (4 starts → 4 BEGIN lines; `grep -c 'mux-startup BEGIN'` after start A = 1, `grep -c 'BEGIN epoch=25f8ef7e'` = 1, `grep -c 'probe START'` = 1).
- Repeated `cli list` 3s after start A returned the identical single pane (`$SCRATCH/listA-2.json`) — no later default spawn, handler not re-fired.
- Kill (SIGTERM, exit 143) + restart re-ran the handler with the freshly supplied `DMUX_SERVER_EPOCH` each time; each sentinel workspace and each descriptor carries its own epoch.

## Task 3 — in-process mux work vs recursive CLI (server C, PID 35833, phase=3)

- **3a PASS:** `wezterm.mux.all_windows()` inside mux-startup returned ok, `count=0` pre-spawn and `count=4` post-spawn with correct per-window workspaces (log above). Note the returned order is arbitrary (2,1,3,0) — never rely on ordinals.
- **3b PASS:** three windows spawned sequentially in-process with distinct workspaces (`dmux:u:alpha|beta|gamma`), distinct cwds (`/tmp`, `/`, `$HOME`) and distinct argv (`/bin/sh` x2, `/bin/zsh`), all within the same log second, before any client ever attached. Post-restart `cli list` (`$SCRATCH/listC-1.json`) confirmed pane cwds:

```text
panes: 4
2 dmux:u:beta   file:///           sleep
1 dmux:u:alpha  file:///private/tmp/ sleep
3 dmux:u:gamma  file:///Users/fredrir/ sleep
0 dmux:system:25f8ef7e-93db-4bc1-8ea0-a8de0f24e192 file:///Users/fredrir/ bash
```

- **3c FAIL-AS-EXPECTED (the point):** the recursive `wezterm cli --no-auto-start list` against the server's OWN socket, launched via `io.popen` from inside mux-startup, produced **no output and no error for the full 5 seconds** until the watchdog SIGKILLed it: `probe rc=137 elapsed=5s`, `/tmp/dmux-s2/probe-out.json` empty (0 bytes), `/tmp/dmux-s2/probe-err.txt` empty (0 bytes). The client connects (socket is already bound) but the server cannot service the RPC because its own startup path is blocked inside the very handler that is waiting on the client → deadlock until an external timeout. Meanwhile `mux-startup END` was delayed exactly those 5 s (11:37:47 → 11:37:52), i.e. **the whole server's readiness is held hostage by anything synchronous in the handler**. The server was fully healthy afterwards (list above worked).

**Conclusion T3: in-process `wezterm.mux.*` inspection and creation during mux-startup is fast and reliable; any synchronous `wezterm cli` self-call is a guaranteed deadlock-until-timeout. The plan's in-process-only recovery rule (§15.3 step 3, "never recursively query the starting mux through `wezterm cli`") is confirmed, and any child the handler must spawn needs an external watchdog bound.**

## Task 4 — auto-start disablement matrix

Config under test had `no_serve_automatically = true` on the unix domain. Server STOPPED for all probes; every probe kept `WEZTERM_UNIX_SOCKET=/tmp/dmux-s2/sock` so any accident stayed scratch-directed. `pgrep`/`ps` before/after each probe.

### Probe 0 (safety-relevant resolution check, `--no-auto-start`, env var UNSET)

```sh
env -u WEZTERM_PANE -u WEZTERM_UNIX_SOCKET -u TMUX -u TMUX_PANE \
  /opt/homebrew/bin/wezterm --config-file $SCRATCH/spike2.lua cli --no-auto-start list
# rc=0 — and it printed the LIVE GUI's panes (window 6, workspace "default", ~/.codex panes)
```

**Finding: `--config-file` does NOT scope `wezterm cli` socket resolution.** With `WEZTERM_UNIX_SOCKET` unset, the CLI ignored the scratch config's `unix_domains[].socket_path` and connected to the default runtime-dir socket (the live GUI). Read-only, no harm — but for dmux this means the env var (exact endpoint) is mandatory on every call; a config file alone is not an isolation mechanism.

### Probe A: `cli list` WITHOUT `--no-auto-start` (env var set, socket dead/absent)

```text
WARN  wezterm_client::client > While connecting to Socket("/tmp/dmux-s2/sock"): connecting to /tmp/dmux-s2/sock.  Will try spawning the server.
WARN  wezterm_client::client > Running: "/opt/homebrew/bin/wezterm-mux-server" "--daemonize"
ERROR wezterm > (after spawning server) failed to connect to Socket("/tmp/dmux-s2/sock"): ... No such file or directory; terminating
rc=1
```

After: a stray `wezterm-mux-server --pid-file-fd 3` (PID 40683, started at the probe's exact second) was alive, **running the user's DEFAULT config** (the spawn line above shows `--config-file` is NOT propagated) and **bound to the user's default socket** `~/.local/share/wezterm/sock` (lsof-verified; socket file birth time = probe time). The CLI itself still failed (it wanted `/tmp/dmux-s2/sock`), so the net effect of one un-flagged call was: error to the caller PLUS an orphan default-config server squatting the default socket. Stray killed immediately; the socket file it created was removed (its pre-existing `pid` file, birth Aug 15, was left in place; the live GUI's `gui-sock-9640` untouched throughout).

### Control: `cli --no-auto-start list` (env var set, socket dead/absent)

```text
ERROR wezterm > failed to connect to Socket("/tmp/dmux-s2/sock"): connecting to /tmp/dmux-s2/sock; terminating
rc=1  — no process spawned (ps before/after identical)
```

### Probe B: `cli --prefer-mux list` WITHOUT `--no-auto-start`

Byte-for-byte the same behavior as Probe A: `Will try spawning the server` → `Running: "wezterm-mux-server" "--daemonize"` (again no `--config-file`) → connect failure rc=1 → stray PID 41305 bound to `~/.local/share/wezterm/sock`. `--prefer-mux` only changes endpoint *preference*; it adds no restraint. Stray killed, its socket file removed.

(A GUI-based connect probe — `wezterm connect` — was deliberately NOT run: it necessarily opens a GUI and, per the resolution finding above, GUI-side serving semantics of `no_serve_automatically` could not be tested without risking the user's live session. Its documented/intended semantics — GUI won't auto-serve the domain — remain an assumption for the launchd/systemd design, to be re-verified in the GUI-presentation spike.)

### Knob matrix

| Knob | Prevents `wezterm cli` from birthing a server? | Notes |
|---|---|---|
| `--no-auto-start` on the CLI call | **YES** — clean rc=1 error, zero processes spawned | The only knob that works for CLI paths |
| `no_serve_automatically = true` (unix domain, in the loaded config) | **NO** | CLI still ran `wezterm-mux-server --daemonize`; the knob governs GUI-side serving, not CLI auto-start |
| Both | YES (via `--no-auto-start`) | Belt-and-suspenders as the plan specifies |
| `WEZTERM_UNIX_SOCKET` env | Not an auto-start knob, but **mandatory for endpoint scoping** | Without it, `--config-file` alone routes `wezterm cli` to the DEFAULT socket (the live GUI!) |

**Bonus hazard proven:** when auto-start fires, the spawned server gets NO `--config-file` — it materializes with the user's default config on the default socket. So a single dmux call missing `--no-auto-start` doesn't just start "a" server, it starts the WRONG server in the wrong place while the call still fails. The plan's "every dmux CLI call uses `--no-auto-start`" (§15.1) is not defense-in-depth; it is load-bearing.

## Task 5 — duplicate server start on the same config/socket

While server C (PID 35833, 4 panes, epoch `25f8ef7e…`) was serving `/tmp/dmux-s2/sock`, a second foreground `wezterm-mux-server --config-file spike2.lua` was started with fresh epoch `6961cda6…` (PID 36955).

Observed:

- **No bind failure, no error output, both processes kept running.** dup-stderr/dup-stdout: empty.
- The duplicate ran its own full `mux-startup` (log lines for PID 36955 above), spawned its own sentinel, and **overwrote the runtime descriptor** with `pid:36955, epoch:6961cda6…`.
- lsof showed **both** processes holding a socket named `/tmp/dmux-s2/sock` — two distinct socket inodes; the filesystem path now resolved to the newcomer's:

```text
wezterm-m 35833 fredrir 3u unix 0x292c46c9e71a1afa /tmp/dmux-s2/sock   (old, orphaned binding)
wezterm-m 36955 fredrir 3u unix 0x3a3e1e06236bb980 /tmp/dmux-s2/sock   (new, owns the path)
```

- `cli list` on the path returned **1 pane, epoch `6961cda6…`** — the 4-pane server C had been silently partitioned away (`$SCRATCH/list-after-dup.json`).
- After killing the thief (36955): the path pointed at a dead socket inode; `cli --no-auto-start list` → `failed to connect ... terminating`, while orphan 35833 was still alive holding its unlinked socket. **Killing the duplicate does not heal the original; the original's panes are unreachable garbage until it is killed and a fresh start rebinds.**

Related observation (task 1/2): a SIGTERM'd `wezterm-mux-server` never unlinks its socket file — every restart begins by rebinding over a stale path. Same unlink-and-rebind logic is what makes the steal silent.

**Conclusion T5: WezTerm provides zero mutual exclusion on the unix socket path. Concurrent starts silently partition the mux. A single serializing owner (launchd/systemd) plus descriptor{pid,epoch,socket inode} + sentinel-epoch handshake (which DOES detect the swap: list shows the wrong `dmux:system:<epoch>`) are both necessary, exactly as §15.1 prescribes.** The descriptor should additionally record the bound socket's device/inode so a swapped path is detectable even before listing.

## Task 6 — sentinel filtering/handshake viability

In every capture the sentinel row appears in normal `cli list --format json` output with:

- `"workspace": "dmux:system:<epoch>"` — full epoch string, exact-match filterable;
- stable `window_id`/`tab_id`/`pane_id` also published in the descriptor for cross-checking.

Client-side exclusion is a trivial workspace-prefix filter, and the epoch handshake (compare list workspace suffix against descriptor/served epoch) works with no extra RPC. Confirmed viable; the duplicate-start test doubles as the negative case (mismatched epoch exposed the replaced server).

## Risks / unknowns carried forward

1. `no_serve_automatically`'s GUI-side semantics were not exercised (would require touching the live GUI); verify in the GUI/presentation spike before enabling `default_gui_startup_args = {'connect','unix'}`.
2. The suppression mechanism is "mux-startup left the mux non-empty". If a future WezTerm changes the default-spawn condition, the sentinel invariant breaks; keep the `default_prog` canary marker in the dmux config so a regression is observable in list output.
3. mux-startup runs before readiness: anything slow/synchronous in the handler delays serving (observed 5 s hold). Cold recovery doing large restores in-process will delay `ready` — acceptable per plan (service waits on descriptor), but timeouts must budget for it.
4. Stale socket files are never unlinked on shutdown; the service must treat "socket file exists" as meaningless without a connect + epoch handshake.
5. `wezterm.procinfo.pid()` worked on this fork/version; keep the pcall guard.
6. Duplicate-start behavior means even a *read-only* client that auto-starts (missing `--no-auto-start`) can, with the right config resolution, become a partitioning writer. Argv construction must be centralized in dmux (§13) so the flag can never be dropped.

## Cleanup ledger

Spawned and killed (nothing else touched): 35494 (server A), 35731 (server B), 35833 (server C), 36955 (task-5 duplicate), 40683 (probe-A stray, default config — killed immediately), 41305 (probe-B stray, default config — killed immediately). Sentinel/`sleep` children died with their servers (post-sweep found none). `/tmp/dmux-s2/sock` gone; `~/.local/share/wezterm` restored (stray-created `sock` file removed after lsof-confirmed unbound; pre-existing `pid` file left; live GUI PID 9640 and `gui-sock-9640` untouched). Sibling spike servers (dmux-s1/s3/s4/s5, spike3/spike4) never touched. Full ledger: `$SCRATCH/pids.txt`.

## Artifacts

- Config: `$SCRATCH/spike2.lua`; probe wrapper: `$SCRATCH/probe.sh`
- Raw handler log: `$SCRATCH/mux-startup.log`
- Descriptor (final state from last start): `$SCRATCH/runtime-descriptor.json`
- List captures: `$SCRATCH/listA-1.json`, `listA-2.json`, `listB-1.json`, `listC-1.json`, `list-after-dup.json`
- Server stderr: `$SCRATCH/serverA-stderr.log`, `serverC-stderr.log` (empty = clean), daemon logs via `daemon_options`
- Task-4 probe transcripts: `/tmp/dmux-s2/probeA-out.txt`, `probeA-err.txt` (and inline above)

`$SCRATCH` = `/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike2`
