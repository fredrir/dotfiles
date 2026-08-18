# ADR 007: P0 gate record — selected mechanisms and frozen artifacts

Status: accepted (P0 gate closed; every blocker has one selected, evidence-backed mechanism)
Date: 2026-08-16 (proposed), 2026-08-18 (ratified at the P11 gate)
Plan refs: §18 P0 row, §2.17, §22, §19.2 W7

Every P0 blocker has exactly one demonstrated, evidence-backed mechanism.
No fallback choice remains unresolved. No product behavior changed: the
product repository gained only `docs/adr/dmux/**`; the 116-test baseline is
green and frozen in `baseline-tests.json`.

## Blocker → selected mechanism

| P0 blocker | Selected mechanism | ADR |
| --- | --- | --- |
| Strict exact-socket/epoch targeting | Stock CLI: non-empty `WEZTERM_UNIX_SOCKET` + `--no-auto-start` + sanitized env, with dmux-side deadline, errno/peer-pid probe, dev-ino check, and sentinel-in-list epoch handshake. No fork selector. | 001 |
| Service/sentinel default suppression | `mux-startup` handler spawns the `dmux:system:<epoch>` sentinel (suppression via non-empty mux + canary `default_prog` tripwire); env-passed epoch; handler-written descriptor; foreground server under the service manager with config-pinned `socket_path`. | 002 |
| `start --attach` presentation | `wezterm start --class <uniq> --always-new-process --domain <d> --attach`, owner diff-zero on nonempty and sentinel-only domains; sentinel existence is a mandatory pre-attach check (empty-domain attach spawns); `start --domain` without `--attach` banned. | 003 |
| Atomic activate-existing | In-GUI pcall'd `wezterm.mux.set_active_workspace` (raises on missing, creates nothing); `SwitchToWorkspace` banned from activation paths; same-callback atomicity race-proven (2×2500 iterations, zero leaks). No fork primitive. | 003 |
| Acknowledged authorized bridge | File-spool under `dmux_runtime_dir()/bridge/`, 50 ms poll, HMAC-signed one-use requests, frozen req/ack schema, replay/expiry/not-found proven, ~31 ms median latency, heartbeat liveness. | 003 |
| Atomic Wez adoption | Fork CAS primitive `rename-workspace --window-id --if-workspace [--if-sole-window]` (PDU 63/64, codec 45→46), single-write-lock check+swap, prototyped/built/demoed; `(epoch, window_id)` as the stable native ref — no fork-side generation counter. | 006 |
| Provisional pane bootstrap + correlation + orphan recovery (both providers) | Helper + per-uid FIFO (O_RDWR open), reserved `dmux-bootstrap:<uid>` title, three-way correlation (spawn return = title scan = inherited pane env), exec-in-place; full orphan matrix proven incl. duplicate-uid conflict and self-closing timeout panes. | 004 |
| Kill convergence | Bounded re-list/kill by exact pane id under the exclusive instance lease with final re-verify; adversarial non-convergence reported honestly at the bound. | 005 |
| tmux marker passthrough | DCS `tmux;` wrap, ESC-doubled OSC 1337 SetUserVar, base64 value; **`allow-passthrough all` asserted per managed session** (`on` drops from invisible panes; default is `off`). | 005 |
| Non-reentrant `mux-startup` recovery | In-process `wezterm.mux` restore only (recursive CLI self-call proven to hang). Registry-only lease helper is permitted as a bounded, watchdog-killed child that never touches a mux socket; watchdog expiry marks startup `failed` (fail closed). Coordinator-owned-lease fallback rejected as unnecessary. | 002 |

## Frozen artifacts

- Exact argv/config templates: ADRs 001 (CLI), 002 (server config + handler
  shape), 003 (GUI attach argv, bridge parameters).
- Bootstrap handshake and orphan proof: ADR 004 (spawn-return formats, FIFO
  protocol, timeout exit 41, orphan matrix).
- Failure modes: ADR 001 (endpoint classification), 002 (auto-start matrix,
  socket steal), 003 (empty-domain spawn, callback death), 004 (zero-orphan
  takeover), 005 (convergence TOCTOU, rename-workspace silent failures).
- Bridge request/ack schema: ADR 003 §3.
- Startup/recovery coordinator: ADR 002 + non-reentrancy row above.
- Fork requirements: exactly one primitive (adoption CAS) plus optional
  follow-ups (mux-side user-var exposure; attach/detach PDUs) explicitly NOT
  selected — stock mechanisms proved sufficient for those paths.
- Baseline test IDs/results: `baseline-tests.json` (116/116 pass at `039e2ee`).

## Plan corrections discovered (root will fold into contracts at P1)

1. §15.1: `no_serve_automatically` does not gate CLI auto-start; the
   invariant is carried by `--no-auto-start` + non-empty `WEZTERM_UNIX_SOCKET`
   on every dmux CLI call (empty string falls through to discovery). The
   config knob stays as defense-in-depth; GUI-side fail-closed was proven.
2. §11.1/§10.3: owner-side headless verification cannot read Wez user vars
   (GUI-only events; no CLI list field). Pane-stamp health verification is
   registry-ack-based; GUI-side correlation is unaffected. Optional fork
   exposure deferred (not required for any P11 gate).
3. Server sockets are pinned only by `unix_domains[].socket_path` in config;
   `WEZTERM_UNIX_SOCKET` does not control a server's listen path, and
   `--daemonize` contends the shared default pid lock → the P5 service runs
   the server foreground with a generated config.
4. tmux passthrough requires `allow-passthrough all` (not `on`).
5. macOS `sun_path` limit (~104 bytes) makes short runtime socket paths a
   doctor-verified requirement.

## Gate disposition

All §18 P0 exit criteria are met. This record is ratified: the P0 gate is
closed, and §22's first clause ("every P0 blocker has one selected,
checked-in, evidence-backed mechanism; no bridge, endpoint, adoption,
service, or recovery fallback remains undecided") is satisfied by the
blocker table above.

Ratification was overdue. P1 through P10 were built against this record while
its status still read "proposed", so the gate it describes was in force in
practice long before it was recorded as closed. Ratifying it now states the
fact rather than changing it; nothing in the blocker table or the frozen
artifacts is altered by this edit.

### Consequences of closing the gate

1. **P3c path globs are frozen** (ADR 000 deferred this "to the P0 gate").
   The globs are exactly those recorded in ADR 000 and are now final:
   `codec/src/**`, `mux/src/lib.rs`, `mux/src/window.rs`,
   `wezterm-client/src/client.rs`, `wezterm-mux-server-impl/src/**`,
   `wezterm/src/cli/**`.
2. **W7 ownership is well-defined.** Plan §19.2 item 8 says "specialists
   stop; root integrates". Because the P0 gate never closed, the specialist
   globs it depends on were never frozen, so at W7 no writer was defined for
   the one known P11 blocker (the ssh-domain proxy socket, in
   `shared/wezterm/wez/{dmux_bridge,remote}/**`). Closing the gate resolves
   this: at W7 every specialist glob reverts to the root integrator, which
   is what "specialists stop; root integrates" means. The root records that
   reclamation here rather than re-dispatching a specialist.
3. The spike worktree `~/packages/wezterm-dmux-p0` may be deleted; P3c
   reimplemented the selected primitive in the canonical worktree, and
   `fredrir` now contains it (see ADR 000).

### W7 ownership reclamation (recorded per §19.2)

From this date the root integrator holds every path in the §19 table for the
duration of P11-P12. No specialist assignment is active. Editing subagents
dispatched during P11 receive a strict subset of root's paths per §19.3 and
return through the root, which reruns the phase tests.
