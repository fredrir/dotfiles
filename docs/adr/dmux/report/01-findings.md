# Ranked findings

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`. Answers review-ask #2 (prove reachability, not just presence).

---

Ranked by severity, writes above reads at equal severity. "proven-by-execution" means a repro
agent ran it and captured output; "proven-by-call-chain" means every hop was read but nothing ran.
Findings marked **2/2** or **1/1** survived adversarial verification at that vote count; three
findings were killed by verification and appear only in [Refuted findings and false leads](07-refuted-and-false-leads.md).

| # | site (file:line) | sev | r/w | reachability | evidence |
|---|---|---|---|---|---|
| 1 | `src/registry/mod.rs:1551` | critical | writes-registry | proven-by-execution (**live on this host**) | sole writer of `server_epoch`; no clear, no liveness re-check |
| 2 | `src/adopt_cli.rs:238` | critical | writes-native+registry | proven-by-execution | NULL epoch → CAS rename lands, exit 0, binding written |
| 3 | `src/migrate_cli.rs:743` | critical | writes-native+registry | proven-by-execution | `adopted:2, committed:true` against a foreign mux |
| 4 | `src/space_cli.rs:1162` | critical | writes-native | proven-by-execution | literal `None` for managed tmux; `group_new` mutated an impostor |
| 5 | `src/backend/tmux.rs:474` | critical | writes-native | proven-by-execution | `binding_epoch` returns the binding's own epoch on `None` |
| 6 | `src/backend/wez.rs:2049` (+8) | critical | writes-native | proven-by-execution | 9 wez verbs use `verified_scan(scope, None)`; unit tests pass with `scope(None)` |
| 7 | `src/space_cli.rs:631` | high | writes-registry | proven-by-execution | `.ok().and_then(..)` → `abort_create` on an unepoched server's word |
| 8 | `src/main.rs:1453` | high | writes-pane-markers | proven-by-execution | ambient `$TMUX` endpoint **and** no epoch; marker minted 4 ways |
| 9 | `src/list.rs:151`, `src/attach.rs:99` | high | read → native activate | proven-by-execution | default path; ADR-001 rules 1-3 all violated; sentinel listed as row 1 |
| 10 | `src/operations.rs:2243` | high | writes-registry | proven-by-call-chain | journals the unverified scan epoch before any mutation |
| 11 | `src/operations.rs:133` | high | writes-registry | proven-by-call-chain | tmux `socket_dev`/`socket_ino` hard-coded `None` |
| 12 | `src/new_cli.rs:362` | high | read → registry writes | proven-by-execution | `Blocked`/`Connect` decided from an unverified scan |
| 13 | `src/remote/agent.rs:1281` | high | read | proven-by-execution | `spaces` RPC: `complete` from a replaced tmux server |
| 14 | `src/doctor.rs:231` | high | read | proven-by-execution | the named remedy cannot observe epochs at all |
| 15 | `src/gui_cli.rs:1430` | medium | read | proven-by-execution | opposite-backend collision fence, unverified |
| 16 | `src/rm_cli.rs:1138` | medium | read | proven-by-execution | `--row` tail advertises a stranger as adoptable |
| 17 | `src/remote/agent.rs:1417` | medium | read | proven-by-execution | `new_lookup` answers "name free" from an unverified scan |
| 18 | `src/registry/mod.rs:2812` | medium | writes-registry | proven-by-call-chain | `native_bindings.server_epoch` INSERTed, never SELECTed |
| 19 | `src/ls_cli.rs:781` | medium | read | proven-by-execution | `Unpublished` never fenced → destructive operator advice |
| 20 | `src/remote/agent.rs:1405` | low | none (latent) | proven-by-call-chain | fabricates `ServerEpoch(Uuid::nil())` |
| 21 | `src/space_cli.rs:222` | low | writes-native | proven-by-call-chain | `--socket` seam; apply pins to the unpinned plan's own epoch |
| 22 | `src/ls_cli.rs:1187` | low | none | proven-by-call-chain | `scan_error_code`'s doc orphaned onto `unpublished_detail` |

**1 — `registry/mod.rs:1551`, the stale-incarnation hole (live).**
`UPDATE backend_instances SET server_epoch = ?2, server_pid = ?3, …` is the only write of that
column anywhere; `grep -rn "server_epoch = NULL" src/` returns nothing and there is no
`DELETE FROM backend_instances`. I confirmed the live divergence read-only: registry
`40c99029-…/pid 5458/dev,ino 16777231,10519741`; `ps -p 5458` empty; live mux pid 54528 on
`dev,ino 16777233,14788383` serving one workspace `dmux:system:895ca35a-…`, and `895ca35a`
occurs 0 times in a `.dump` of the registry copy. The doc at `registry/mod.rs:1585-1586` calls a
NULL epoch "stopped or never published" — a state the code can never re-enter — which is exactly
the reading that made every reviewer treat a published epoch as proof of a live server.

**2 — `adopt_cli.rs:238`.** `owner_scope` `ok_or_else`-refuses a missing instance (222-230) and a
missing endpoint (231-234), so only the epoch is allowed to be absent; that is how managed is told
from unmanaged here — there is no discovery path into this function. Repro (scratch registry,
in-memory fake mux): with `server_epoch = NULL`, `dmux adopt native:wez:…` exits **0**, executes one
`rename-workspace --if-workspace … --if-sole-window …`, and writes a binding whose `server_epoch`
is the stranger's. With a published epoch the identical run refuses `provider_unavailable` and
mutates nothing. The mutation is not fenced because `cas_rename_workspace` (`wez.rs:2814`) opens
with `verified_scan(scope, None)` and never calls `required_action_epoch`.

**3 — `migrate_cli.rs:743`.** The `Unregistered` (731) and `Unaddressable` (736) arms peel off the
genuinely-unmanaged cases, so a `Target::Managed` is a registered, addressable instance by
construction. Repro with the real `SystemRunner`: `dmux migrate` previewed `disposition:"adopt"` for
two foreign workspaces, `--commit --yes` returned `{"adopted":2,"recorded":true,"committed":true}`,
renamed both on the foreign server, and wrote `migrated-v1.json` — after which every later migrate
is a permanent no-op (`migrate_cli.rs:422-423`). In the same process, on the same registry, `ls`
**refused** with `backend_epoch_changed`. That single contrast is the whole review in one line.

**4 — `space_cli.rs:1162`.** `row` is an Active Space (1129-1143) and the instance is an FK
dereference (`backend_instance_info`, 1146-1148); the Wez arm twelve lines down calls
`verified_wez_target` and pins `Some(epoch)` (1167/1174). Proven live at the provider layer:
`group_new` under an unpinned scope created window `@2` on an impostor tmux server, while
`split_list` on the identical scope refused. Note the unconditional bug underneath: because
`operations::group_new` calls `split_list` immediately after (`operations.rs:2325`), **every**
`dmux group new` on tmux mutates and then aborts, leaking an orphan window and a live
`pane-bootstrap` process — no divergence required.

**5 — `backend/tmux.rs:474`.** `binding_epoch` returns `Ok(binding.server_epoch)` unchecked when the
scope carries `None`, and `operations.rs:2277` sets that field from the epoch the unverified scan
just observed, so the subsequent `check_epoch` compares the server against its own self-report. Six
tmux verbs route through it (1686, 1729, 1765, 1789, 1805, 2054) versus thirteen through
`required_epoch`. This directly falsifies "mutations DO require an epoch (tmux.rs:466)".

**6 — `backend/wez.rs:2049` and eight siblings.** `required_action_epoch` (1265) is called from
exactly four sites — 1359, 1405, 1522, 1581, the P9 exact-child actions. `create` (2049),
`group_rename` (2297), `group_remove` (2359), `split_list` (2417), `split_new` (2445),
`split_remove` (2539), `normalize_plan` (2590), `sole_window_id` (2749) and `cas_rename_workspace`
(2814) all call `verified_scan(scope, None)`, whose only pin is the skipped `if let Some(expected)`
at `wez.rs:1113`. The crate's own tests drive five of them with `scope(None)` and assert the native
spawn/kill/rename happens; `cargo test -p dmux --lib backend::wez::tests::split_new_lands_in_same_tab`
passes today.

**7 — `space_cli.rs:631`.** The only epoch read in the crate that uses `.ok()`, so a
`RegistryError` — busy, IO, NotFound — becomes "no epoch, skip verification" indistinguishably from
a NULL. Executed against a scratch registry + an unepoched foreign tmux server: outcome
`reservation_released`, and the reserved Space durably flipped to `lifecycle=Aborted` via
`registry.abort_create` (`operations.rs:4098`). With `Some(epoch)` supplied, the identical run
returned `failed_closed` and left the row `Reserved`. The in-code comment at 626-630 defends the
`None` on the grounds that mutations fail closed — true, and irrelevant, because the consumers here
are registry writes driven by the read.

**8 — `main.rs:1453`.** Worse than a missing epoch: the endpoint comes from ambient `$TMUX`
(`namespace_from_tmux_env`) and is never compared to the instance's recorded `socket_path`, while
`context_read` (`operations.rs:3226`) *does* resolve the instance and take its fence
(`operations.rs:3250`). Reproduced four ways — stranger endpoint, stale live epoch, registry NULL,
and a rebind — each minting `server_epoch`/`group_ref`/`split_ref` from whatever answered, exit 0.
The one-line fix already exists 50 lines earlier in the same file: `validate_marker_context`
(`operations.rs:3174-3181`) does `published.server_epoch != Some(live_epoch)`.

**9 — `list.rs:151` / `attach.rs:99`.** `WEZ_FIRST_BY_DEFAULT = false` (`main.rs:1724`), so this is
the path that runs on the user's machine today, and nobody in the review opened either file.
`Command::new("wezterm").args(["cli","--no-auto-start","list","--format","json"])` — no
`WEZTERM_UNIX_SOCKET`, no `--config-file`, no deadline, no sentinel filter, and
`grep -cE 'Registry|registry' src/list.rs src/attach.rs` is **0**. Executed read-only with the
socket pinned: `dmux ls --json` returned the reserved sentinel
`dmux:system:895ca35a-…` as row 1, an ordinary attachable target that `attach_wez` will
`activate-pane` into.

**10 — `operations.rs:2243`.** This is the corrected form of the reported "`operations.rs:2223` is
`if let Some(expected)` too". That guard is provably unreachable in production — both adapters
refuse a `Some`-mismatch before returning `Complete` (`wez.rs:1113`, `tmux.rs:1521`) and
`scan_space_row` maps every non-`Complete` outcome to `Indeterminate` (`operations.rs:2022`). The
surviving defect is the durable write two statements later: `bootstrap_issue { server_epoch: epoch }`
commits the *scan-observed* epoch into `bootstrap_requests`, where the column is non-`Option`, so an
unverified value becomes indistinguishable from a verified one to all 25 downstream readers.

**11 — `operations.rs:133`.** The only production tmux publisher passes literal `None, None` for
`socket_dev`/`socket_ino`. All five dev/ino comparison sites are wez-only, so the stat-based
replacement witness is structurally unreachable for the backend ADR 002:64-73 says is easiest to
spoof. The values are already in hand and thrown away: `tmux.rs:392-397` stats the socket and folds
dev/ino into a `start_token` string.

**12 — `new_cli.rs:362`.** Reproduced at library level with a real scratch tmux server: with the
epoch laundered, `lookup_new_owner_fenced` returned `Blocking{UnmanagedSameName}` off a stranger's
sessions where the epoch-pinned control returned `Indeterminate`; a replaced server that recycled
`$1` produced `Connect{space, SpaceNo(1)}` for a dead Space. The write half reserves a Space and a
bootstrap row before the provider refuses (tmux) — SpaceNo burned, row left `Aborted`. The `create`
half of the enumerator's claim is **disproved**: `NewPlan::Create{Wez}` requires
`wez_service_compatible`, and `gui_cli.rs:1286-1292` returns `false` precisely when the epoch is
`None`.

**13 — `remote/agent.rs:1281`.** A literal `None`, not a laundered Option — the registry epoch is
never read, so this fires even when publication is healthy. Proven live: after replacing the server
on a scratch `-L` namespace, `_agent spaces` returned `{"outcome":"complete","rows":2}` while `new`
on the identical rig at the identical instant returned `backend_epoch_changed`. The Wez arm 36 lines
below routes through `ready_wez_identity` and pins `Some(epoch)`. Downstream,
`ls_cli.rs:1144-1148` maps `"complete"` to `Observation::Live` without ever reading
`scan.server_epoch`.

**14 — `doctor.rs:231`.** `grep -cE 'descriptor|backend_instance|backend_server|server_epoch|Registry' src/doctor.rs`
is **0**; `registry_detail()` self-documents that doctor "never opens the registry". Both
operator-facing epoch remedies in the crate (`gui_lifecycle.rs:978`, `ls_cli.rs:1197`) end
"then re-run `dmux doctor`". On this host doctor reports green while the registry names a dead pid.
The remedy loop is closed and empty.

**15-17 — `gui_cli.rs:1430`, `rm_cli.rs:1138`, `remote/agent.rs:1417`.** All three refuse the
unregistered and endpoint-less cases first, so a `None` epoch can only be a managed NULL. Each was
reproduced: the gui site's opposite-backend scan waved a create through (`creates == 1`, durable
Space row) where the epoch-pinned control refused `Indeterminate`; `rm --row N` past the managed
prefix reported `repair_required: … dmux adopt native:tmux:JDA` for a stranger's session,
byte-identically to the matching-epoch control; `new_lookup` returned a determinate `no_match`
derived from an unverified server. None reaches a native write — the downstream fences hold — so
these are misinformation and denial, not corruption.

**18 — `registry/mod.rs:2812`.** `BINDING_COLUMNS` is `binding_id, space_uid, native_token,
native_kind, binding_state, observation`; `native_bindings.server_epoch` is INSERTed
(`registry/mod.rs:1908`) and never SELECTed. Because it is unreadable, every `binding_epoch` caller
must be handed a synthesised binding, and `operations.rs:2277` synthesises it from the live scan —
making the nine `binding-REQUIRED` cells of the provider matrix tautologies. One refutation pass
graded this "a hardening item with no failure scenario"; that grading is wrong, because
`binding_epoch` is the *sole* fence for ten verbs.

**19 — `ls_cli.rs:781`.** `ScanTarget::instance()` returns `None` for `Unpublished`, so it never
enters `fenced` (911-925) and the scan arm (940-942) cannot consult it. A/B with an identical held
exclusive lock, changing only the epoch, produced: NULL → *"restart the managed mux service"*;
published → *"backend instance is recovering or mutating"*. `tmux_bootstrap` registers at
`operations.rs:88` and publishes at `:129` with the exclusive lock taken in between, so the
destructive advice is emitted exactly during a first bootstrap. Correction to the reported claim:
this does **not** bypass a safety fence — `Unpublished` is never probed — and a journaled restore
cannot be in flight, because `begin_recovery` requires a published epoch
(`registry/recovery.rs:625`). The defect is the misclassification and the remedy text.

**20-22.** `agent.rs:1405` fabricates `ServerEpoch(Uuid::nil())`; it is inert today because
`new_lookup` reads only `.backend`/`.instance`, but `Target.epoch` is consumed at 18 other sites in
the same file. `space_cli.rs:222` is the `--socket` test seam ([Refuted findings and false leads](07-refuted-and-false-leads.md)), whose residual is that
`normalize_apply` pins to `plan.server_epoch` — the epoch the *unpinned* plan observed.
`ls_cli.rs:1187-1189` documents `scan_error_code` (now at 1214) but sits on `unpublished_detail`
(1193); confirmed against the 493e92c diff hunk `@@ -1137,6 +1187,30 @@`.
