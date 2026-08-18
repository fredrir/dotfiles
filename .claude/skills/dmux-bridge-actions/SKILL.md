---
name: dmux-bridge-actions
description: >
  Extending and maintaining the dmux GUI bridge in shared/wezterm/wez/dmux_bridge/** — the signed
  request protocol, its action allowlists, origin kinds and token verification, pane-marker
  correlation, acknowledgement construction, and the keybinding verbs that call the dmux CLI. Use
  when adding or changing a bridge action, a protocol field, a target schema, a keybinding that
  drives dmux, or GUI-local tab/pane focusing, and when debugging why a request is refused with
  unknown_action, malformed_request, unauthorized, invalid_origin, ambiguous_group, or a latched
  bridge. Also fires on "add a bridge action", "why is the bridge rejecting this", "the HMAC
  doesn't match", "make Command+W do X", or "focus the right pane after attach". For general Lua
  style, tests, and static checks use wezterm-lua-config; for the owner mux server, sentinel, and
  cold recovery use dmux-mux-lifecycle.
compatibility: >
  Needs a Lua 5.4-compatible interpreter on PATH to run the bridge test suite. Protocol changes
  must be matched on the Rust signing side before they take effect.
metadata:
  version: "1"
---

# dmux bridge actions

Paths are relative to the repo root; the subsystem is `shared/wezterm/wez/dmux_bridge/`.

The bridge is a fail-closed security boundary, not a feature module. Its whole job is to refuse
anything it cannot prove. Read `docs/adr/dmux/003-gui-attach-activate-bridge.md` and
`docs/dmux-wezterm-first-plan.md` §13.2 before changing the protocol surface — they are the
normative contract this code implements.

## Two planes, opposite directions

Conflating these is the easiest way to get this wrong. They share no vocabulary.

| | Outbound (GUI → owner) | Inbound (owner → GUI) — *this is "a bridge action"* |
|---|---|---|
| Entry | `actions.lua` keybinding → `controller.run(window, pane, verb, args)` | the poll loop in `consumer.lua` → `state.bridge:next_request()` |
| Names | **verbs**, mostly kebab-case: `group-new`, `split-remove`, `safe-quit`, `context` (12 in all) | snake_case **actions** in `protocol.M.ACTIONS`: `present`, `focus_pane`, `attach_domain` |
| Transport | `wezterm.run_child_process { dmux, '_gui', '--origin-json', …, verb }` | signed JSON read through the fork lease |
| Validated by | a file-local `decode_response` in `controller.lua` | `protocol.validate_and_authenticate` |
| Spec | plan §13.3 | plan §13.2 |

Plan sections cite `docs/dmux-wezterm-first-plan.md`. The poll loop and `decode_response` are
file-locals, not module members — you cannot call or unit-test them from outside their file.

`safe_quit` and `safe-quit` are different objects. `present`, `focus_pane`, `activate`, `toast`,
`ping` have no `actions.lua` counterpart; `group-new`, `split-new`, `group-rename` have no protocol
counterpart.

The Lua layer does **not** branch on backend. It validates the pane marker, serializes it into
`--origin-json`, and the Rust `dmux _gui <verb>` binary owns the per-backend dispatch from §13.3.
Don't add `if backend == 'wez'` logic here.

## Adding an inbound bridge action

Do these in order. Steps 1–6 are all in `protocol.lua`; skipping any of them fails in a different
way, and step 9 fails *silently until a crash*.

1. Add the name to `M.ACTIONS`. Skip → `unknown_action`, "action is not allowed by bridge v1".
2. Add an entry to `TARGET_KEYS_BY_ACTION`. **Skip → silent over-permission**: the lookup is
   `TARGET_KEYS_BY_ACTION[action] or TARGET_KEYS`, so your action inherits the union of every
   target key. This is the one allowlist in the subsystem that fails *open*.
3. Add any new field name to `TARGET_KEYS` too, or the fallback table drifts out of sync.
4. Add a branch in `validate_target` before its trailing `return nil, 'unsupported action'`.
   Skip → `malformed_request` on every call.
5. If the action needs a live pane, add it to `PANE_ACTIONS`. Skip → a `cold_launcher` origin can
   drive it with no pane at all.
6. If it is cold-launchable, extend the target≡origin binding so the launcher can only act on its
   own exact target.
7. Implement `local function <name>(target, state, deadline, authorize, done)` in
   `presentation.lua`.
8. Add an `elseif` in `M.dispatch`. Skip → `unknown_action`, "action is not implemented".
9. **Add every field your result returns to `ACK_KEYS` in `consumer.lua`.** See gotchas — this one
   passes all happy-path tests and then latches the bridge permanently dead.
10. Extend `correlation.lua` only if new GUI-local matching is needed.
11. Add protocol/correlation assertions to `tests/run.lua` and dispatch behaviour to
    `tests/presentation.lua`.

The Rust side must learn to sign the action; Lua alone changes nothing.

Then validate:

```bash
sh shared/wezterm/wez/dmux_bridge/tests/suite.sh
```

## Refusing correctly

Typed failures are `{ code = <string>, message = <string> }` returned as a value — `return nil, err`
— never raised. Codes are lowercase snake nouns (`not_found`, `ambiguous_group`,
`wrong_backend_instance`). Messages are lowercase fragments with no trailing period, naming the
invariant that failed.

There is no shared constructor; each module has its own short local, and the names differ:
`failure` in `protocol.lua` (returns *three* values — the third is the digest), `failure` in
`correlation.lua`, `invalid` in `context.lua`, and `fail(done, code, message)` in
`presentation.lua`, which calls back rather than returning. Match the one in the file you're
editing.

Raising is reserved for two cases: config evaluation in `init.lua`, where an `error` correctly
aborts the whole config, and genuine programmer error such as `canonical.array` being handed a
non-table. A runtime protocol refusal is a returned value, never a raise.

## Gotchas

- **`ACK_KEYS` in `consumer.lua` is a second, easily-missed allowlist.** A new result field encodes
  fine and the first ack writes fine, but `exact_ack` re-reads that ack on any crash-recovery path,
  rejects the unknown key, and routes to `fatal_corruption` → `state.failed = true`. That latch is
  never cleared: the poller keeps running and the heartbeat keeps succeeding, so the outside world
  sees a healthy GUI while every request dies on the Rust ack timeout.
- **`authorize` is a closure to re-invoke, not a boolean to cache.** `dispatch` calls it once, then
  passes the *function* down; every executor calls it again in the same callback as each side
  effect. HMAC authenticates the request producer, not mutable GUI pane state.
- **The execution deadline is capped at 4 seconds** (`math.min(request.expiry, os.time() + 4)`)
  regardless of the signed TTL, which itself maxes at 10. Any polling action must finish inside
  that. `wait_until` checks the deadline *before* the predicate, so a state change observed after
  expiry can never turn an expired request into an ack.
- **`after_ack` is a reserved result key.** Returning `{ platform_action = 'hide', after_ack = fn }`
  strips `after_ack` from the ack body and runs the closure only after the ack is durably written.
  Getting this backwards means a quit that races its own acknowledgement.
- **Build empty lists with `json.array()`/`canonical.array()`.** An unmarked empty Lua table
  canonicalizes to `{}`, not `[]`, and the HMAC diverges from Rust. WezTerm's own JSON decoder is
  deliberately unused for exactly this reason.
- **Signature is verified before freshness**, so an expired-but-valid request still gets a
  client-verifiable typed ack instead of looking like bridge corruption. Don't reorder those.
- **A pane marker is a locator hint, never authorization.** Rust revalidates every GUI-originated
  operation against the owner registry and a live provider scan. `nonce` and `replay_key` are
  entropy *inside* the signed document, not credentials — only `crypto.constant_time_equal` over
  `canonical.signing_document` authorizes.
- **The key and identity come only from the fork lease** (`instance.require_secure_surface`), never
  from `wezterm.GLOBAL`, an env var, or a filesystem path. `wezterm.GLOBAL.dmux_bridge_instance`
  holds an ID that is itself re-verified against the lease.
- **`mux.all_domains()` cannot be walked naively.** Every `ClientDomain::attach()` leaks a
  `TermWizTerminalDomain` that reports `Attached` forever and refuses `detach()`. Call
  `inventory.routable` *before* the duplicate-name check, and drop a skipped domain's panes along
  with it — otherwise the unknown-domain refusal latches the bridge.
- **`present` retries exactly one marker shape** by string-matching `'pane marker is missing dmux_'`,
  because a newly imported pane can be visible one event-loop turn before its `SetUserVar` snapshot
  arrives. Changing that message text in `context.lua` silently breaks the retry.
- **`resident_gui` exists because `dmux-managed-application-quit-requested` fires with zero
  arguments** — no window, no pane, so no marker. `controller.run_resident` hard-refuses any verb
  other than `safe-quit`; don't generalize it.
- `AttachDomain`, `SwitchToWorkspace`, `mux.kill_window`, and `QuitApplication` are all deliberately
  unused. `SwitchToWorkspace` may *create* a workspace, which violates con-never-creates;
  `domain:attach()` is the selected no-spawn primitive.
- `window_decorations = 'RESIZE'` in `init.lua` is a security control, not cosmetics: the native
  close control routes straight to `mux.kill_window` with no Lua interception hook, so managed mode
  removes the control entirely.
- `init.preflight` runs twice — once from `wezterm.lua` before all modules, once inside `apply`.
  Preflight clears `config.keys` by design and `apply` restores the sanitized products immediately
  after. Inserting code between those two lines breaks the config.
- `publish_persistent_domains` must stay last in `apply`. It sets the module-local authority
  `instance.create` reads at `gui-startup`; skip it and `consumer.start()` dies with "managed
  persistent domain inventory is unavailable", with no obvious link to config evaluation.
- A guard that fired correctly is not a defect. Use `controller.toast` for it, not
  `controller.report` — "a line that reads like a bug every time a safety check succeeds is how a
  log stops being read."

## Reference files

- `references/protocol-validation.md` — the fixed validation order in
  `validate_and_authenticate`, the three origin kinds and what each may do, and the key-allowlist
  tables. Read when adding or changing an action, a target field, or an origin rule.
- `references/correlation-rules.md` — how `DMUX_GROUP_REF`/`DMUX_SPLIT_REF` map to GUI-local tabs
  and panes, with the exact zero/one/many disposition for every level. Read when an action targets
  a Group or Split, or when debugging `not_found`/`ambiguous_*`.
