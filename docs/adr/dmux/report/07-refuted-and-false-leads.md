# Refuted findings and false leads

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`.

---

**The known-wrong prior-list item is `space_cli.rs:221-233`.** It is listed under *"literal
`expected_epoch: None` against a managed instance"*, and that characterisation is false on the
mechanism: `grep -rn 'expected_epoch: None' src/` returns exactly four code literals —
`space_cli.rs:1162`, `ls_cli.rs:846`, `main.rs:1453`, `remote/agent.rs:1281` — and this is not among
them. It is tuple destructuring, `Some(socket) => (socket, None)`, taken only when the operator
passes `--socket`, declared `#[arg(long, hide = true)] /// Test seam: exact wez service socket`
(`space_cli.rs:148-150`) and used by `tests/normalize_flow.rs:266`. No registry instance is resolved
on that branch; the default branch calls `verified_wez_target` and yields `Some(epoch)`. **But
"wrong item" ≠ "clean":** `normalize_plan` derives `plan.server_epoch` from
`verified_scan(scope, None)` (`wez.rs:2590`) and `normalize_apply` then pins to
`Some(plan.server_epoch)` (`wez.rs:2611`) — the apply is pinned to the epoch the unpinned plan
observed, self-consistent and entirely unverified. Separately, `repair_scan_wez` calls
`register_backend_instance` (`operations.rs:3501`), which returns the existing `(owner, backend)`
row and ignores the socket (`registry/mod.rs:1512-1522`), so the seam fences instance A while
scanning and healing endpoint B. Both are real, both are low, neither is this class. Two other
list *labels* are also wrong: `remote/agent.rs:1281` and `main.rs:1453` are literal `None`s, not
laundered Options — worse, not better, because they fire even when publication is healthy.

**Killed by adversarial verification — do not resurrect:**

- **`backend/tmux.rs:1502` — "tmux inventory has no sentinel-equivalent floor".** REFUTED. The
  behaviour is the frozen spec: plan §11.2 L640 requires `ls` to list an unepoched tmux server as
  `unmanaged:unepoched` and write nothing; it is implemented at `inventory.rs:218` /
  `output.rs:297`, and `tmux.rs:2296` (`inventory_unepoched_server_reports_none_and_never_writes`)
  passes. The wez contrast is also false: the sentinel is equally server-self-reported (`wez.rs:1082`
  is a `starts_with` on a workspace name any client can mint with `wezterm cli spawn --workspace`),
  and the one floor wez genuinely has — the SO_PEERCRED probe at `wez.rs:680` — is *inert* on the
  compared path, because `ls_cli.rs:957` builds `WezProvider::new` with no `with_identity`. The
  doc-comment charge is a misquote: `tmux.rs:412` reads "the only writer of the option **in this
  module**" and `:408-411` explicitly reasons about an external racer.
- **`inventory.rs:172` — "reconcile never cross-checks the observed epoch against the binding".**
  REFUTED, and its stated evidence is factually false. `BindingRow` (`registry/mod.rs:816-823`) has
  no `server_epoch`; `BINDING_COLUMNS` (`:2812`) is six columns and does not include it; the cited
  `registry/mod.rs:1711` is `pane_stamps`, not a binding query. `reconcile` receives no registry
  epoch at all, so the comparison is not omitted there, it is impossible there. (The true kernel —
  the column is write-only — survives as finding #18, filed at its real site.)
- **`ls_cli.rs:1096` — "hierarchy probes after every fence is released".** REFUTED. The finder read
  the caller and never opened the callee: `operations::hierarchy` re-acquires the fence itself at
  `operations.rs:3076-3087` (`try_acquire(BackendInstance, Shared)`, returning
  `"backend instance {} is recovering or mutating"` on failure) and holds it across the probe at
  `:3089`. Recovery's exclusive hold (`recovery.rs:1814`) does conflict, so a half-restored tree
  cannot be read. The scope is also epoch-pinned via `ManagedScope::scope()`.
- **`operations.rs:2223` — "the operation layer's fence is `if let Some(expected)` too".** The
  *site* is a real defect but the *mechanism* is wrong: that guard is unreachable in production,
  because both adapters refuse a `Some`-mismatch before returning `Complete` and `scan_space_row`
  maps every non-`Complete` to `Indeterminate`. Re-filed as finding #10 at its real line, `:2243`.
- **`gui_lifecycle.rs:972` as a live fault on this host.** REFUTED. The descriptor is
  `state:"starting"`, so `require_ready` (`runtime.rs:298`) returns `WouldBlock` at
  `gui_lifecycle.rs:565` and `validate_ready_descriptor` is never reached. The *unachievable-remedy*
  criticism of its message text stands; the claim that the code fires does not.
- **`new_cli.rs:377` "can spawn a workspace on an unverified server".** DISPROVED for wez.
  `NewPlan::Create{Wez}` requires `wez_service_compatible`, and `gui_cli.rs:1286-1292` returns
  `false` exactly when `server_epoch` is `None`. The site is still defective — the reads decide
  Connect/Blocked and registry rows are written — but no unverified wez spawn is reachable from it.
- **`backend/wez.rs:3919` (`group_new_verifies_parent_and_new_tab`) as an unpinned mutation.**
  Not one. Despite `scope(None)`, `group_new` pins via `binding_epoch` (`wez.rs:1180`), which falls
  back to the binding's epoch. Same for `remove` (2163), `inspect` (2209), `rename`, `group_list`.

**Confirmed clean, each traced to its producer rather than pattern-matched:**
`connect_cli.rs:1167`, `rm_cli.rs:1115`, `gui_lifecycle.rs:1007` (reported as ~964; 493e92c moved
it), `main.rs:1463`, `gui_cli.rs:1392`, `space_cli.rs:646`, `space_cli.rs:1174`,
`remote/agent.rs:884`, `remote/agent.rs:1317`, `ls_cli.rs:749`, `ls_cli.rs:846`. Also clean and
worth naming so they are not re-reported: `runtime.rs:634/645/666`'s `Option<Uuid>` (all six
production callers pass `Some` after an `ok_or_else`), `registry/recovery.rs:172`'s Option (widens a
query, the safe direction), `rm_cli.rs:1240`'s `is_some_and` (the documented replay-shortcut rule),
`gui.rs:3140`'s `expected_epoch: &str` (non-optional, marker-internal), and the
`GroupActivationResult`/`SplitDirectionResult`/`SplitResizeResult`/`SplitZoomResult` witnesses,
which *are* compared (`operations.rs:2827`, `:2897`, and equivalents).

