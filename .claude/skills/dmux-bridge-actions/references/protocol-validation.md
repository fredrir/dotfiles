# Bridge protocol v1 validation

Everything here is in `shared/wezterm/wez/dmux_bridge/protocol.lua` unless noted.

## Contents
- Validation order
- The key allowlists
- Origin kinds
- Field validators
- Replay, TTL, and idempotency
- What an unknown action costs

## Validation order

`validate_and_authenticate` runs these in a fixed sequence. The order is load-bearing — two steps
in particular are deliberately placed:

1. `exact_keys(request, TOP_KEYS)` — any unknown top-level key is `malformed_request`
2. required fields present
3. protocol version
4. request uid
5. **action allowlist** → `unknown_action` if absent from `M.ACTIONS`
6. `nonce` / `replay_key` shape (32–128 lowercase hex)
7. `issued_at` / `expiry`, with TTL bounded to `MAX_TTL_SECONDS` (10)
8. origin shape, by kind
9. origin × action policy (`PANE_ACTIONS`, `resident_gui` restrictions)
10. target schema (`validate_target`)
11. cold-launcher target≡origin binding
12. `hmac_sha256` shape — 64 lowercase hex characters
13. bridge key shape — exactly 32 bytes
14. **HMAC verification** over `canonical.signing_document(request)`
15. **freshness** — expiry, then `issued_at` no more than 2s in the future

Step 14 precedes step 15 so an otherwise valid expired request still receives a client-verifiable
typed acknowledgement carrying the canonical digest, rather than looking like bridge corruption.

`signing_document` is the request minus its own `hmac_sha256` field, canonically encoded.
Comparison is `crypto.constant_time_equal`, which folds length into the accumulator so a truncated
signature costs the same as a wrong full one.

## The key allowlists

Six tables, all enforced by `exact_keys`, which rejects any key not present:

| Table | Scope |
|---|---|
| `TOP_KEYS` | request envelope |
| `TARGET_KEYS` | union fallback for `target` |
| `TARGET_KEYS_BY_ACTION` | per-action `target` schema |
| `IN_GUI_KEYS` | `origin` when kind is `in_gui` |
| `COLD_KEYS` | `origin` when kind is `cold_launcher` |
| `RESIDENT_KEYS` | `origin` when kind is `resident_gui` |

Plus `ACK_KEYS` in `consumer.lua` for the response.

`validate_target` selects with `TARGET_KEYS_BY_ACTION[action] or TARGET_KEYS`. That fallback is the
single fail-open path in the subsystem: an action registered in `M.ACTIONS` but missing from
`TARGET_KEYS_BY_ACTION` silently accepts every target key any action may use.

## Origin kinds

`docs/wezterm-first-plan.md` §13.2 in the dmux repo defines two; the code has three. `resident_gui` is local
and exists because
`dmux-managed-application-quit-requested` fires with zero arguments — no window, no pane, hence no
marker.

| | `in_gui` | `cold_launcher` | `resident_gui` |
|---|---|---|---|
| Required | gui_instance, pid, process_start_token, pane_id, domain, host_uid, space_uid, space_no, backend, server_epoch, group_ref, split_ref | gui_instance, uid, pid, start_token, launcher_request_uid, domain, host_uid, backend_instance_uid, server_epoch | gui_instance, pid, process_start_token |
| Pane | required, revalidated live | none | none |
| Refused actions | — | `detach_domain`, `focus_pane`, `safe_quit` (all in `PANE_ACTIONS`) | standalone `detach_domain` |
| Extra binding | child-ref epoch and provider must agree with `backend` | target ≡ origin across domain, host_uid, backend_instance_uid, server_epoch, space_uid | must match a prepared `safe_quit` proof |
| Execution recheck | exact one-pane scan | one-use launcher witness, memoized by `launcher_request_uid` | pid + start_token + `resident_brokered()` |

Every origin must name this consumer: `origin.gui_instance ~= instance` is refused.

`in_gui` cross-field rules: both child refs must carry `origin.server_epoch`; the provider prefix
must match `backend` (`.wz-` for wez, `.tx-` for tmux, `.x-` accepted for a future opaque provider);
and `tmux_client_uid` is required exactly when `backend == 'tmux'` — a Wez `in_gui` origin that
supplies one is refused.

`cold_launcher` is allowed only for explicit `--launch-gui` and only for `attach_domain`,
`activate`, `present`, and `establish_resident`. Its witness is consumed once; a second use with a
different request uid is `launcher_witness_replayed`.

## Field validators

Local helpers returning `true` or `nil, message`. Reuse them rather than open-coding:

- `string_field(value, label, maximum)` — non-empty, byte length bounded
- `uint_field` — non-negative integer within 2^53−1, so it is safe against the
  `math.abs(math.mininteger)` hole that affects the raw range checks elsewhere
- `uuid_field`
- `domain_field` — `'^[A-Za-z0-9][A-Za-z0-9_.:-]*$'`, max 128 bytes
- `child_ref(value, prefix, label)` — returns the embedded epoch for comparison

`validate_space_target` binds the opaque workspace key by string equality:

```lua
if target.workspace ~= 'dmux:' .. target.host_uid .. ':' .. target.space_uid then
```

The workspace is derived, never free-form. It also enforces that `split_ref` implies `group_ref`,
and that both refs carry `target.server_epoch`.

## Replay, TTL, and idempotency

`request.uid` is the sole persisted one-use identity. `nonce` and `replay_key` are signed entropy
for retry correlation — changing `replay_key` for the same uid changes the canonical digest and is
a conflict, while an accidental repeat under a different uid does not invent a second persistence
index.

`consumer.lua` resolves replay against durable state, comparing prior consumed evidence and prior
primary ack:

- same uid, same digest, prior ack exists → republish the original ack (`replayed`)
- same uid, different digest → `request_uid_conflict`
- `consume_new` must succeed *before* any ack is written; failure sets `state.failed`

## What an unknown action costs

It is not dropped. `validate_and_authenticate` returns `nil, {code = 'unknown_action'}` with **no
digest**, because the failure precedes HMAC computation. `consumer.process_request` still calls
`consume_new` — burning the uid permanently — and writes a typed failure ack using
`digest or string.rep('0', 64)`.

A 64-zero digest in an ack is the tell that the rejection fired before signing was attempted. No
mux state is touched: `presentation.dispatch` is never reached.
