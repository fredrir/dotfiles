# ADR 000: Worktrees, pinned revisions, and fork path ownership

Status: accepted; P3c path globs frozen at the P0 gate (ADR 007, ratified 2026-08-18)
Date: 2026-08-16
Owner: root integrator

## Product repository

| Item | Value |
| --- | --- |
| Worktree | `/Users/fredrir/dotfiles` |
| Branch | `dmux` |
| Base revision at P0 start | `039e2ee` (finalize dmux plan) |

Only the root integrator writes product-repository ADRs/contracts during W0/P0.
No spike patch merges into this repository as product behavior.

## WezTerm fork (`fredrir/wezterm`)

| Item | Value |
| --- | --- |
| Canonical worktree | `/Users/fredrir/packages/wezterm` |
| Branch | `fredrir` |
| Pinned revision | `b9d8dfae226370deb0a963eadb8844975cbca980` (was `9e6323bb5`; `fredrir` fast-forwarded to `dmux-primitives` on 2026-08-18 and pushed) |
| Installed build | `20260817-233913-b9d8dfae` — identical version installed on Macie (`/opt/homebrew/bin/wezterm`) and Archie, and now identical to `fredrir` HEAD |
| Build command | `cargo build --release -p wezterm -p wezterm-gui -p wezterm-mux-server` (macOS local bundle produced as `WezTerm-macos-local`; Arch uses the pinned PKGBUILD in `9e6323bb5`) |
| P0 spike worktree (disposable) | `/Users/fredrir/packages/wezterm-dmux-p0`, branch `dmux-p0-spike`, created from `9e6323bb5` |
| P3c working worktree | `/Users/fredrir/packages/wezterm-dmux`, branch `dmux-primitives`, created from `9e6323bb5`; the Wez provider/fork agent's exclusive fork workspace within the P3c globs below. **Merged:** `fredrir` was fast-forwarded to `dmux-primitives` (`b9d8dfae2`) on 2026-08-18 and pushed to `origin/fredrir`, satisfying the "required before P6 adoption goes live" condition. The build already installed on both hosts is that revision, so no reinstall was required. |

Rules:

- The P0 spike worktree is disposable. Its branch is never merged directly;
  P3c reimplements any selected primitive in the canonical worktree under the
  Wez provider/fork agent's ownership.
- During P0 the spike agent may edit any path inside the spike worktree only.
  It never pushes, and never edits `/Users/fredrir/packages/wezterm` itself.
- Exact P3c relative path globs (the Wez provider/fork agent's exclusive fork
  ownership), derived from the P0 spike patch's actual footprint
  (ADR 006, prototype commit `d045ed94a` on `dmux-p0-spike`):
  `codec/src/**`, `mux/src/lib.rs`, `mux/src/window.rs`,
  `wezterm-client/src/client.rs`, `wezterm-mux-server-impl/src/**`,
  `wezterm/src/cli/**`.
  No fork edit outside the spike worktree is permitted until P3c begins under
  these globs in the canonical worktree. These globs are **frozen** as of the
  P0 gate ratification (ADR 007, 2026-08-18); they are final for the first
  release.

## Resurrection fork (`fredrir/resurrect.wezterm`)

| Item | Value |
| --- | --- |
| Canonical worktree | `/Users/fredrir/packages/resurrect.wezterm` |
| Branch | `main` |
| Pinned revision | `f40db0c` |
| Build command | none (pure Lua plugin; consumed via `wezterm.plugin.require('https://github.com/fredrir/resurrect.wezterm')`) |
| Runtime consumer | `shared/wezterm/wez/plugins/resurrect.lua` |

Rules:

- Owned by the lifecycle/recovery agent from P3c-era work onward, per plan §19.
- P0 does not edit this fork; recovery spikes use scratch Lua configs only.

## Scratch and evidence locations

- All P0 spike scratch state (sockets, configs, FIFOs, logs) lives under the
  session scratchpad, never under `~/.local/share/wezterm` or the default tmux
  server socket.
- Spike evidence files are copied/summarized into `docs/adr/dmux/` ADRs by the
  root integrator only.
