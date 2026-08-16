# ADR 006: WezTerm-fork CAS primitive for atomic adoption (P0 spike 6)

Status: accepted (P0 evidence; primitive selected, prototyped, and demoed)
Date: 2026-08-16
Evidence: repo `docs/adr/dmux/evidence/spike6-fork-adoption.md` (full code map, patch
diff, atomicity citations, demo transcript). Prototype commit `d045ed94a` on
branch `dmux-p0-spike` in `/Users/fredrir/packages/wezterm-dmux-p0` (local
only, never pushed; disposable — P3c reimplements in the canonical worktree).
Plan refs: §10.3, §18 P3c, §12.1

## Decision

Atomic Wez adoption is feasible via a ~210-line additive fork primitive,
implemented and live-demoed:

- **CLI:** `wezterm cli rename-workspace --window-id N --if-workspace OLD [--if-sole-window] NEW`
  — exit 0 silent on success; exit 1 with stable stderr prefixes
  (`workspace_mismatch actual="..."`, `no_such_window`, `not_sole_window`) on
  typed failure.
- **Wire:** `RenameWorkspaceIf { window_id, expected_workspace, new_workspace,
  expect_sole_window }` (ident 63) → `RenameWorkspaceIfResponse { outcome }`
  (ident 64); outcome enum `Renamed | NoSuchWindow | WorkspaceMismatch{actual}
  | NotSoleWindow{other_window_ids}`. Every non-`Renamed` variant guarantees
  zero mutation.
- **Core:** `Mux::rename_workspace_for_window_if` performs existence +
  expectation + sole-window checks + rename under **one** `windows.write()`
  lock.

### Generation/epoch mapping (resolves §10.3's open requirement)

No per-window generation counter is needed in the fork. The atomic
server-side check of `(window_id, expected_workspace, sole-window)` is the
entire relevant source state; epoch verification stays dmux-side, bracketing
the call. `WindowId` is process-monotonic (`mux/src/window.rs:6-7`), so
`(epoch, window_id)` is a stable native ref. This is stronger than the plan's
stated acceptable minimum.

## Atomicity evidence

The headless mux server's main loop is `loop { executor.tick() }` over a
single-threaded `SimpleExecutor` (`wezterm-mux-server/src/main.rs:231,248-250`;
`promise/src/spawn.rs:186-224`); each connection's dispatch future runs on
that same thread (`wezterm-mux-server-impl/src/local.rs:20-31`) and every
mutating PDU handler is a zero-await closure (`sessionhandler.rs:247-297`) —
one PDU handler is atomic w.r.t. all other clients. The primitive additionally
holds a single write lock across check+swap. Live demo proved a
stale-expectation CAS after a concurrent external rename fails typed with no
mutation.

Demo matrix (against a scratch server running the prototype build): CAS
success → opaque key; stale expectation → `WorkspaceMismatch{actual}`, no
mutation; `no_such_window`; `not_sole_window`, no mutation; success after
resolving the interloper.

## Codec / compatibility

- `CODEC_VERSION` bumped 45→46. The version handshake is exact-match but is
  called **only on GUI attach paths** (`wezterm-gui/src/main.rs:556`,
  `wezterm-client/src/domain.rs:969`); **`wezterm cli` performs no codec
  handshake at all** (P3c demo evidence). A codec-45 `wezterm cli list`
  works against a codec-46 server; version skew on the CLI path is
  *silently half-capable*, not fail-closed.
- The CAS verb itself is safe in every cross-version direction: a fork CLI
  sending ident 63 to a codec-45 server gets a typed
  `ErrorResponse { reason: "Error: invalid PDU Invalid { ident: 63 }" }`,
  exit 1, zero mutation.
- Consequence (amended at P3c): dmux must gate CAS use on a **positive
  capability probe** — fork version match, or classifying the stable
  `invalid PDU Invalid { ident: 63 }` error as `capability_missing` — and
  never infer capability from connect success. Lockstep fork upgrade on both
  hosts remains the rollout requirement for GUI attach compatibility.
- Unknown idents decode to `Pdu::Invalid` and `wezterm cli proxy` is a raw
  byte pump, so the verb traverses SSH proxies unchanged.

## Assessment of the other conditional fork primitives

- **Strict socket selector:** fork unnecessary (confirms ADR 001).
  `compute_unix_domain` (client.rs:1222-1255): a non-empty
  `WEZTERM_UNIX_SOCKET` short-circuits to the exact path; `--no-auto-start`
  is plumbed. Caveat: an **empty-string** value falls through to discovery —
  dmux must always set it non-empty.
- **attach-domain / detach-domain CLI verbs:** no PDUs exist (they are GUI
  KeyAssignments, `config/src/keyassignment.rs:633-634`); feasible via the
  GUI-hosted sessionhandler with a SpawnV2-style async shim if spike 3 finds
  the stock paths insufficient.
- **activate-existing-workspace:** feasible but needs client-identity
  targeting design plus a no-create check (GUI `SwitchToWorkspace`
  creates-if-absent); decision owned by ADR 003 (bridge spike).

## P3c fork ownership globs (recorded in ADR 000)

Exact files touched by the prototype: `codec/src/lib.rs`, `mux/src/lib.rs`,
`wezterm-client/src/client.rs`,
`wezterm-mux-server-impl/src/sessionhandler.rs`,
`wezterm/src/cli/rename_workspace.rs`.
Ownership globs: `codec/src/**`, `mux/src/lib.rs`, `mux/src/window.rs`,
`wezterm-client/src/client.rs`, `wezterm-mux-server-impl/src/**`,
`wezterm/src/cli/**`.

## Risks / follow-ups

- Codec-bump rollout sequencing across both hosts (P3c gate: pinned fork
  build available before downstream use).
- CLI failure surface is stderr-prefix-based; JSON/exit-code refinement is a
  small P3c follow-up.
- Attached-GUI notification behavior on CAS rename compiled but not demoed.
- Upstreaming the primitive is plausible and would eliminate fork carry.
- Build timings (Apple Silicon, debug): cold `cargo check` 1:48; incremental
  build 24.5s — fork iteration cost is low.
