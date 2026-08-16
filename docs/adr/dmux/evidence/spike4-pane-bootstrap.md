# Spike 4 — Pane bootstrap feasibility (plan §11.1 bootstrap / §11.2)

Date: 2026-08-16. Host: macOS (Darwin 25.5.0).
wezterm 20260813-114614-18a44cb7 (fredrir fork), `/opt/homebrew/bin/wezterm{,-mux-server}`. tmux 3.7b.
Scratch: `/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike4/` (`$SP` below).
Wez scratch socket: `/tmp/dmux-s4/sock`. tmux server namespace: `tmux -L dmux-spike4`. Both killed at the end.

Verdict: **the provisional pane-bootstrap mechanism works on both providers.** Spawn-return IDs are exact,
reserved-title correlation is exact and unique, the blocking-FIFO handshake execs the real command in-place
(same pane ID, same PID), timeout is visible and never runs user code, orphans are found/killed/conflict-detected
by title scan alone.

---

## 0. Helper script (verbatim, stand-in for the Rust helper)

`$SP/helper.sh`:

```bash
#!/bin/bash
# dmux spike4 bootstrap helper — stand-in for the future Rust bootstrap helper.
# argv: helper.sh <request-uid> [timeout-seconds]
# Behavior (mirrors plan §11.1 bootstrap paragraphs):
#   1. immediately claim reserved title  dmux-bootstrap:<uid>   (OSC 2)
#   2. record inherited $WEZTERM_PANE / $TMUX_PANE to a per-uid+pid file
#   3. block on per-uid FIFO with bounded timeout (no user code runs yet)
#   4. on payload: emit OSC 1337 SetUserVar marker + final title, write ack, exec payload
#   5. on timeout: write visible timeout marker file, exit 41 (never exec user code)

SP="/private/tmp/claude-501/-Users-fredrir-dotfiles/1eb1d38d-6cca-4e36-9cc0-7c6fee189484/scratchpad/spike4"
uid="$1"
tmo="${2:-30}"

# (1) reserved provisional title, before anything else
printf '\033]2;dmux-bootstrap:%s\007' "$uid"

# (2) record inherited native pane identity from environment
envfile="$SP/env.$uid.$$"
{
  echo "uid=$uid"; echo "pid=$$"
  echo "WEZTERM_PANE=${WEZTERM_PANE-<unset>}"
  echo "TMUX_PANE=${TMUX_PANE-<unset>}"
  echo "TMUX=${TMUX-<unset>}"
  echo "start_epoch=$(date +%s)"
} > "$envfile"

# (3) blocking handshake read on per-uid FIFO, bounded.
# Open read-write so open(2) itself cannot block forever; timeout applies to read.
fifo="$SP/fifo.$uid"
[ -p "$fifo" ] || mkfifo "$fifo"
exec 3<>"$fifo"
payload=""
if read -t "$tmo" -u 3 payload; then
  # (4) success: final marker user-var (OSC 1337 SetUserVar, base64 value) + final title
  b64=$(printf 'done:%s' "$uid" | base64)
  printf '\033]1337;SetUserVar=dmux_bootstrap=%s\007' "$b64"
  printf '\033]2;dmux-run:%s\007' "$uid"
  { echo "uid=$uid"; echo "pid=$$"; echo "payload=$payload"; echo "ack_epoch=$(date +%s)"; } > "$SP/ack.$uid"
  exec 3>&-
  exec $payload
  echo "exec-failed" > "$SP/execfail.$uid"; exit 42
else
  # (5) timeout: visible marker, nonzero exit, user code never ran
  { echo "uid=$uid"; echo "pid=$$"; echo "timeout_after=$tmo"; echo "timeout_epoch=$(date +%s)"; } > "$SP/timeout.$uid.$$"
  printf '\033]2;dmux-bootstrap-timeout:%s\007' "$uid"
  exit 41
fi
```

Key mechanics that made it work:

- **FIFO must be opened read-write (`exec 3<>fifo`)**, then `read -t N -u 3`. A plain `read -t N < fifo`
  blocks forever in `open(2)` before the timeout even starts. This is a real trap the Rust helper must
  also avoid (open O_RDWR or O_NONBLOCK+poll).
