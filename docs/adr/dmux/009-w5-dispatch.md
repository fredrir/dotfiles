# ADR 009: W5 dispatch record — P7 remote/routes and P8a local hierarchy

Status: active (W5). Root-owned. Records the §19.2 W5 ownership starts, the
root's pre-dispatch contract extensions, and the frozen cross-agent API
surface for this wave. Base revision: the commit titled
"dmux W5 dispatch: split placement + normalize contract, remote entry
skeletons" (child of 360be98, the P6 gate commit).

## 1. Root pre-dispatch contract extensions (this commit)

Made by the root while no specialist held the paths (between W4 and W5),
mechanically migrating both adapters so the tree stays green at the
boundary:

- `backend/mod.rs` (root-frozen): added `SplitDirection`
  (`Left|Right|Up|Down`, CLI default `Down`), `SplitSpec { spec: CreateSpec,
  direction, percent: Option<u8> }` with `From<CreateSpec>` (Down/None), and
  `NormalizePlan { native_token, server_epoch, target_window, moves:
  Vec<NormalizeMove{pane_id, from_window}> }`. Trait changes:
  `split_new(..., &SplitSpec)`; new default-refusing `normalize_plan` /
  `normalize_apply` (typed `normalize_unsupported:` detail) that only the
  Wez adapter overrides.
- `backend/tmux.rs` / `backend/wez.rs`: mechanical `split_new` migration
  with deterministic placement argv — tmux `-v | -v -b | -h | -h -b` plus
  `-l N%`; wez `--bottom|--top|--right|--left` plus `--percent N`
  (`split_pane_invocation` gained the two parameters). Unit-test argv
  expectations updated accordingly; behavior-level coverage stays with the
  provider agents.
- `remote/{mod,agent,attach}.rs` + hidden `main.rs` subcommands `_agent
  --protocol N METHOD [--data-dir --lock-dir]` and `_attach --token T
  [--data-dir --lock-dir]`: entry-point skeletons with frozen signatures
  (`remote::agent::run(&AgentArgs) -> i32`, `remote::attach::run(&AttachArgs)
  -> i32`); bodies are placeholders the remote agent replaces. The hidden
  `--data-dir/--lock-dir` seams mirror `_tmux-bootstrap` so two-host tests
  never touch a production registry.

## 2. Ownership starts (plan §19 path matrix, unchanged globs)

| Agent | Phase | Owned paths (exclusive from this record) |
| --- | --- | --- |
| Identity/registry | P7 prereq | `src/{model.rs,refs.rs,error.rs,history.rs,locks.rs,registry/**}`, `tests/{identity/**,registry/**}` |
| tmux provider | P8a | `src/backend/tmux.rs`, `tests/{provider_tmux.rs,fixtures/tmux/**}` |
| Wez provider | P8a | `src/backend/wez.rs`, `tests/{provider_wez.rs,fixtures/wez/**}` (+ fork worktree globs per ADR 000; no fork change expected this wave) |
| Remote/routing | P7 | `src/{routes.rs,remote/**}`, `tests/{remote_protocol/**,fixtures/remote/**}` — dispatched after the identity handoff lands |
| Root | P8a/P7 integration | everything else per §19, incl. `operations.rs`, `main.rs`, `bootstrap.rs`, root test files |

GUI/presentation and lifecycle agents are idle in W5; the interactive zsh
prompt-hook wiring for marker refresh is deliberately deferred to W6/P9
(it edits the live shell config and must ride the same feature flag as the
GUI changes). P8a proves marker propagation through `dmux _context` and the
bootstrap path programmatically.

## 3. Frozen registry API surface for P7 (identity agent implements)

Registry schema v2 (migration from v1; `registry-v1.sql` remains the v1
contract and gains a v2 appendix when this lands):

- New table `attach_tokens(token_hash PK, request_uid UNIQUE, host_uid FK,
  space_uid, server_epoch, route, attach_argv/*JSON argv*/, issued_at,
  expires_at, state CHECK issued|redeemed|expired|revoked, redeemed_at)`.
  Only the sha256 of a token is ever stored.
- New API over existing v1 tables (names frozen; shapes may gain fields):
  - `enroll_host(host_uid, label: Option<&str>) -> EnrolledHost` —
    idempotent by HostUid; allocates the next bijective base-26 alias only
    for a new HostUid (`a` is the local host, minted at registry open).
  - `hosts() -> Vec<HostRow{host_uid, alias, label, lifecycle, enrolled_at}>`,
    `host_by_alias(&str)`, `set_host_label(host_uid, &str)` (a spelling is
    never rebound to a different HostUid),
    `forget_host(host_uid)` — refuses the local host, tombstones host +
    current refs, disables its routes, retains history.
  - `upsert_route(&RouteSpec) -> i64` keyed on (host_uid, transport,
    endpoint); `routes_for(host_uid) -> Vec<RouteRow>` priority-ordered;
    `set_route_enabled(route_id, bool)`;
    `record_route_outcome(route_id, outcome_token)`.
  - `issue_attach_token(&AttachTokenSpec)` and
    `redeem_attach_token(token_hash, now) -> AttachRedemption`
    (`Redeemed{...}` atomically single-use via a guarded UPDATE, or the
    typed `Replayed | Expired | Unknown`). Expiry/replay never deletes the
    row; the journal keeps it for audit.
  - `peer_cache(host_uid) -> Option<PeerCache>` /
    `store_peer_cache(host_uid, &PeerCache)` over `remote_cache`
    (registry_uid, authority_revision, authority_head_hash, snapshot_json).
    Lineage *policy* (§12.1 conflict/stale/rollback rules) stays in
    `remote/**`; the registry only stores and returns the checkpoint, and
    `classify_lineage` remains available to it.

## 4. Frozen inputs for the remote agent (P7)

- `remote/protocol.rs` envelope shape (ADR 008). Method constants may be
  extended additively in remote-owned code: `hello`, `spaces`, `new`,
  `rename`, `rm`, `attach_plan`.
- Entry signatures from §1; production registry/lock resolution through
  `operations::OperationEnv::production()`.
- Owner-side mutations go through `operations::{create_space, rename_space,
  remove_space}` — the agent never allocates identity client-side and never
  chooses a backend.
- Route retry matrix (§12.1/§12.3, acceptance 21/22): only enumerated
  pre-authentication transport failures try the next verified route;
  auth/host-key/identity/protocol/mutation/postcondition failures never
  retry, and no route outcome falls back to another backend. A cross-route
  retry re-sends the identical UID/method/payload.
- Two-host testing rules (Archie): rsync `scripts/rust` to a scratch path
  (e.g. `~/.cache/dmux-w5/`), build there; never touch `~/dotfiles` on
  Archie, its default tmux server, or any production registry (always the
  hidden `--data-dir/--lock-dir` seams and scratch `-L` namespaces with
  cleanup). The single live transport today is the `archie` ssh alias
  (USB link, 10.77.77.2); a Tailscale route may be probed, and absent
  transports are fault-injected (dead endpoint = transport failure class,
  wrong user/key = auth class, never retried).

## 5. W5 gates

- P8a: local both-backend hierarchy, child-orphan recovery, stale-epoch
  rejection, and marker propagation pass (root tests; providers keep their
  adapter suites green). Local normalization lands here (plan §18).
- P7: two-host identity, request replay, PTY attach, route fault, and
  backend-instance/epoch verification matrix passes.
- P8b starts only after both handoffs return and the root records them
  here.
