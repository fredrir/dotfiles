---
name: dmux-mux-lifecycle
description: The service-owned WezTerm mux server, its startup handler, sentinel and epoch
  handshake, managed unix domain, resurrection split, and guarded cold recovery —
  shared/wezterm/mux/dmux-mux.lua, wez/domains/init.lua, wez/plugins/resurrect.lua,
  linux/arch/wezterm-mux/**, and macos/launchd/com.fredrir.wezterm-mux.plist. Use when editing
  mux-startup, the sentinel, the runtime descriptor, the unix domain declaration, snapshot or
  restore logic, the systemd unit or launchd plist, or when a managed GUI will not start, the
  server was replaced, panes vanished after a restart, or recovery restored the wrong thing.
  Also fires on "the mux service won't start", "why is there a DMUX-CANARY pane", "restore my
  workspaces on boot", or "make wezterm reconnect after reboot". For the GUI-side signed bridge
  use dmux-bridge-actions; for general Lua style and tests use wezterm-lua-config.
compatibility: Needs a Lua 5.4-compatible interpreter for the test suite, plus systemctl (Linux)
  or launchctl (macOS) to manage the mux service. Restarting that service kills live panes.
metadata:
  version: "1"
---

# dmux mux lifecycle

Paths are relative to the repo root. Normative sources: `docs/dmux-wezterm-first-plan.md` §15 and
`docs/adr/dmux/002-service-sentinel-startup.md`.

The governing idea: the mux server holds live processes, so anything that stops it, empties it, or
lets a second server steal its socket is destructive. Normal GUI restart must be a pure attach that
preserves owner pane and process IDs; cold recovery is only for a freshly started, provably empty
server.

## The service is the only starter

WezTerm has no unix-socket mutual exclusion, so a manually launched `wezterm-mux-server` on the same
socket silently steals it and orphans the service's panes. systemd (`linux/arch/wezterm-mux/
wezterm-mux.service`) and launchd (`macos/launchd/com.fredrir.wezterm-mux.plist`) both run
`shared/wezterm/mux/dmux-mux-start.sh` and are the only legitimate starters.

Restart it through the service manager rather than by hand. **A restart kills every process in the
mux** — shells, editors, long-running jobs. Cold recovery reconstructs layout, cwd, and titles, but
never process IDs, so treat this as destructive and confirm before running it on a machine with
live work. Reattaching a GUI is not a restart and costs nothing.

```bash
systemctl --user restart wezterm-mux.service
```

```bash
launchctl kickstart -k gui/$UID/com.fredrir.wezterm-mux
```

`no_serve_automatically = true` on the unix domain is defense in depth only — it does not stop CLI
auto-start. The load-bearing invariant is that every dmux CLI call carries `--no-auto-start` plus an
explicit `WEZTERM_UNIX_SOCKET`.

`dmux-mux-start.sh` mints the epoch, boot nonce, and backend-instance UID, resolves the runtime
directory per-OS (macOS `getconf DARWIN_USER_TEMP_DIR`, Linux `$XDG_RUNTIME_DIR`), scrubs inherited
`WEZTERM_UNIX_SOCKET`/`WEZTERM_PANE`/`TMUX`/`TMUX_PANE`, and `exec`s in the foreground.
`--daemonize` is deliberately not used: it contends the shared default pid-file lock.

The GUI never guesses the socket path. `wez/domains/init.lua` reads a verified descriptor through
the maintained fork's `wezterm.gui.dmux_read_mux_descriptor` and shape-checks it against
`'^/.+/dmux/wez%-dmux%.sock$'`. If the descriptor is missing, `starting`, or invalid, config
evaluation aborts — `wezterm.lua` re-raises a `wez.domains` failure instead of swallowing it,
because swallowing would let WezTerm fall back to an unmanaged default shell.

## Rules for `mux-startup`

`mux-startup` in `mux/dmux-mux.lua` runs once per new server, before the default program. Inside it:

- **Never call `wezterm cli`.** It is a guaranteed deadlock against the server that is still
  starting. Use `wezterm.mux.*` in-process for every scan and mutation.
- **Never call `io.popen`, and never recurse.** Anything slow here delays serving.
- Never synchronously launch a child that reconnects to the same starting mux.

When a helper genuinely must run out of band, interpose the fixed system `env` binary to strip the
inherited identity — `wezterm.background_child_process` injects the current mux endpoint into its
child environment even though the service wrapper scrubbed it:

```lua
local clean = { '/usr/bin/env', '-u', 'WEZTERM_UNIX_SOCKET', '-u', 'WEZTERM_PANE',
                '-u', 'TMUX', '-u', 'TMUX_PANE' }
```

Recovery helpers are registry/file only and reject every inherited pane or mux identity.

## The sentinel and the canary

`mux-startup` creates exactly one reserved window in workspace `dmux:system:<epoch>` running
`dmux _mux-idle`. It is never a Space, Group, or Split, is excluded from user inventory, and
publishes the epoch through normal list fields.

Its load-bearing job is to keep an intentionally empty server non-empty, which is what suppresses
WezTerm's unmanaged default program. Backing that up, `config.default_prog` is a permanent tripwire:

```lua
config.default_prog = { '/bin/sh', '-c',
  'echo DMUX-CANARY-DEFAULT-PROG-MUST-NEVER-RUN; exec sleep 300' }
```

A pane running that marker under workspace `default` means default-shell suppression has broken.
Treat it as a regression, not as something to clean up quietly.

A missing, duplicate, or wrong-epoch sentinel makes the backend unavailable. If `DMUX_BIN` is
absent the sentinel falls back to a shell idle loop, and the descriptor is published `failed` rather
than `ready` — a fallback sentinel is explicitly not readiness.

GUI-side, `instance.lua` recognizes the reserved workspace syntactically (`^dmux:system:(.+)$` with
a UUID epoch), counts its panes separately, never marker-parses them, and refuses multiple system
workspaces or epochs. Lua reports the syntactic reservation; it never treats it as owner authority —
Rust accepts the exemption only after matching the exact owner descriptor and sentinel epoch.

## Recovery guards

- Recovery restores only into a new server with the valid sentinel and **zero user panes**. A
  nonempty server never restores.
- `mux-startup` inspects `wezterm.mux.all_windows()` in-process for this. P5 logs a nonempty
  pre-spawn count as an anomaly; the P10 coordinator owns acting on it.
- Removing the final active user Space records an `intentional_empty_revision`, and a later startup
  never restores a manifest at or before that revision.
- The compare-and-mutate step is a critical section: no sleep, yield, file IO, or callback boundary
  is permitted between reading the raw tree and the native create or remove. Both sites say so in
  comments — preserve that property when editing.
- A crash leaves the lease and journal observable; takeover resumes the same generation rather than
  starting a blind restore. Abort may remove only nodes proven to belong to that generation.

## The resurrection split

The fork has two modes and they must not blur.

GUI mode (`wez/plugins/resurrect.lua`): under `DMUX_WEZ_FIRST=1` it sets
`opts.startup_restore = false`, because the GUI is an attach-only client of a server that is already
populated. The manual restore picker is bound only when the flag is off, since restoring by spawning
native resources is a legacy-only path. With the flag off, `setup()` behaves exactly as it did
before dmux.

Owner mode (`mux/dmux-mux.lua`): reached only under `WEZ_FIRST`, through
`wezterm.plugin.require` on the fork, and requires `resurrect.dmux.prepare_restore`,
`execute_restore_node`, and `build_manifest`. A plugin checkout without that API is refused with
"resurrection fork has no dmux owner API" rather than silently degrading.

## Gotchas

- Managed configs pin `automatically_reload_config = false`. Editing Lua does not hot-reload a
  running managed server or GUI; restart the service.
- `default_gui_startup_args = { 'connect', 'dmux' }` makes attach the normal startup path. Before
  enabling it, the fork's unconditional `gui-startup` restore must be off, or the GUI will restore
  into an already-populated server.
- Blocking `wezterm.sleep_ms` spins exist only in `mux/dmux-mux.lua` and are deliberate —
  `mux-startup` must finish its critical section without yielding. Do not copy that pattern into
  GUI-side code, which uses `wezterm.time.call_after` instead.
- `wezterm.json_encode` cannot distinguish an empty sequence from an empty object. The recovery
  manifest requires `spaces` to stay a JSON array, so the code patches `"spaces":{}` after encoding
  and fails if the replacement count is not exactly 1.
- The flag reaches the systemd unit from the user manager's own environment block, which `~/.config/environment.d/50-dmux.conf` populates (the unit's `PassEnvironment=` lines are inert for a user manager — see the unit's comments); the flag is read at config evaluation and
  a service that does not inherit it comes up unmanaged.
- The launchd plist deliberately sets no `StandardOutPath`/`StandardErrorPath` — asking launchd to
  open predictable `/tmp` log paths is a symlink hazard.
- `dmux _mux-idle` is what the sentinel runs. If you rename that subcommand, the sentinel silently
  degrades to the fallback and the backend stops being ready.

- macOS purges regular files in the per-user temporary directory (the fixed managed-mux
  runtime) after three untouched days — the descriptor included. The maintained fork fixes that
  directory, so the fix is not a path change here: `com.fredrir.dmux-runtime-keepalive` runs
  `dmux _runtime-keepalive` twice a day to refresh the descriptor, lease and bridge files. A
  `ready` descriptor older than a day means the agent is not loaded. A bare Spotlight/Dock start
  of WezTerm under the flag is refused by the fork's broker gate; `~/Applications/dmux.app`
  (`macos/applications`, linked file by file so the bundle is a real directory) and Hammerspoon's CMD+§ both go through `dmux _gui summon`.

## Validating a change

```bash
sh shared/wezterm/wez/dmux_bridge/tests/suite.sh
```

The `domains`, `top_level`, `top_level_missing_descriptor`, and `top_level_missing_key` cases cover
the descriptor and abort paths. They stub the descriptor reader rather than touching a real server,
so a green suite does not prove the service works. `mux_startup_witness` covers the other
startup refusal: a native descriptor publisher whose service witness disagrees with the
handler's (`service_witness_mismatch`, naming the field) or is absent (`native_absent`) must
leave the descriptor `failed`, never frozen at `starting` (ADR 012 WS-B.5).

Proving that needs a real restart, which kills live panes. Do it deliberately — on a machine with
nothing running, or with the user's agreement — then confirm the descriptor reaches `ready` with
exactly one sentinel and no canary pane.

## Reference files

- `references/recovery-protocol.md` — the cold-recovery algorithm step by step, descriptor and
  generation states, journal and lease fencing, and the eligibility rules for a manifest. Read
  before changing restore, snapshot publication, or anything that acquires the recovery lease.
