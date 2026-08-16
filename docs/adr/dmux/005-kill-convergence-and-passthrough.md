# ADR 005: Kill convergence, tmux passthrough, stock CLI gaps (P0 spike 5)

Status: accepted (P0 evidence; mechanisms selected; fork-gap set confirmed)
Date: 2026-08-16
Evidence: repo `docs/adr/dmux/evidence/spike5-convergence-passthrough.md` (wezterm 20260813-114614-18a44cb7, tmux 3.7b)
Plan refs: §14 Wez remove, §13.1, §10.3, §11.1

## 1. Wez removal: bounded re-list/kill loop is selected and sound

Loop shape: list panes of the exact workspace → `kill-pane` each by id →
re-list → repeat, max N rounds.

| Scenario | Result |
| --- | --- |
| Clean 4-pane workspace (splits + 2nd window) | converged in 1 round; workspace gone; other workspaces untouched |
| `kill-pane` on already-dead pane | `Error: no such pane`, exit 1, no hang — benign race, treat as success-equivalent |
| Adversary spawning at 0.3s cadence | converged at observation time, **but workspace was back (46 panes) seconds later — exit 0 is point-in-time only** |
| Tight adversary | bound hit at N=5 (227→…→8), remaining pane ids reported, distinct exit, no infinite loop |
| Rerun after adversary stopped | converged; 383 panes killed in one round |

Consequences: the bounded loop is honest under attack (matches §14
partial/failure + no tombstone). But convergence exit 0 must be **fenced**:
tombstoning requires either the exclusive backend-instance mutation lease
held across the whole remove (so no managed spawner can race) plus one final
post-kill re-verify list, and external races surface as the plan's `conflict`.

## 2. Frozen tmux passthrough recipe (mandatory for markers inside tmux)

```sh
printf '\033Ptmux;\033\033]1337;SetUserVar=%s=%s\007\033\\' \
  "$NAME" "$(printf '%s' "$VALUE" | base64)"
```

DCS `tmux;` wrap, ESC doubled inside the payload, OSC terminated with BEL,
value base64.

- **Required option: `set -g allow-passthrough all` — not `on`.** Compiled-in
  3.7b default is `off`. The dotfiles currently set `on`
  (`shared/tmux/00-core.conf:11`), which passes only from **visible** panes:
  an emit from a non-active window under `on` is silently and permanently
  dropped (not buffered). Under `all` it lands. dmux must assert
  `allow-passthrough all` per managed session, never assume ambient config.
- Plain unwrapped OSC 1337 never escapes tmux (wrapping is mandatory, as the
  plan assumes). Wrapped OSC 2 also passes (sets the outer WezTerm title
  directly).

## 3. User-var observability: GUI-only in stock WezTerm

- A GUI-side `user-var-changed` Lua handler is the **only** channel that
  observes SetUserVar. A headless `wezterm-mux-server` does not fire the
  event into config Lua, and `cli list --format json` exposes no user-var
  field (full key set recorded in evidence).
- Consequences: GUI-side marker correlation (§13.2) is unaffected — the
  bridge runs in the GUI and can read user vars. But any **owner-side
  headless verification** of Wez pane stamps cannot read markers back;
  pane-stamp acknowledgement must be registry-side (the `dmux context stamp`
  ack the plan already records) and/or a fork primitive exposing user vars to
  `cli list`. Decision deferred to the P0 gate with spike 6's findings.

## 4. Stock CLI gap set (this fork build = stock upstream surface)

Live-tested `rename-workspace` semantics make it unusable for adoption:

- renames **all** windows sharing the name (not window-scoped);
- missing source → **silent no-op, exit 0**;
- target collision → **silent merge, exit 0**;
- `--pane-id` merely resolves the name, then renames globally.

There is **no CAS and no window-id-scoped workspace verb anywhere** in the
stock CLI: workspace is only assignable at window creation
(`spawn --new-window --workspace`, `move-pane-to-new-tab`); no verb
re-assigns an existing window's workspace.

Confirmed fork-primitive gap set for the P0 gate decision (with spike 6):
1. window-id-scoped, compare-and-swap workspace assignment (adoption);
2. optionally, mux-side user-var exposure (headless stamp readback).

Any use of stock `rename-workspace` (even in migration tooling) requires
pre/post verification via `cli list` JSON — its exit code encodes nothing.

## Reconfirmed hazards

- `WEZTERM_UNIX_SOCKET` is ignored by `wezterm-mux-server --daemonize` (bound
  the default socket; killed instantly) — server sockets are pinned only via
  `unix_domains[].socket_path` in config (same finding as ADR 004; P5 service
  requirement).

## Untested (tracked)

- Nested tmux double-wrapping; tmux clobbering passthrough-set titles on
  refresh.