**Both 493e92c fixes verified, not trusted.** `ManagedScope.epoch` is a non-`Option` `ServerEpoch`
(`ls_cli.rs:741`), `scan_target` diverts a NULL to `ScanTarget::Unpublished` (`:857-862`), and the
`Unpublished` arm returns `Unreachable` without probing (`:940-942`). `cargo test -p dmux --test
ls_cli` → 31 passed, including `a_managed_instance_without_a_published_epoch_refuses_to_scan`, which
asserts exit Partial, `backend_epoch_changed`, and `!scratch.ran_wezterm()`. Independently
reproduced end-to-end: NULL epoch + stranger stub → exit 7, `observation:"unreachable"`, the stub
`wezterm` never spawned. `gui_lifecycle.rs:972` does return `fatal(..)` while the live-scan mismatch
at `:1013` stays `Retry`. Both claimed follow-on defects — the `Unpublished` fence collapse and the
orphaned doc comment — are confirmed ([the fence/state analysis](04-fence-and-instance-states.md), [Ranked findings](01-findings.md) row 22).

---

## Post-review verdicts (ADR 012 WS-E.2, 2026-08-23)

The three out-of-crate findings listed in [08 §7](08-untested.md) as call-chain-only were each made
executable by the QA role and found to be **non-defects**; the evidence is in the tree:

- **`shared/zsh/conf.d/91-tmux-attach.zsh:45`** — `_dmux_wrap` expands a lone bare non-flag word that
  is not in `_dmux_verbs` to `dmux --host H new NAME` and forwards everything else verbatim; it
  carries no backend flag, no `DMUX_WEZ_FIRST`/`DMUX_LEGACY_POLICY`, no `--socket`/`--epoch`/
  `--data-dir`/`--lock-dir`/`--namespace`, and no default. Every epoch decision is the crate's,
  reached identically through the wrapper or directly (`tests/cases/dmux-wrappers.sh`, 10 cases).
  ADR 010 §4 upheld; `repair` is already in the allowlist, so `repair retire-incarnation` and
  `repair rebind` need no wrapper change.
- **`shared/zsh/conf.d/94-dmux-context.zsh:216`** — the prompt hook is a pure carrier: §13.1 puts
  epoch verification in the crate (WS-A.7, `tests/context_cli.rs`). The hook exports and emits only
  what one validated `dmux _context` response for the requested Space contains, refuses child refs
  whose epoch differs from the response's `server_epoch`, takes a rotated epoch only from that
  response, and retires the prior epoch on a controller refusal (`tests/cases/dmux-context.sh`,
  three cases).
- **`shared/wezterm/wez/dmux_bridge/controller.lua:115`** — `invoke`/`argv` build
  `_gui --origin-json <marker> <verb>` with the marker byte for byte and no seam argument; the
  controller consults only `DMUX_BIN`, runs no child on an unparseable marker or an unready bridge,
  and surfaces the crate's typed `backend_epoch_changed` unchanged; the GUI-origin marker is
  revalidated crate-side by `operations::validate_marker_context` via `gui_cli::validate_local_marker`
  (Lua `controller` case).

Also refuted on this pass: [06](06-unreachable-code.md) row 11's claim that the live
`HeartbeatSource::live_instances` lacks a freshness check — at 493e92c `gui_lifecycle.rs:652`
reaches `validate_heartbeat`, which enforces `HEARTBEAT_MAX_AGE`; the dead helper was deleted and
the stale-heartbeat refusal is proven on the production reader (GUI close, ADR 012 §10). And
`bare()`'s `wezterm cli spawn` in the legacy `attach.rs` is a deliberate non-pin (it carries no pane
id; spawning on the managed server would create an unmanaged pane) — WS-C.

