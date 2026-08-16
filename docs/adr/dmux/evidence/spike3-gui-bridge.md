# Spike 3 — P0 GUI presentation mechanisms (plan §13.2, §14, §15.1)

Date: 2026-08-16. Host: macOS (Darwin 25.5.0). wezterm 20260813-114614-18a44cb7
(fredrir fork) at /opt/homebrew/bin/wezterm + wezterm-mux-server.

Safety envelope: the live user GUI (`/Applications/WezTerm.app`, PID 9640) and its
sockets under `~/.local/share/wezterm` were never touched. All scratch state lived in
`/tmp/dmux-s3/` (sockets; short paths, macOS `sun_path` < 104 bytes) and the session
scratchpad `.../scratchpad/spike3/`. Every GUI launch used `--config-file <scratch cfg>`,
a scratch `WEZTERM_UNIX_SOCKET`, a distinct `--class dmux-spike3*` (prevents delegation
to the live GUI instance), and `--always-new-process` where the subcommand supports it.
Every `wezterm cli` call used
`env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=<exact-sock> wezterm --config-file <cfg> cli --no-auto-start ...`.
Only PIDs spawned by this spike were killed. Other concurrent spike agents' servers
(/tmp/dmux-s1, /tmp/dmux-s5, scratchpad/spike4) were observed and left alone.

## 0. Setup

Two scratch mux servers:

- server #1 "dmux" on `/tmp/dmux-s3/mux.sock` (PID 41056), seeded with
  sentinel-style workspace `dmux:system:testepoch` (1 pane: `sh -c 'echo SENTINEL $$; exec sleep 86400'`)
  and user workspace `dmux:testhost:space1` (1 window, 2 panes via `cli split-pane`,
  running `sleep 86401` / `sleep 86402`).
- server #2 "dmux2" on `/tmp/dmux-s3/mux2.sock` (PID 41460), sentinel-only.

Servers started as:

```
nohup env -u WEZTERM_PANE -u TMUX -u TMUX_PANE /opt/homebrew/bin/wezterm-mux-server \
  --config-file $SP/mux.lua > $SP/mux1-server.log 2>&1 &
```