- bash 3.2 (`/bin/bash` on macOS) suffices: `read -t`, `read -u`, `exec` all present.

## 1. Scratch server bootstrap — hazards found (important for the P5 service manager)

1. `wezterm-mux-server --daemonize` **locks a shared pid file** `~/.local/share/wezterm/pid` regardless of
   socket: with a live GUI/other server holding it, daemonize fails:
   `ERROR wezterm_mux_server > unable to lock pid file /Users/fredrir/.local/share/wezterm/pid: Resource temporarily unavailable (os error 35)`.
   Foreground + external supervision (launchd) avoids the shared lock.
2. **`WEZTERM_UNIX_SOCKET` does NOT control where the server listens** — only where the CLI connects.
   Started with only the env var set, the server bound the DEFAULT `~/.local/share/wezterm/sock`
   (observed via `lsof`; killed immediately). The listen path must be forced in config:

   ```lua
   return { unix_domains = { { name = "unix", socket_path = "/tmp/dmux-s4/sock" } } }
   ```

   With that config the server bound exactly `/tmp/dmux-s4/sock` (lsof-verified).
3. On startup with no PROG, the mux server **auto-spawns one default shell pane**:
   `window_id 0 / tab_id 0 / pane_id 0, workspace "default", title "~"`. Any inventory/sentinel logic must
   expect this pre-existing pane (plan's `dmux:system:<epoch>` sentinel slot is consistent with this).

All wez CLI calls below are:

```
env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=/tmp/dmux-s4/sock \
  /opt/homebrew/bin/wezterm --config-file $SP/wez.lua cli --no-auto-start <verb> ...
```

## 2. Wez Task 1 — frozen spawn-return formats

`cli spawn --help` states: "Outputs the pane-id for the newly created pane on success". No JSON/format flag exists.

| operation | exact command | exact stdout (hexdump) |
|---|---|---|
| Space create | `spawn --new-window --workspace spike4-ws -- $SP/helper.sh uid-A1 120` | `31 0a` = `1\n` |
| Split create | `split-pane --pane-id 1 -- $SP/helper.sh uid-A3 120` | `32 0a` = `2\n` |
| Group create | `spawn --window-id 1 -- $SP/helper.sh uid-A2 120` | `33 0a` = `3\n` |

**Frozen format: a single decimal pane-id followed by `\n`. Nothing else.** No window/tab id on stdout;
those must come from the same-epoch `list --format json` correlation (which is what the plan prescribes).
stderr empty, exit 0 on success; usage errors exit 2 with a clap error on stderr and create no pane.

`list --format json` per-pane fields observed (this build): `window_id, tab_id, pane_id, workspace, size{...},
title, cwd, cursor_*, left_col, top_row, tab_title, window_title, is_active, is_zoomed, tty_name`.
No user-vars and no foreground-process field in this build's list output.

Structure confirmation from the list after all three spawns:

```
{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"default","title":"~"}
{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"spike4-ws","title":"dmux-bootstrap:uid-A1"}
{"window_id":1,"tab_id":1,"pane_id":2,"workspace":"spike4-ws","title":"dmux-bootstrap:uid-A3"}   <- split: same tab as pane 1
{"window_id":1,"tab_id":2,"pane_id":3,"workspace":"spike4-ws","title":"dmux-bootstrap:uid-A2"}   <- window-id spawn: new tab, same window
```

## 3. Wez Task 2 — three-way correlation (spawn return / title scan / inherited env)

Helper env recordings (`$SP/env.<uid>.<pid>`) vs spawn stdout vs `list` title scan:

```
uid-A1: spawn_return=1 title_scan=1(count=1) inherited_env=WEZTERM_PANE=1
uid-A2: spawn_return=3 title_scan=3(count=1) inherited_env=WEZTERM_PANE=3
uid-A3: spawn_return=2 title_scan=2(count=1) inherited_env=WEZTERM_PANE=2
```

Exact three-way agreement on all three creation paths. The reserved title `dmux-bootstrap:<uid>` appears on
exactly one pane per uid. `WEZTERM_PANE` is inherited by the helper (TMUX vars unset), so the helper-side
cross-check required by §11.1 is available.

## 4. Wez Task 3 — handshake completion

Coordinator wrote `/bin/sleep 4321` into `$SP/fifo.uid-A1`. One second later:

- ack file `$SP/ack.uid-A1`: `uid=uid-A1 / pid=35789 / payload=/bin/sleep 4321 / ack_epoch=...`
- `ps -p 35789`: `PID 35789 PPID 35624(mux-server) COMMAND /bin/sleep 4321` — **exec in place, same PID,
  still a direct child of the mux server.**
- `list` shows the SAME pane_id 1, title now `dmux-run:uid-A1` (all other panes unchanged):
  `{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"spike4-ws","title":"dmux-run:uid-A1"}`

OSC 1337 SetUserVar emission: the escape was written on the same channel as the OSC 2 title (which
demonstrably landed in the mux server's model). This build's `cli list` does not expose user vars, so
server-side receipt was not directly observable from the CLI; user-var-changed is a GUI-side event.
Emission mechanics proven; CLI-side readback is NOT available and should not be designed for.

## 5. Wez Task 4 — crash-orphan matrix

uid-B1: spawned into fresh workspace `spike4-ws-orphan` with an 8s handshake timeout, FIFO never fed
(simulated coordinator death). spawn_return=4.

| step | result |
|---|---|
| (a) title scan while blocked | exactly 1 match: `{"pane_id":4,"workspace":"spike4-ws-orphan","title":"dmux-bootstrap:uid-B1"}` (other bootstrap titles distinct) |
| (b) timeout visibility | marker file `timeout.uid-B1.39631` written (`timeout_after=8`), helper pid gone (exit 41) |
| (b) pane linger? | **NO — pane closes on process exit** (default config): pane 4 absent from `list` right after timeout; its single-pane workspace `spike4-ws-orphan` disappeared with it |
| (c) kill of live orphan (uid-B2, 120s timeout, pane 5) | `cli kill-pane --pane-id 5` exit 0; re-list shows 0 matches for pane 5 |
| ambiguity (uid-DUP spawned twice, panes 6 and 7) | title scan multiplicity=2: both panes matched `dmux-bootstrap:uid-DUP` → conflict path taken, no kill issued |

Consequence for recovery design (wez): a timed-out orphan self-removes at the pane level (default
`exit_behavior`), so takeover may find *zero* panes for a journaled request — the durable timeout marker /
journal must carry the evidence, and "zero found after confirmed absence → safe retry" is the correct
path exactly as the plan states. A *blocked* (not yet timed-out) orphan is killable exactly by returned id.

## 6. Wez Task 5 — lost spawn-return

uid-C1 spawned into `spike4-ws-lost` with stdout discarded.

```
before pane-id set: [0,1,2,3,6,7]
after  pane-id set: [0,1,2,3,6,7,8]
tree diff (new panes): [8]
title scan: [{"pane_id":8,"window_id":6,"tab_id":7,"workspace":"spike4-ws-lost","title":"dmux-bootstrap:uid-C1"}]  count=1
helper env: WEZTERM_PANE=8
```

Title-scan-only correlation identifies the pane uniquely; before/after tree diff independently agrees.

## 7. tmux Task 6 — exact-ID returns and title landing

All commands via `env -u TMUX -u TMUX_PANE -u WEZTERM_PANE tmux -L dmux-spike4 ...`.
Server pid 40754 (recorded in pids.txt). NOTE: server auto-loaded the user's tmux.conf (no `-f` given),
which is realistic for dmux-managed servers on this machine.

| operation | exact command | exact stdout |
|---|---|---|
| Space create | `new-session -d -P -F '#{session_id}\|#{window_id}\|#{pane_id}' -s spike4A $SP/helper.sh uid-T1 120` | `$0\|@0\|%0` |
| Group create | `new-window -t spike4A -P -F '#{session_id}\|#{window_id}\|#{pane_id}' $SP/helper.sh uid-T2 120` | `$0\|@1\|%1` |
| Split create | `split-window -P -F '#{session_id}\|#{window_id}\|#{pane_id}' -t %0 $SP/helper.sh uid-T3 120` | `$0\|@0\|%2` |

**Frozen format: whatever `-F` asks for — `$<n>|@<n>|%<n>` with `-P -F '#{session_id}|#{window_id}|#{pane_id}'`.
All three IDs are returned atomically at creation; strictly richer than wez.**

Live confirmation of the plan's "window ID, not window index" rule: `split-window -t spike4A:0` failed with
`can't find window: 0` because the user config sets `base-index 1`. Targeting the immutable pane id `%0`
worked. Index-based targeting is demonstrably config-fragile.

Title landing (OSC 2 → `pane_title`):

```
list-panes -a -F '#{pane_id}|#{pane_title}|#{pane_current_command}'
%0|dmux-bootstrap:uid-T1|bash
%2|dmux-bootstrap:uid-T3|bash
%1|dmux-bootstrap:uid-T2|bash
```

Relevant option state on this server (user config + defaults):

```
allow-rename off        # affects WINDOW NAME via escape; irrelevant to pane_title
allow-set-title on      # THIS is the gate for OSC 0/2 -> pane_title (tmux >= 3.3); default on — required
set-titles on           # outer-terminal title; irrelevant
automatic-rename on     # window name; irrelevant
remain-on-exit off      # pane closes when process exits
```

Requirement to record: **`allow-set-title on`** (default) is what lets the helper's OSC 2 land in
`pane_title`. `allow-rename` may stay off. If a user turns `allow-set-title` off, title correlation breaks —
dmux should assert/force this option (pane-scoped `set -p allow-set-title on` is possible at spawn time)
or rely on the returned exact IDs, which tmux always provides.

## 8. tmux Task 7 — correlation, handshake, orphans

Three-way correlation:

```
uid-T1: spawn_return=%0 title_scan=%0(count=1) inherited_env=TMUX_PANE=%0
uid-T2: spawn_return=%1 title_scan=%1(count=1) inherited_env=TMUX_PANE=%1
uid-T3: spawn_return=%2 title_scan=%2(count=1) inherited_env=TMUX_PANE=%2
```

Handshake (fed `/bin/sleep 4322` into `fifo.uid-T1`):

```
ack.uid-T1: uid=uid-T1 pid=40757 payload=/bin/sleep 4322
ps -p 40757: /bin/sleep 4322          # exec in place, same PID
%0|dmux-run:uid-T1|sleep              # same pane id, new title, pane_current_command flipped bash->sleep
```

Crash-orphan matrix (tmux):

| step | result |
|---|---|
| (a) title scan while blocked (uid-T4, 8s, `$0\|@2\|%3`) | exactly 1 match `%3\|dmux-bootstrap:uid-T4` |
| (b) timeout visibility | marker `timeout.uid-T4.41025` written (`timeout_after=8`) |
| (b) pane linger? | **NO — `remain-on-exit off` (default): pane %3 and its window @2 gone after exit 41** |
| (c) kill of live orphan (uid-T5, `%4`, 120s) | `kill-pane -t %4` exit 0; re-list count 0 |
| ambiguity (uid-TDUP twice → `%5`,`%6`) | scan multiplicity=2 → conflict path, no kill |
| lost-return (uid-T6, `-P` omitted) | before/after `list-panes -a` diff = `%7`; title scan count=1 → `%7`; helper env `TMUX_PANE=%7` |

## 9. tmux Task 8 — marker stamping and rename survival

```
set-option -t spike4A @dmux_space_uid 9f3a1c2e-spike4-4b6d-8e21-77aa00dmux01   # exit 0
show-options -t spike4A -v @dmux_space_uid        -> 9f3a1c2e-spike4-4b6d-8e21-77aa00dmux01
rename-session -t spike4A spike4A-RENAMED         # external rename, exit 0
list-sessions -> $0|spike4A-RENAMED               # session_id stable across rename
show-options -t spike4A-RENAMED -v @dmux_space_uid -> (same uuid)
show-options -t '$0' -v @dmux_space_uid            -> (same uuid)   # readable by immutable session id
set-option -g @dmux_server_epoch epoch-spike4-1; show-options -g -v @dmux_server_epoch -> epoch-spike4-1
```

Session user options survive external rename and are addressable by immutable `$n` session id; global
(server-scope) user options work for the `@dmux_server_epoch` scheme in §11.2.

## 10. Frozen results summary

Spawn-return formats (freeze these):

- **wez** (`cli spawn --new-window|--window-id`, `cli split-pane`): stdout = `<pane_id>\n`, decimal, nothing
  else; exit 0. Window/tab ids only via follow-up `list --format json`. No format flag exists.
- **tmux** (`new-session|new-window|split-window -P -F '#{session_id}|#{window_id}|#{pane_id}'`):
  stdout = `$<n>|@<n>|%<n>\n`. All three ids atomic at creation.

Proven handshake shape: pre-create per-uid FIFO → spawn `helper <uid>` → helper sets reserved OSC-2 title,
records inherited `WEZTERM_PANE`/`TMUX_PANE`, opens FIFO O_RDWR, bounded read → coordinator correlates
(return id = title scan = helper env) → writes payload → helper emits SetUserVar + final title, writes ack,
`exec`s payload in place (pane id and PID preserved on both providers) → timeout path writes marker,
exits 41, pane self-closes on both providers under default config.

Orphan-recovery matrix (identical shape on both providers):

| case | wez | tmux |
|---|---|---|
| blocked orphan found by title | yes, count=1 | yes, count=1 |
| timeout visible | marker file + exit 41 | marker file + exit 41 |
| pane after exit | closes (no linger) | closes (`remain-on-exit off`) |
| kill proven orphan | `cli kill-pane --pane-id N` → absent | `kill-pane -t %N` → absent |
| duplicate reserved title | detected, multiplicity=2, no kill | detected, multiplicity=2, no kill |
| lost spawn return | title scan + tree diff unique | title scan + tree diff unique |

## 11. Risks / unknowns

1. **Wez server provisioning is the sharp edge, not pane bootstrap**: `--daemonize` contends a shared
   `~/.local/share/wezterm/pid` lock, and the listen socket comes only from `unix_domains` config
   (`WEZTERM_UNIX_SOCKET` env is ignored by the server). The P5 user-service manager must generate a config
   with an explicit `socket_path` and run the server foreground under launchd.
2. Wez `list` gives no foreground-process or user-var fields; correlation must rest on title + ids (as planned).
3. Pane self-close on helper timeout means a takeover can legitimately find zero orphans for a journaled
   request; the plan's "retry only after confirmed absence" rule is load-bearing.
4. tmux title correlation depends on `allow-set-title on` (default). tmux's `-P -F` exact-ID return makes
   title-scan strictly a fallback there.
5. FIFO open semantics: helper must open the handshake channel O_RDWR (or nonblocking) or the bounded
   timeout is void. Applies equally to the Rust implementation.
6. Helper title emission raced nothing in practice (title visible in list within ~1s on both providers),
   but a scan immediately after spawn-return could in principle see the pre-title pane; a bounded re-scan
   handles it. (Not observed in this spike; every 1s-later scan was complete.)
7. User config leaks into managed tmux servers (`base-index 1` broke index targeting; `set-titles`,
   `automatic-rename` active). Managed servers may want `-f` with a controlled config or strict
   id-only targeting (the plan already mandates id-only).

## 12. Teardown

- wez scratch server pid 35624 (`/tmp/dmux-s4/sock`) killed; rogue default-socket server pid 35355 killed
  during setup (bound `~/.local/share/wezterm/sock` for ~60s; no user mux-server existed on that path — only
  GUI `gui-sock-*` sockets, which were untouched).
- `tmux -L dmux-spike4 kill-server` executed (server pid 40754).
- Raw command transcripts and JSON snapshots retained under `$SP/` (`out.spawn*`, `out.tmux*`, `list.*.json`,
  `env.*`, `ack.*`, `timeout.*`, `tmux.scan*`, `mux.log`, `pids.txt`).
