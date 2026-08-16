# ADR 003: GUI attach, activate-existing, and acknowledged bridge (P0 spike 3)

Status: accepted (P0 evidence; mechanisms selected; no GUI fork primitive required)
Date: 2026-08-16
Evidence: repo `docs/adr/dmux/evidence/spike3-gui-bridge.md` (verbatim configs incl.
full bridge Lua, before/after owner diffs, ack files, latency data)
Plan refs: §13.2, §13.4, §14, §15.1

## 1. Selected `--launch-gui` attach mechanism (§13.2 evaluation order: mechanism 1 passes)

Frozen argv:

```text
env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=<exact-socket> \
  wezterm --config-file <gui-cfg> start --class <unique-class> \
  --always-new-process --domain <domain> --attach
```

- Nonempty domain: owner-side diff ZERO (no pane/tab/window/workspace
  created). Sentinel-only domain: diff ZERO. GUI-local state also clean.
- Plain `connect <domain>` is likewise diff-zero on nonempty/sentinel-only
  domains (usable for the in-GUI/default-startup path).
- **On a truly empty domain both `start --attach` and `connect` spawn a
  default pane; `start --domain` without `--attach` spawns even on a nonempty
  domain.** No invocation is unconditionally no-create. The
  `dmux:system:<epoch>` sentinel is therefore the load-bearing precondition
  for every attach: dmux must verify the sentinel exists (via exact-socket
  list) before any GUI attach, and never attach to a sentinel-less domain.
  `start --domain` without `--attach` is banned outright.

Stopped server + `no_serve_automatically=true`: `start --attach` and
`connect` both fail closed (GUI logs the connect failure and terminates); zero
servers birthed in stale-socket and no-socket cases. This closes the GUI-side
gap left open in ADR 002.

## 2. Activate-existing: in-GUI compare-and-activate is sound; no fork primitive

- Hazard confirmed: `SwitchToWorkspace { name = <missing> }` silently creates
  an owner workspace+window+pane. **Banned from all activation paths.**
- Selected primitive: pcall'd `wezterm.mux.set_active_workspace(name)` — it
  raises on a missing workspace and creates nothing, so it fails closed even
  when the pre-check is stale.
- Race evidence: 2×2500-iteration in-GUI probes against a shell adversary
  creating/killing the target workspace produced **zero** create-on-miss
  leaks for same-callback composites — GUI Lua callbacks are atomic w.r.t.
  the GUI mux model (single-threaded event loop). A deterministic split test
  (check and switch in different callbacks, workspace killed in between)
  proved the stale `SwitchToWorkspace` path silently creates — hence the ban
  and the fail-closed primitive choice.
- Every bridge callback must be pcall-wrapped: an uncaught Lua error silently
  kills the `call_after` chain (observed) — see watchdog requirement below.

## 3. Frozen bridge contract (file-spool transport)

Transport: request/ack spool directory under `dmux_runtime_dir()/bridge/`
(dir 0700, files 0600), GUI poll loop at 50 ms. Proven round trip and
semantics:

- Request: `{uid, action, target, nonce, expiry}`;
  Ack: `{uid, action, nonce, ok, error?, completed_at, workspace?, window_ids?}`
  (window_ids are GUI-side ids, per §9.1).
- Proven: successful activate; `not_found` with owner diff-zero; replay →
  original ack preserved byte-identical plus a distinct `replayed` error doc;
  `expired` rejection; toast action (API level).
- Latency over 10 activates: min 12.6 / median 31.3 / max 37.0 ms — file
  spool comfortably meets interactive needs.

Root-frozen parameters completing §13.2's required schema (mechanics proven
by the spike; values normative for P9):

| Parameter | Frozen value |
| --- | --- |
| Transport | file spool under `dmux_runtime_dir()/bridge/` (0700/0600) |
| Authorization | HMAC-SHA256 over the canonical request doc; per-boot key file issued by the runtime broker (0600); GUI reads key at config load |
| Max message size | 64 KiB |
| Request TTL | 10 s (expiry field, GUI-enforced) |
| Client timeout | 5 s wait for ack, then typed `bridge_timeout`, fail closed |
| Replay persistence | consumed-uid set in GUI memory + consumed request files renamed; acks preserved for idempotent re-read |
| Origin variants | `in_gui` and `cold_launcher` per §13.2; actions requiring a pane reject `cold_launcher` |
| Liveness | GUI poller heartbeat file updated each cycle; a stale heartbeat means bridge-down (fail closed), guarding against silent callback death |

## 4. Detach and zero-window re-attach

- `DetachDomain` → owner diff-zero; pane shell PIDs survive; after SIGKILL of
  the GUI, owner state intact and pane ids stable across re-attach.
- `perform_action(AttachDomain)` requires a GUI window (fails `no_gui_window`
  at zero windows). The window-independent
  `wezterm.mux.get_domain(d):attach()/:detach()/:state()` works from any
  state including zero-window re-attach (Detached→Attached, window
  materialized, owner diff-zero) — **selected primitive for §13.4's
  Hammerspoon zero-window path**, replacing the forbidden
  `cli spawn --new-window`.

## Risks carried forward

- Empty-domain spawn edge: guarded by the mandatory sentinel precondition
  (ADR 002); doctor should verify sentinel before any presentation plan.
- Silent Lua callback death: heartbeat requirement above; P9 tests must kill
  the poller and assert fail-closed behavior.
- One poller per GUI instance: spool must be per-GUI-instance-scoped to avoid
  collisions when two GUIs run (P9 design detail; runtime dir layout).
- Toast visual delivery unverified (API succeeded); verify manually in P9.
- Benign mirror staleness of GUI-local workspace list — never used for
  authorization, display only.