Owner-side cli wrapper (`wcli.sh`, `wcli2.sh` for server #2):

```sh
exec env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=/tmp/dmux-s3/mux.sock \
  /opt/homebrew/bin/wezterm --config-file "$SP/mux.lua" cli --no-auto-start "$@"
```

### Incidental finding (feeds §15.1): mux server spawns a default program at startup

Immediately after `wezterm-mux-server` start, before any client attached, `cli list`
showed pane 0 in workspace `default`; `cli get-text --pane-id 0` printed
`MUX1-DEFAULT-PROG-SPAWNED 41060` (our marker `default_prog`). Same on server #2.
So a fresh mux server DOES create one unmanaged default window/pane on its own.
This confirms the plan's requirement that dmux `mux-startup` must suppress the default
program on every server-start path. Both default panes were killed before baselining.

Baseline owner state (server #1), `cli list --format json` reduced to
(window,tab,pane,workspace,tty):

```
2 2 2 dmux:testhost:space1  /dev/ttys015
2 2 3 dmux:testhost:space1  /dev/ttys016
1 1 1 dmux:system:testepoch /dev/ttys007
```

Pane shell PIDs: 41093 (`sleep 86400`), 41095 (`sleep 86401`), 41097 (`sleep 86402`).

## 1. Task 1 — attach mechanisms (owner-side before/after diffs)

Diff method: `cli list --format json` before and after; key =
`(window_id, tab_id, pane_id, workspace)`; sets compared.

| # | Invocation | Domain state | Owner diff | Notes |
| - | ---------- | ------------ | ---------- | ----- |
| 1a | `wezterm --config-file gui.lua start --class dmux-spike3 --always-new-process --domain dmux --attach` | nonempty (sentinel + 2-pane Space) | added: [] removed: [] → **DIFF-ZERO** | GUI window appeared (expected). GUI-side `list_ws`: workspaces `[dmux:system:testepoch, dmux:testhost:space1]`, active `dmux:system:testepoch`; no GUI-local workspace/pane created either |
| 1b | same, `--domain dmux2` | sentinel-only | **DIFF-ZERO** | server #1 also verified untouched |
| 1c | `wezterm --config-file gui.lua connect --class dmux-spike3c dmux2` | sentinel-only | **DIFF-ZERO** | plain `connect` is also non-creating when the domain has ≥1 pane |
| 1d | `start ... --domain dmux2` **without** `--attach` | sentinel-only | added: `(2,2,2,'default')` → **CREATES** | spawn-as-attach hazard confirmed. New pane ran the **server-side** `default_prog` (`MUX2-DEFAULT-PROG-SPAWNED`) |
| 1e | `connect dmux2` | **truly empty** (sentinel killed) | added pane 3 in `default` → **CREATES** | |
| 1f | `start ... --domain dmux2 --attach` | **truly empty** | added pane 4 in `default` → **CREATES** | matches `--attach` help text: non-spawn only "if the domain already has running panes" |

Conclusions:

- Selected attach path (§13.2 mechanism 1): this fork's
  `wezterm start --domain <d> --attach` attaches an existing domain with **zero**
  owner-side pane/tab/window/workspace creation — **provided the domain has at least
  one pane**. The plan's always-present `dmux:system:<epoch>` sentinel is exactly the
  guarantee that keeps this path no-create; 1f proves the sentinel is load-bearing,
  not decorative.
- Plain `connect` behaves identically on nonempty domains but also spawns on empty
  ones; neither invocation is intrinsically no-create. Production must continue to
  treat "domain proven nonempty (sentinel handshake)" as a precondition, per plan.
- Implicit spawns resolve the **mux server's** `default_prog`, not the GUI's
  (evidence: 1d/1e/1f panes printed the MUX2 marker, and the Task 3 leak printed the
  MUX1 marker, never the GUI config's marker).

## 2. Task 2 — attach with the server stopped (`no_serve_automatically = true`)

Server #2 killed; three invocations attempted against its dead socket:

| Case | Socket file | Invocation | Result |
| ---- | ----------- | ---------- | ------ |
| 2a | stale file present | `start --domain dmux2 --attach` | GUI logs `ERROR wezterm_gui > failed to connect to Socket("/tmp/dmux-s3/mux2.sock"): connecting to /tmp/dmux-s3/mux2.sock; terminating` and **exits**; `pgrep wezterm-mux-server` before/after identical → **no auto-serve, no server spawned** |
| 2b | file removed | same | identical fail-closed behavior |
| 2c | file removed | `connect dmux2` | identical fail-closed behavior |

With `no_serve_automatically = true` in the client-side unix domain config, both attach
paths fail closed with a clear error and never birth a server. (An error window flashes
briefly; the process terminates on its own.)

## 3. Task 3 — activate-existing semantics inside the GUI

Driven through the file-spool bridge (see §4) running inside the attached scratch GUI.

### 3a. `SwitchToWorkspace { name = <missing> }` CREATES (the hazard)

Bridge action `switch_action_raw` → `win:perform_action(act.SwitchToWorkspace{name='dmux:testhost:doesnotexist'}, pane)`.

Owner diff: added `(3,3,4,'dmux:testhost:doesnotexist')` — a brand-new owner
workspace + window + pane (pane 4, running the server default_prog marker
`MUX1-DEFAULT-PROG-SPAWNED`), and the GUI's active workspace became the new name.
Hazard confirmed exactly as §13.2 states: with `default_domain` pointing at the mux
domain, the created pane lands on the **owner server**, violating con-never-creates.

### 3b. `wezterm.mux.set_active_workspace(<missing>)` ERRORS, creates nothing

Bridge action `set_active_raw` with target `dmux:testhost:neverexists` returned:

```
ok=false
error='"dmux:testhost:neverexists" is not an existing workspace' (Lua error, caught by pcall)
workspaces_after=["dmux:system:testepoch","dmux:testhost:space1"]   <- unchanged
```

Owner list unchanged. `set_active_workspace` is fail-closed: it can only activate an
existing workspace.

### 3c. Check-then-switch composites under an adversarial create/remove loop

Adversary (shell): tight loop on owner cli — spawn a pane into workspace
`dmux:testhost:raceX` (recording its pane id), sleep 30 ms, kill it, then scan
`cli list` for any raceX pane it did not create (= a leaked GUI create-on-miss),
recording and killing leaks. 60 cycles per run (~20 s).

GUI probe: self-rescheduling `wezterm.time.call_after(0.005, tick)` loop, 2500
iterations per run; each tick does membership check via
`wezterm.mux.get_workspace_names()` then the variant's switch. Each tick's work is
pcall-guarded.

Variant A — check then `mux.set_active_workspace` (same callback):

```
iters=2500 miss=1999 hit_ok=501 hit_err=0 errors={} leaks=0
```

Variant B — check then `perform_action(SwitchToWorkspace)` (same callback):

```
iters=2500 miss=1985 hit_ok=502 hit_err=13 leaks=0
hit_err breakdown: "there is no active pane!?" x8,
                   "error converting Lua nil to userdata" (active_pane nil) x5
```

Zero create-on-miss slipped through in either same-callback composite across 5000
iterations against ~120 create/remove cycles. Reason (confirmed by the errors seen and
by 3d below): Lua callbacks run to completion on the mux model's owning thread, and
`get_workspace_names`, `set_active_workspace`, and `perform_action` all consult the
GUI process's local mux model synchronously — the model cannot change between a check
and a switch issued in the same callback. The 13 caught errors in variant B were
window/pane-death artifacts of switching into a workspace being torn down
(`perform_action` raising "there is no active pane!?"), not creations; an earlier
unhardened run showed such an error silently kills the `call_after` chain, so
production Lua must pcall every callback body.

### 3d. Deterministic race proof: split the composite across callbacks and it CREATES

Bridge action `split_switch`: membership check in callback 1, then
`call_after(0.8, ...)` performs `SwitchToWorkspace` in callback 2. Between the two,
the shell killed the target workspace's only pane. Result file:

```json
{"member_at_check":true,"member_at_switch":false,"switch_ok":true,
 "workspaces_after":["dmux:system:testepoch","dmux:testhost:space1","dmux:testhost:victim"]}
```

Owner gained pane 186 in `dmux:testhost:victim` — a silent create-on-miss, no error
anywhere. The GUI's model had already processed the removal (`member_at_switch=false`),
yet `SwitchToWorkspace` recreated the workspace and spawned into the owner.

### Verdict on §13.2 mechanism 2/3

An in-GUI compare-and-activate is **sound and sufficient; no fork primitive is
required**, with these hard rules:

1. The activate primitive must be `wezterm.mux.set_active_workspace` wrapped in pcall,
   preceded by a membership check for a clean typed `not_found` (the check is UX; the
   pcall'ed `set_active_workspace` is the actual safety, since it fails closed even if
   the check is stale or elided).
2. `SwitchToWorkspace` (and anything spawn-capable) is banned from the activation
   path. Same-callback usage happened to be safe empirically, but its miss branch
   creates owner resources (3a, 3d), so any future refactor that separates check from
   switch — another callback, an event, a queued action — silently violates
   con-never-creates. Fail-open is the wrong default; `set_active_workspace` is
   fail-closed by construction.
3. Every bridge callback body must be pcall-wrapped: an uncaught Lua error kills the
   `call_after` chain silently (observed; nothing in the GUI stderr).

Residual (accepted) staleness: the GUI model mirrors the owner asynchronously, so
`activate` can succeed against a workspace the owner removed milliseconds ago; the
result is a transient view of a dying workspace, never a creation. dmux's owner-side
verification covers this.

## 4. Task 4 — acknowledged bridge round trip (file spool)

Transport: request file `<scratch>/spike3/bridge/req-<uid>.json`, written atomically
(tmp + rename) by the client; the GUI poller (`wezterm.time.call_after` every 50 ms)
consumes it (unlink), executes, and atomically writes `ack-<uid>.json`. Replay of a
consumed uid produces `ack-<uid>.replay.json`, leaving the original ack intact.
Consumed-uid set held in Lua process memory.

### Frozen P0 request schema (proven shape)

```json
{ "uid": "<unique id>", "action": "activate|toast|detach|attach|ping|list_ws",
  "target": "<opaque workspace key | domain name | toast text>",
  "nonce": "<hex>", "expiry": <unix seconds> }
```

### Frozen P0 ack schema (proven shape)

```json
{ "uid": "<echoed>", "action": "<echoed>", "nonce": "<echoed>",
  "ok": true|false, "error": "not_found|expired|replayed|malformed_request|...",
  "completed_at": <unix seconds>,
  "workspace": "<resulting active workspace, on success>",
  "window_ids": [<GUI-side mux window ids of that workspace>] }
```

(`window_ids` are GUI-process mux ids, NOT owner ids — consistent with §13.2's rule
that owner tab/pane ids are never applied to the GUI; the production ack adds the
GUI instance/domain refs the plan requires.)

### Proofs (verbatim acks)

Successful activate:

```json
{"latency_ms": 31.4, "ack": {"action": "activate", "completed_at": 1786880903,
 "nonce": "0259a1818a1a2718", "ok": true, "uid": "t4-activate-ok",
 "window_ids": [1], "workspace": "dmux:testhost:space1"}}
```

Not-found (no creation — owner diff after this ack: **DIFF-ZERO**):

```json
{"latency_ms": 49.5, "ack": {"action": "activate", "completed_at": 1786880911,
 "error": "not_found", "nonce": "819a70c635baa63a", "ok": false, "uid": "t4-activate-missing"}}
```

Replay rejection (same uid re-sent; original `ack-t4-activate-ok.json` verified
byte-identical afterwards):

```json
{"latency_ms": 49.5, "ack": {"completed_at": 1786880911, "error": "replayed",
 "ok": false, "uid": "t4-activate-ok"}}
```

Expiry rejection (`expiry` 5 s in the past):

```json
{"latency_ms": 36.2, "ack": {"action": "activate", "completed_at": 1786880911,
 "error": "expired", "nonce": "5073babe1de6274d", "ok": false, "uid": "t4-expired"}}
```

Toast (`window:toast_notification('dmux', msg, nil, 4000)`):

```json
{"latency_ms": 25.2, "ack": {"action": "toast", "completed_at": 1786880920,
 "nonce": "0040308a2d49f7ef", "ok": true, "uid": "t4-toast"}}
```

(API-level success; visual delivery is subject to macOS notification permissions for
the scratch app instance and was not separately verified.)

### Latency, 10 real activate round trips (50 ms poll / 5 ms client ack poll)

```
[12.6, 30.6, 31.4, 31.4, 31.3, 31.4, 37.0, 29.7, 30.7, 29.9] ms
min 12.6 / median 31.3 / max 37.0
```

Request→ack ≈ one poll interval. A production TTL of 1–2 s is comfortably above the
observed ceiling by >25x.

## 5. Task 5 — detach safety

Before: owner list `[2,3 in dmux:testhost:space1; 1 in dmux:system:testepoch]`,
`ps -p 41093,41095,41097` all alive (`sleep 86400/86401/86402`).

- Bridge `perform_action(act.DetachDomain{DomainName='dmux'})` → ack ok. Owner diff:
  **DIFF-ZERO**; all three shell PIDs alive; GUI process survived with zero windows
  (`quit_when_all_windows_are_closed=false`) and the bridge poller kept answering pings.
- **Zero-window finding:** `perform_action(act.AttachDomain 'dmux')` requires a GUI
  window and fails (`no_gui_window`) in the zero-window state. The window-independent
  pair works from any state: `wezterm.mux.get_domain('dmux'):detach()` /
  `:attach()` and `:state()` (`"Attached"/"Detached"`).
  `detach_mux`: state Attached→Detached, gui_windows 0, owner DIFF-ZERO.
  `attach_mux` from zero windows: state Detached→Attached, gui_windows 1 (a GUI window
  materialized mirroring existing content), owner **DIFF-ZERO**, pane ids still
  `[1,2,3]`, shell PIDs alive. This is the correct primitive for §13.4's
  Hammerspoon/zero-window path.
- Hard kill: `kill -9 <gui-pid>` → owner list unchanged, PIDs 41093/41095/41097 alive,
  `cli get-text --pane-id 2` still returned `SPACE1A 41095`. (Also independently
  demonstrated mid-spike by an earlier plain `kill` of the first GUI instance.)
- Re-attach after GUI death: a fresh `start --domain dmux --attach` (task 5 GUI
  restarts) re-presented the same owner panes with the same pane ids each time.

## 6. Task 6 — cleanup

- All `dmux-spike3*` GUI processes verified gone (`pgrep`): none.
- Scratch mux servers 41056/41460 killed; `pgrep -f spike3/mux`: none; seeded
  `sleep 8640x` shells: none.
- `/tmp/dmux-s3/*.sock` removed; directory empty.
- Our 13 stale `gui-sock-<pid>` files (all belonging to dead spike GUIs launched by
  this session) removed from `~/.local/share/wezterm`. Remaining entries are the
  pre-existing ones only (`gui-sock-9640` live GUI, `gui-sock-1789`,
  `default-org.wezfurlong.wezterm -> gui-sock-9640`). Live GUI PID 9640 confirmed
  running and untouched. Note: the passed scratch `WEZTERM_UNIX_SOCKET=/tmp/dmux-s3/guiN.sock`
  paths were never created by the GUI — the GUI always serves
  `~/.local/share/wezterm/gui-sock-<pid>`; the env var only redirects client
  discovery (which is why setting it, plus a distinct `--class`, isolates scratch
  GUIs from the live instance).

## 7. Risks / unknowns

1. **Empty-domain edge:** every attach path spawns if the domain has zero panes (1e,
   1f). The sentinel invariant plus the "wait for ready + sentinel handshake before
   GUI start" rule in §15.1 must be enforced on every launch path; there is no
   attach flag that is unconditionally no-create.
2. **Silent Lua callback death:** an uncaught error in a `call_after` callback ends
   the chain with no log. The production bridge needs pcall-everything plus a
   watchdog (e.g. bridge heartbeat file the CLI can check before trusting a timeout).
3. **Two bridge consumers:** each GUI process evaluating the config starts its own
   poller; two scratch GUIs would race on one spool dir. Production must key the
   spool/socket per GUI instance (plan already requires origin GUI instance identity
   in the token) and/or lock the spool.
4. **Mirror staleness:** `activate` can ack ok for a workspace the owner removed
   moments earlier (benign, no creation) — owner-side verification remains
   authoritative, per plan.
5. **Toast visual delivery** unverified (macOS notification permission for the
   scratch instance); API returns success.
6. **Fork surface not needed for P0**, but if the bridge ever needs atomicity
   against its own model *across* callbacks, `set_active_workspace`'s fail-closed
   error is the only guard; that was deemed sufficient.
7. `wezterm-mux-server` default-program spawn at startup (see §0) must be suppressed
   by `mux-startup` (§15.1); unhandled, every service start creates an unmanaged
   owner pane in workspace `default`.

## 8. Scratch file inventory (session scratchpad, `.../scratchpad/spike3/`)

`mux.lua`, `mux2.lua`, `gui.lua` (below), `wcli.sh`, `wcli2.sh`, `breq.py`,
`adversary.sh`, raw before/after JSON snapshots (`t1a-*`, `t1b-*`, `t1c-*`, `t1d-*`,
`t3a-*`, `t3d-*`, `t4-*`, `t5-*`), `bridge/` (acks, `bridge-log.txt`,
`race-result-*.json`, `split-result-*.json`), server logs, GUI logs (`gui*.log`),
`pids.txt`.

## 9. Configs verbatim

### mux.lua (scratch mux server #1)

```lua
-- spike3 scratch mux server #1 (nonempty domain): serves /tmp/dmux-s3/mux.sock
local config = {}
config.unix_domains = {
  { name = 'dmux', socket_path = '/tmp/dmux-s3/mux.sock', no_serve_automatically = true },
}
-- marker default_prog: if anything ever spawns a default program server-side we can identify it
config.default_prog = { '/bin/sh', '-c', 'echo MUX1-DEFAULT-PROG-SPAWNED $$; exec sleep 86510' }
return config
```

### mux2.lua (scratch mux server #2, sentinel-only)

```lua
-- spike3 scratch mux server #2 (sentinel-only domain): serves /tmp/dmux-s3/mux2.sock
local config = {}
config.unix_domains = {
  { name = 'dmux2', socket_path = '/tmp/dmux-s3/mux2.sock', no_serve_automatically = true },
}
config.default_prog = { '/bin/sh', '-c', 'echo MUX2-DEFAULT-PROG-SPAWNED $$; exec sleep 86520' }
return config
```

### gui.lua (scratch GUI config including the complete bridge Lua, final hardened version)

```lua
-- spike3 scratch GUI config: client domains for both scratch mux servers + file-spool bridge
local wezterm = require 'wezterm'
local act = wezterm.action
local mux = wezterm.mux

local config = {}
if wezterm.config_builder then config = wezterm.config_builder() end

config.unix_domains = {
  { name = 'dmux',  socket_path = '/tmp/dmux-s3/mux.sock',  no_serve_automatically = true },
  { name = 'dmux2', socket_path = '/tmp/dmux-s3/mux2.sock', no_serve_automatically = true },
}
config.default_domain = 'dmux'
config.automatically_reload_config = false
config.window_close_confirmation = 'NeverPrompt'
-- keep the gui process (and the bridge poller) alive when DetachDomain closes all windows
config.quit_when_all_windows_are_closed = false
-- marker default_prog: any GUI-driven implicit spawn (the SwitchToWorkspace create hazard)
-- lands in default_domain 'dmux' running this identifiable command
config.default_prog = { '/bin/sh', '-c', 'echo GUI-LEAKED-SPAWN $$; exec sleep 86530' }
config.initial_cols = 60
config.initial_rows = 12

---------------------------------------------------------------------------
-- minimal file-spool bridge
-- request : <BRIDGE>/req-<uid>.json  {uid, action, target, nonce, expiry}
-- ack     : <BRIDGE>/ack-<uid>.json  {uid, action, nonce, ok, error?, completed_at, ...}
-- replayed uid -> ack-<uid>.replay.json {ok=false, error='replayed'}
---------------------------------------------------------------------------
local BRIDGE = '/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike3/bridge'

local function read_file(p)
  local f = io.open(p, 'rb')
  if not f then return nil end
  local d = f:read('*a'); f:close(); return d
end

local function write_file(p, d)
  local tmp = p .. '.tmp'
  local f = io.open(tmp, 'wb')
  if not f then return end
  f:write(d); f:close()
  os.rename(tmp, p)
end

local function log(msg)
  local f = io.open(BRIDGE .. '/bridge-log.txt', 'ab')
  if f then f:write(os.date('!%FT%TZ') .. ' ' .. msg .. '\n'); f:close() end
end

local function gui_win_pane()
  local ok, wins = pcall(function() return wezterm.gui.gui_windows() end)
  if not ok or #wins == 0 then return nil, nil end
  local w = wins[1]
  return w, w:active_pane()
end

local function workspace_windows(name)
  local ids = {}
  for _, w in ipairs(mux.all_windows()) do
    if w:get_workspace() == name then table.insert(ids, w:window_id()) end
  end
  return ids
end

local function is_member(name)
  for _, n in ipairs(mux.get_workspace_names()) do
    if n == name then return true end
  end
  return false
end

-- race probe state ---------------------------------------------------------
local RACE_STATS = nil

local function start_race_probe(req)
  RACE_STATS = {
    variant = req.variant or 'setactive',
    target = req.target,
    max = req.iterations or 500,
    iters = 0, miss = 0, hit_ok = 0, hit_err = 0,
    errors = {}, done = false, uid = req.uid,
  }
  local function tick()
    local s = RACE_STATS
    if s.iters >= s.max then
      s.done = true
      write_file(BRIDGE .. '/race-result-' .. s.uid .. '.json', wezterm.json_encode(s))
      log('RACE done variant=' .. s.variant .. ' iters=' .. s.iters ..
          ' miss=' .. s.miss .. ' hit_ok=' .. s.hit_ok .. ' hit_err=' .. s.hit_err)
      return
    end
    s.iters = s.iters + 1
    local tok, terr = pcall(function()
      if not is_member(s.target) then
        s.miss = s.miss + 1
      else
        if s.variant == 'setactive' then
          -- composite under test: membership check (above) then mux.set_active_workspace
          local ok, err = pcall(mux.set_active_workspace, s.target)
          if ok then
            s.hit_ok = s.hit_ok + 1
          else
            s.hit_err = s.hit_err + 1
            err = tostring(err)
            s.errors[err] = (s.errors[err] or 0) + 1
          end
        else
          -- composite under test: membership check (above) then SwitchToWorkspace action
          local w, p = gui_win_pane()
          if w then
            local ok, err = pcall(function()
              w:perform_action(act.SwitchToWorkspace { name = s.target }, p)
            end)
            if ok then
              s.hit_ok = s.hit_ok + 1
            else
              s.hit_err = s.hit_err + 1
              err = tostring(err):sub(1, 120)
              s.errors[err] = (s.errors[err] or 0) + 1
            end
          end
        end
      end
    end)
    if not tok then
      s.tick_errors = (s.tick_errors or 0) + 1
      log('RACE tick error: ' .. tostring(terr):sub(1, 200))
    end
    wezterm.time.call_after(0.005, tick)
  end
  tick()
end

-- request dispatch ----------------------------------------------------------
local consumed = {}

local function handle(req)
  local resp = { uid = req.uid, action = req.action, nonce = req.nonce, ok = false }
  if req.action == 'ping' then
    resp.ok = true
  elseif req.action == 'list_ws' then
    resp.ok = true
    resp.workspaces = mux.get_workspace_names()
    resp.active = mux.get_active_workspace()
  elseif req.action == 'activate' then
    -- the candidate dmux primitive: compare-and-activate with strict no-create
    if not is_member(req.target) then
      resp.error = 'not_found'
    else
      local ok, err = pcall(mux.set_active_workspace, req.target)
      if ok then
        resp.ok = true
        resp.workspace = mux.get_active_workspace()
        resp.window_ids = workspace_windows(req.target)
      else
        resp.error = 'switch_failed: ' .. tostring(err)
      end
    end
  elseif req.action == 'set_active_raw' then
    -- probe: does mux.set_active_workspace create on miss, or error?
    local ok, err = pcall(mux.set_active_workspace, req.target)
    resp.ok = ok
    if not ok then resp.error = tostring(err) end
    resp.workspace = mux.get_active_workspace()
    resp.workspaces_after = mux.get_workspace_names()
  elseif req.action == 'switch_action_raw' then
    -- probe: SwitchToWorkspace create-on-miss hazard
    local w, p = gui_win_pane()
    if not w then
      resp.error = 'no_gui_window'
    else
      w:perform_action(act.SwitchToWorkspace { name = req.target }, p)
      resp.ok = true
      resp.note = 'perform_action dispatched (async)'
    end
  elseif req.action == 'toast' then
    local w = (gui_win_pane())
    if not w then
      resp.error = 'no_gui_window'
    else
      local ok, err = pcall(function()
        w:toast_notification('dmux', req.target or 'hello from spike3 bridge', nil, 4000)
      end)
      resp.ok = ok
      if not ok then resp.error = tostring(err) end
    end
  elseif req.action == 'detach' then
    local w, p = gui_win_pane()
    if not w then
      resp.error = 'no_gui_window'
    else
      w:perform_action(act.DetachDomain { DomainName = req.target }, p)
      resp.ok = true
    end
  elseif req.action == 'attach' then
    local w, p = gui_win_pane()
    if not w then
      resp.error = 'no_gui_window'
    else
      w:perform_action(act.AttachDomain(req.target), p)
      resp.ok = true
    end
  elseif req.action == 'attach_mux' then
    -- window-independent attach: MuxDomain:attach() (zero-window recovery path)
    local d = mux.get_domain(req.target)
    if not d then
      resp.error = 'no_such_domain'
    else
      resp.state_before = d:state()
      local ok, err = pcall(function() d:attach() end)
      resp.ok = ok
      if not ok then resp.error = tostring(err):sub(1, 200) end
      resp.state_after = d:state()
      resp.gui_windows = #wezterm.gui.gui_windows()
    end
  elseif req.action == 'detach_mux' then
    -- window-independent detach: MuxDomain:detach()
    local d = mux.get_domain(req.target)
    if not d then
      resp.error = 'no_such_domain'
    else
      resp.state_before = d:state()
      local ok, err = pcall(function() d:detach() end)
      resp.ok = ok
      if not ok then resp.error = tostring(err):sub(1, 200) end
      resp.state_after = d:state()
    end
  elseif req.action == 'domain_state' then
    local d = mux.get_domain(req.target)
    if not d then
      resp.error = 'no_such_domain'
    else
      resp.ok = true
      resp.state = d:state()
      resp.gui_windows = #wezterm.gui.gui_windows()
    end
  elseif req.action == 'split_switch' then
    -- deliberately split check and switch across callbacks to expose the race:
    -- membership check NOW, SwitchToWorkspace after `delay` seconds
    local member = is_member(req.target)
    local delay = req.delay or 0.3
    if not member then
      resp.error = 'not_found_at_check'
    else
      wezterm.time.call_after(delay, function()
        local w, p = gui_win_pane()
        local still = is_member(req.target)
        local ok, err = pcall(function()
          w:perform_action(act.SwitchToWorkspace { name = req.target }, p)
        end)
        write_file(BRIDGE .. '/split-result-' .. req.uid .. '.json', wezterm.json_encode({
          uid = req.uid, member_at_check = true, member_at_switch = still,
          switch_ok = ok, switch_err = ok and nil or tostring(err):sub(1, 200),
          workspaces_after = mux.get_workspace_names(),
        }))
      end)
      resp.ok = true
      resp.note = 'checked now, switching in ' .. tostring(delay) .. 's'
    end
  elseif req.action == 'race_probe' then
    start_race_probe(req)
    resp.ok = true
    resp.note = 'probe started'
  elseif req.action == 'race_stats' then
    resp.ok = true
    resp.stats = RACE_STATS
  else
    resp.error = 'unknown_action'
  end
  return resp
end

local function poll()
  local ok, err = pcall(function()
    local entries = wezterm.read_dir(BRIDGE)
    table.sort(entries)
    for _, path in ipairs(entries) do
      local base = path:match('([^/]+)$')
      local uid = base and base:match('^req%-(.+)%.json$')
      if uid then
        local raw = read_file(path)
        os.remove(path)
        local pok, req = pcall(wezterm.json_parse, raw)
        if not pok or type(req) ~= 'table' then
          write_file(BRIDGE .. '/ack-' .. uid .. '.malformed.json',
            wezterm.json_encode({ uid = uid, ok = false, error = 'malformed_request', completed_at = os.time() }))
          log('MALFORMED uid=' .. uid)
        elseif consumed[uid] then
          write_file(BRIDGE .. '/ack-' .. uid .. '.replay.json',
            wezterm.json_encode({ uid = uid, ok = false, error = 'replayed', completed_at = os.time() }))
          log('REPLAY rejected uid=' .. uid)
        else
          consumed[uid] = true
          local resp
          if req.expiry and os.time() > req.expiry then
            resp = { uid = req.uid, action = req.action, nonce = req.nonce, ok = false, error = 'expired' }
          else
            local hok, hres = pcall(handle, req)
            resp = hok and hres or { uid = req.uid, ok = false, error = 'handler_error: ' .. tostring(hres) }
          end
          resp.completed_at = os.time()
          write_file(BRIDGE .. '/ack-' .. uid .. '.json', wezterm.json_encode(resp))
          log('ACK uid=' .. uid .. ' action=' .. tostring(req.action) ..
              ' ok=' .. tostring(resp.ok) .. (resp.error and (' err=' .. tostring(resp.error)) or ''))
        end
      end
    end
  end)
  if not ok then log('POLL ERROR: ' .. tostring(err)) end
  wezterm.time.call_after(0.05, poll)
end

local function ensure_poller()
  if wezterm.GLOBAL.spike3_poller then return end
  wezterm.GLOBAL.spike3_poller = true
  local pid = '?'
  pcall(function() pid = tostring(wezterm.procinfo.pid()) end)
  log('poller started pid=' .. pid)
  poll()
end

wezterm.on('gui-startup', ensure_poller)
wezterm.on('gui-attached', ensure_poller)

return config
```

### breq.py (bridge client: atomic request write, ack wait, latency)

```python
#!/usr/bin/env python3
"""Write a bridge request atomically, wait for its ack, print ack + latency ms.

usage: breq.py UID ACTION [TARGET] [--expiry-delta N] [--variant V] [--iterations N]
               [--timeout SECS] [--no-wait] [--raw-uid-file-only]
The ack watched for is ack-<UID>.json unless --suffix is given (e.g. .replay).
"""
import json, os, sys, time

SP = "/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike3"
BRIDGE = os.path.join(SP, "bridge")

def main():
    args = sys.argv[1:]
    uid = args.pop(0)
    action = args.pop(0)
    target = None
    if args and not args[0].startswith("--"):
        target = args.pop(0)
    expiry_delta = 30
    variant = None
    iterations = None
    req_delay = None
    timeout = 15.0
    suffix = ""
    wait = True
    while args:
        a = args.pop(0)
        if a == "--expiry-delta": expiry_delta = int(args.pop(0))
        elif a == "--variant": variant = args.pop(0)
        elif a == "--iterations": iterations = int(args.pop(0))
        elif a == "--delay": req_delay = float(args.pop(0))
        elif a == "--timeout": timeout = float(args.pop(0))
        elif a == "--suffix": suffix = args.pop(0)
        elif a == "--no-wait": wait = False
        else: raise SystemExit(f"unknown arg {a}")

    req = {
        "uid": uid,
        "action": action,
        "nonce": os.urandom(8).hex(),
        "expiry": int(time.time()) + expiry_delta,
    }
    if target is not None: req["target"] = target
    if variant: req["variant"] = variant
    if iterations: req["iterations"] = iterations
    if req_delay is not None: req["delay"] = req_delay

    tmp = os.path.join(BRIDGE, f".tmp-req-{uid}.json")
    final = os.path.join(BRIDGE, f"req-{uid}.json")
    with open(tmp, "w") as f:
        json.dump(req, f)
    t0 = time.monotonic()
    os.rename(tmp, final)

    if not wait:
        print(json.dumps({"sent": req}))
        return

    ack_path = os.path.join(BRIDGE, f"ack-{uid}{suffix}.json")
    while time.monotonic() - t0 < timeout:
        if os.path.exists(ack_path):
            lat_ms = (time.monotonic() - t0) * 1000.0
            with open(ack_path) as f:
                ack = json.load(f)
            print(json.dumps({"latency_ms": round(lat_ms, 1), "ack": ack}))
            return
        time.sleep(0.005)
    print(json.dumps({"error": "TIMEOUT waiting for ack", "sent": req}))
    sys.exit(2)

if __name__ == "__main__":
    main()
```

### adversary.sh (workspace create/remove loop + leak detector)

```sh
#!/bin/sh
# Tight create/remove loop for workspace $1 on mux server #1.
# Tracks its own pane ids; any pane found in the workspace that it did not
# create is a LEAK (GUI create-on-miss slipped through); it records and kills it.
SP="/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike3"
WS="$1"; CYCLES="$2"; TAG="$3"
OWN="$SP/adv-$TAG-own-panes.txt"; LEAK="$SP/adv-$TAG-leaks.txt"
: > "$OWN"; : > "$LEAK"
i=0
while [ "$i" -lt "$CYCLES" ]; do
  i=$((i+1))
  P=$("$SP/wcli.sh" spawn --new-window --workspace "$WS" -- /bin/sh -c 'exec sleep 30' 2>/dev/null)
  [ -n "$P" ] && echo "$P" >> "$OWN"
  # brief existence window
  sleep 0.03
  [ -n "$P" ] && "$SP/wcli.sh" kill-pane --pane-id "$P" >/dev/null 2>&1
  # leak scan: any pane in $WS that is not ours
  "$SP/wcli.sh" list --format json 2>/dev/null | python3 -c "
import json,sys
ws='$WS'
own=set(l.strip() for l in open('$OWN') if l.strip())
try: d=json.load(sys.stdin)
except Exception: d=[]
for p in d:
    if p['workspace']==ws and str(p['pane_id']) not in own:
        print(p['pane_id'], p['title'])
" | while read -r pid title; do
      echo "cycle=$i leaked_pane=$pid title=$title" >> "$LEAK"
      "$SP/wcli.sh" kill-pane --pane-id "$pid" >/dev/null 2>&1
    done
done
echo "cycles=$i done" >> "$LEAK"
```
