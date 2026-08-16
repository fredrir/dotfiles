# ADR 008: P1 frozen contracts (v1) and W1→W2 handoff record

Status: accepted (P1; contracts frozen)
Date: 2026-08-16
Plan refs: §18 P1, §19 W1, §19.1 handoffs 1–2, §16, §13.1, §12.1, §10.1

The normative Rust encodings live in the dmux library target
(`scripts/rust/crates/dmux/src/{model,refs,error,backend/mod,remote/protocol}.rs`);
this ADR freezes the cross-language JSON/marker examples and records what P0
corrections were folded in. Where prose and code disagree, the code plus its
contract tests win, and the divergence is a bug to fix here.

## 1. Bounded JSON document (v1)

Exactly one document per bounded command; no ANSI or human diagnostics on
stdout. Partial results carry typed `errors[]` and exit 7. JSON destructive
commands never prompt: without `--yes` they emit one `confirmation_required`
document, change nothing, and exit 5.

```json
{
  "schema_version": 1,
  "ok": true,
  "action": "list",
  "result": [
    {
      "managed": true,
      "uri": "dmux://0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002/spaces/0192aaaa-bbbb-7ccc-8ddd-eeeeffff1111",
      "portable_ref": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002:2",
      "compact_ref": "2",
      "space_uid": "0192aaaa-bbbb-7ccc-8ddd-eeeeffff1111",
      "space_no": 2,
      "name": "dotfiles",
      "owner": { "host_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002", "alias": "a", "label": "macie" },
      "backend": "wez",
      "backend_instance": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0004",
      "groups": 2,
      "splits": 4,
      "lifecycle": "active",
      "observation": "live",
      "health": "healthy",
      "client": "attached",
      "route": "local",
      "stale": false
    },
    {
      "managed": false,
      "native_ref": "native:wez:c2Nyw6F0Y2g",
      "provider": "wez",
      "native_name": "scratch",
      "owner": { "host_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002", "alias": "a", "label": "macie" },
      "backend_instance": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0004",
      "server_epoch": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0005",
      "groups": 1,
      "splits": 1,
      "health": "unknown"
    }
  ],
  "errors": [],
  "authority_revision": 42
}
```

Managed and unmanaged rows are different tagged objects (`managed`); an
unmanaged row never carries a canonical/compact ref or Space UID/number
(plan §16.2 — dmux never fabricates a SpaceNo to fill the table).

Confirmation document (exit 5):

```json
{
  "schema_version": 1,
  "ok": false,
  "action": "rm",
  "result": null,
  "errors": [
    { "code": "confirmation_required", "message": "rm needs --yes in JSON mode",
      "target": "dmux://0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002/spaces/0192aaaa-bbbb-7ccc-8ddd-eeeeffff1111" }
  ],
  "authority_revision": 42
}
```

Error codes and the exit-status mapping are normative in `src/error.rs`
(`every_code_maps_into_the_plan_exit_table` is the contract test).

## 2. Pane marker schema (v1, plan §13.1 + P0 corrections)

```text
DMUX_CONTEXT_VERSION=1
DMUX_HOST_UID=<uuid>
DMUX_SPACE_UID=<uuid>
DMUX_SPACE_NO=<number>
DMUX_BACKEND=wez|tmux
DMUX_DOMAIN=<stable-domain-or-empty>
DMUX_SERVER_EPOCH=<uuid>
DMUX_GROUP_REF=<epoch-qualified-live-ref>   # child_suffix form, e.g. g<epoch>.wz-3
DMUX_SPLIT_REF=<epoch-qualified-live-ref>   # p<epoch>.wz-4
```

Emission is OSC 1337 SetUserVar (base64 value). Inside tmux, the DCS `tmux;`
wrap from ADR 005 is mandatory and the managed session must assert
`allow-passthrough all`. P0 corrections folded in: user vars are observable
only by an attached GUI (ADR 005 §3), so marker readback is GUI-side
correlation only; owner-side stamp health is registry-ack-based. A marker is
a locator hint, never authorization.

## 3. RPC envelope (v1, plan §12.1)

The golden example is embedded and round-trip-tested in
`src/remote/protocol.rs` (`golden_envelope_round_trips`); `PROTOCOL_VERSION = 1`
requires an exact match. Envelope fields: `protocol_version, request_uid,
method, payload_sha256, host_uid, registry_uid, authority_revision,
authority_head_hash, backend_instance_uid?, server_epoch?, capabilities,
payload | error` (exactly one of the last two; optional fields are omitted,
never null). Idempotency: request UID durably bound to method + payload
digest + final/unknown result; reuse with a different digest rejected.
Interactive tmux attach uses the `AttachPlan` single-use-token PTY channel,
never the JSON RPC.

## 4. Registry, leases, locks

`docs/adr/dmux/registry-v1.sql` is the frozen storage contract (P2 implements
it; equivalent index names allowed, weaker semantics not). The lock
acquisition order of plan §10.1 is normative and unchanged: authority gate →
decision locks in exact-byte lexical order → backend-instance lock(s) by
BackendInstanceUid → Space lock; release in reverse; no decision lock after
backend/Space.

## 5. Reference grammar

`src/refs.rs` is the normative structural grammar (plan §6.2/§6.3), with the
truth-table contract tests. Frozen P0-side details it encodes: ID-shaped
tokens are never names; `0`/leading-zero SpaceNo forms are invalid refs;
`<host-token>:<digits>` is numeric before any name lookup; aliases are
bijective base-26 with `z → aa`; handles are `wz-<dec>`, `tx-<dec>`,
`x-<b64url>`; new managed names exclude the ID-shaped classes.

## 6. Dependencies (P1, root-only)

Workspace-managed and lockfile-pinned: `uuid 1.24.1` (v4/v7/serde),
`rusqlite 0.40.2` (bundled/backup). Upgrades only through the normal
full-suite gate.

## 7. W1→W2 handoff record

W1 (root-only) produced the frozen skeletons. **Executed 2026-08-16**: the W2
base revision is the commit titled "dmux W2 dispatch: runtime resolver,
handoff stubs, ownership record" (contracts frozen at `cb780bd`). Specialist
ownership below is live from that commit; specialists do not edit outside
their globs, do not change Cargo manifests, and stop with an evidence-backed
report if implementation contradicts a frozen contract.

| Path | From | To (W2) |
| --- | --- | --- |
| `src/model.rs`, `src/refs.rs`, `src/error.rs` | root | identity/registry agent |
| `src/history.rs`, `src/locks.rs`, `src/registry/**`, `tests/{identity,registry}/**` | root (stubs) | identity/registry agent |
| `src/backend/tmux.rs`, `tests/provider_tmux.rs`, `tests/fixtures/tmux/**` | — | tmux provider agent |
| fork worktree per ADR 000 P3c globs + `tests/fixtures/wez/**` | — | Wez provider/fork agent |
| `src/remote/protocol.rs` | root | remote/routing agent (dormant until P7 scaffolding) |
| `src/backend/mod.rs`, `tests/provider_contract.rs`, `src/lib.rs`, `src/runtime.rs` | root | stays root-owned |

Specialists deliver working-tree changes only; the root reviews, runs the
full suite, and commits each handoff (repo hooks demand `cargo fmt`, which
the root runs at integration).

Gate result: full suite green at 137 tests — the 116-test baseline
(`baseline-tests.json`) unchanged plus 21 additive contract tests; zero
behavior or help-text changes (the binary target was not modified).
