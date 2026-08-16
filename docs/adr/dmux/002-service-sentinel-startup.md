# ADR 002: Service-owned mux, sentinel, and startup suppression (P0 spike 2)

Status: accepted (P0 evidence; mechanism selected; one plan assumption corrected)
Date: 2026-08-16
Evidence: repo `docs/adr/dmux/evidence/spike2-sentinel-startup.md` (wezterm 20260813-114614-18a44cb7)
Plan refs: §15.1, §15.3, §8.1

## Decision

The `mux-startup` sentinel mechanism is feasible and selected. Frozen working
shape (verbatim config in evidence):

- config: `unix_domains = {{ name, socket_path = <short-path>, no_serve_automatically = true }}`,
  `daemon_options` redirecting pid/stdout/stderr away from
  `~/.local/share/wezterm`, a canary `default_prog` (regression tripwire), and
- `mux-startup` handler: write descriptor `starting` → `wezterm.mux.all_windows()`
  → `wezterm.mux.spawn_window { workspace = 'dmux:system:' .. epoch, args = <idle argv> }`
  → write descriptor `ready` (JSON with state, epoch, pid).

## Proven facts

- **Default-shell suppression works on both start paths** (foreground and
  `--daemonize`): after startup, `cli list` shows exactly one pane — the
  sentinel — no default-workspace row, canary never fired. Suppression works
  because the handler leaves the mux non-empty; keep the canary `default_prog`
  forever as the tripwire for that assumption.
- `mux-startup` runs **exactly once per server start** (4 starts → 4 runs, one
  per PID, each with its own epoch); SIGTERM + restart re-runs it with the new
  env epoch.
- **Env propagation works**: `DMUX_SERVER_EPOCH` from the launcher env reaches
  Lua and survives the `--daemonize` re-exec. `wezterm.procinfo.pid()` returns
  the real server PID for the descriptor.
- **In-process mux Lua works during startup**: `all_windows()` correct
  pre/post spawn (order arbitrary — never use ordinals); three windows with
  distinct workspaces/cwds/args spawned in-process, pre-attach, instantly.
- **Recursive CLI self-call from `mux-startup` hangs** (zero output until a 5s
  watchdog SIGKILL; readiness blocked meanwhile). The plan's in-process-only
  recovery rule and bounded-child rule are both proven necessary.
- **Sentinel filtering needs no extra RPC**: list JSON `workspace` carries the
  full `dmux:system:<epoch>` string; prefix exclusion + epoch handshake ride
  the normal list fields.

## Correction to plan §15.1 assumptions

`no_serve_automatically = true` does **NOT** prevent CLI-triggered
auto-start. The only effective CLI knob is `--no-auto-start`
(`--prefer-mux` behaves like plain cli). Worse, the auto-start path is
actively dangerous:

- Auto-start spawns `wezterm-mux-server --daemonize` **without propagating
  `--config-file`** — the spike birthed default-config servers bound to the
  user's default `~/.local/share/wezterm/sock` while the calling CLI still
  failed.
- `--config-file` does not scope `wezterm cli` socket resolution at all; with
  `WEZTERM_UNIX_SOCKET` unset, `cli list` connected to the live GUI's default
  socket. The env var is mandatory for endpoint identity (reinforces ADR 001).

Consequence: "Wez CLI/domain auto-start is disabled" (§2.16) is implemented by
the invariant that **every dmux-issued CLI call carries `--no-auto-start` +
explicit `WEZTERM_UNIX_SOCKET`** — not by server-side config. Keep
`no_serve_automatically = true` as defense-in-depth for domain attach paths;
its GUI-side semantics are tested in spike 3 (ADR 003).

## Duplicate start = silent socket steal

A second server on the same config/socket starts with **no error**; the path
resolves to the newcomer, orphaning the original (alive but permanently
unreachable; killing the thief leaves a dead path). Also: sockets are never
unlinked on shutdown, so stale socket files are meaningless.

Consequences: service-manager start serialization is mandatory (§2.16
confirmed); the epoch-in-sentinel handshake detected the swap end-to-end; the
runtime descriptor should record socket dev/inode (ADR 001 agrees).

## Risks carried forward

- GUI-side `no_serve_automatically`/attach auto-start semantics → spike 3.
- A slow `mux-startup` handler delays readiness; the recovery algorithm
  (§15.3) must keep its bounded, observable `starting` state.
