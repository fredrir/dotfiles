# ADR 012: P11 completion plan — epoch integrity, live-host repair, and the cutover gate

Status: accepted 2026-08-22 (owner-approved; §7 amendments applied to the plan in the same change); dispatch recorded in §10
Date: 2026-08-22
Owner: root integrator
Inputs: `docs/adr/dmux/report/**` (independent review at `493e92c`), ADR 007 (P0 ratified,
W7 reclamation), ADR 010 (P11 gate amendments), ADR 011 (P11 dispatch), `r5.md`, and a
read-only survey of the working tree, the live Macie registry/runtime, and the test suite.
Plan refs: §7.1, §8.1, §8.2, §10.3, §11, §15.1, §15.3, §17, §18 P11 row, §20.2, §21, §22
Scope: everything required to claim P11 (§22) — and nothing in P12.

---

## 1. Why a new record

ADR 011 dispatched P11 as "wire what P4/P6 shipped, then run the gate". Most of that wiring
landed (`f841cdb`…`ca25073`). The independent review then showed that the gate cannot be run
honestly on the current tree: nine of twenty-three managed-scope construction sites skip
epoch verification, two of them complete native mutations on a server nothing verified, and
the registry row that every reader treats as authoritative names a process that is dead on
this host. None of that is polish. §22 requires "exact-existing-first behavior and
no-silent-backend-fallback … proven by fault tests" and "the owner registry is the only
Space-ID authority"; cases 13, 25, 27 and 45 assert the properties the review reproduced
failing. So the remaining P11 work is larger than ADR 011 recorded, and it has a different
shape: remediation first, then the live hosts, then the gate.

This ADR records (a) where P11 verifiably stands today, (b) the findings beyond the report
that change the plan, (c) the ordered workstreams to finish, (d) the plan amendments they
require, and (e) what is explicitly left for P12.

## 2. Where P11 stands — verified, not inherited

Every row below was checked against the working tree or the live host on 2026-08-22, read-only.

| §18 P11 / §21 item | State | Evidence |
| --- | --- | --- |
| Wez-first CLI surface wired (`ls --tree/--all-hosts/--row`, `adopt`, `rm`/`rename` JSON, `--format` envelope) | **done**, gated | `main.rs` gate strings at 724–907, 2059; ADR 011 W1–W2 |
| `dmux migrate` (§17, case 45 driver) | **done**, never run | `migrate_cli.rs` (1453 lines, `ca25073`); no `migrated-v1.json` under `~/.local/share/dmux/` on Macie |
| `repair reconcile` (ADR 011 D8) | **done** | `946e924`, `f1ef3c4` |
| `repair rebind` (§7.1, §10.3; ADR 011 D8 says "no longer optional polish") | **absent** | `grep -n rebind src/main.rs` → nothing; only registry/model references exist |
| Emergency opt-out `DMUX_LEGACY_POLICY=1`, three-valued `DMUX_WEZ_FIRST` | **done** | `main.rs:1715–1756`, tests at 2228+ |
| Harness migration for the flip (ADR 011 D1) | **done** | `tests/cli.rs` exports `DMUX_LEGACY_POLICY=1` (`ca25073`) |
| Wrapper allowlist mechanically verified (ADR 010 §4) | **done** | `91-tmux-attach.zsh`, `the_wrapper_verb_allowlist_matches_the_cli` |
| Docs/completions simplified | **partial, stale** | `docs/scripts.md:1373–1376` still says `migrate` "is not implemented at all" |
| Acceptance cases 1–46 traced to tests/evidence | **no artifact** | only 14 case numbers appear anywhere in `tests/`/`src/` (6, 11, 13, 15, 16, 20, 24, 25, 27, 41–45); nothing maps the other 32 |
| Fresh-context reader test (ADR 010 §3) | **not run** | no question set or answers checked in |
| Two-host live verification (`two_host`) | **not run** | `r5.md`: "deferred to r6"; no r6 release exists; rollout phase is still `mac_deployed` |
| 24–48 h canary on Macie (§21 step 7) | **not started, and not enabled** | `launchctl getenv DMUX_WEZ_FIRST` is empty on Macie today |
| Canary on Archie | **not started** | — |
| Rollback rehearsal, per host (§21 step 7) | **not done** | rollout tool records rollback *inventory* (`workflow.py:857–1017`); no rehearsal checkpoint exists |
| Migration run and mapping inspected on both hosts (§21 step 2) | **not done** | see `migrate` row |
| Global flip (§21 step 9, both halves per ADR 010 §5) | **not done** | `WEZ_FIRST_BY_DEFAULT = false` at `main.rs:1724` |
| Test suite | **green** | `cargo test -p dmux -- --test-threads=1` → 984 passed, 0 failed, 1 ignored (re-run today; identical to the report's baseline) |

## 3. Findings beyond the report that change the plan

### 3.1 Both hosts are in instance state F, and Macie's cause is environmental

The report proved Macie's registry publishes epoch `40c99029…`/pid 5458 (dead) while the live
mux (pid 54528) serves `895ca35a…`. The cause is now established:

- Macie rebooted 2026-08-19 10:57 local (`kern.boottime`). `launchctl setenv` does not
  survive a reboot, and r5's deployment set the canary flag exactly that way
  (`dmux_rollout/workflow.py:889`). After the reboot the flag was gone.
- The mux was (re)started flag-off at 12:42 local (`start_token macos:1787136165`,
  `.wez-dmux-service.lease` mtime 12:42). The descriptor at `$TMPDIR/dmux/wez-dmux.json`
  reads `state:"starting"`, `backend_instance_uid:null`, `written_at 2026-08-19T10:42:45Z` —
  the flag-off path, which by design never publishes to the registry
  (`dmux-mux-start.sh`: "a disabled rollout must not create registry identity").
- So the registry row is the pre-reboot incarnation and nothing has touched it since. The
  live mux holds **only the sentinel** (`wezterm cli list` shows one workspace,
  `dmux:system:895ca35a…`), which is what makes repairing it safe (§6.2 below).
- `r5.md`'s corrections record the same shape on Archie: mux PID 957 today versus the
  registry's 1347078. Both registries are r5-era on `server_pid`/`server_epoch`.

Two consequences. First, canary enablement is not reboot-durable on macOS (and
`systemctl --user set-environment` is equally runtime-only on Linux), so §21 step 7's
24–48 h window cannot be trusted to stay enabled without a durable mechanism — and the same
mechanism is the second half of step 9 (ADR 010 §5). Second, the descriptor never reached
`failed` although `dmux-mux.lua:1148–1160` should have published it on the missing-instance
path; the report lists this as unknown (08 §9) and it remains so. It needs a diagnosis, not a
guess.

### 3.2 The test suite takes kernel locks in the production runtime directory

`$TMPDIR/dmux` on Macie held 568 `backend_<uuid>.lock` and 180 `decision_*.lock` files before
today's test run and 585/181 after it. Each `backend_` UUID is a distinct scratch instance, so
these are test-minted. The tests isolate the *registry* (`XDG_DATA_HOME` / `--data-dir`) but
at least one lock path resolves `dmux_runtime_dir()` for real. That is not only litter: a test
taking `authority-gate.lock` or a backend lock in the live directory can contend with the live
service and with an operator's concurrent `dmux`, and a CI-less repo has no other guard. The
report's reproductions exported `DMUX_RUNTIME_DIR` and still left 28 locks dated today, so the
seam is not honoured on every path.

### 3.3 No acceptance-case ledger exists

§20.2 makes all 46 cases mandatory P11 gates and §20.1 demands baseline accountability
through `baseline-tests.json`, but nothing maps case → test IDs → live evidence. The gate
cannot be declared passed from a green suite; 32 cases are unreferenced by number. This is a
deliverable, not bookkeeping.

### 3.4 `group new` on tmux leaks a window on every call

Report finding #4's footnote is an unconditional functional bug: `operations::group_new`
mutates (`group_new` under an unpinned scope) and then calls `split_list`, which refuses under
the same scope (`operations.rs:2325`), so every `dmux group new` on tmux creates a window,
aborts, and leaves an orphan window plus a live `pane-bootstrap`. Case 26 cannot pass on tmux
today. It is fixed by the scope work but deserves its own regression test.

### 3.5 Five GUI-side helpers have production analogues with weaker semantics

Report 06 #15: `gui.rs` ships `select_compatible_domain`, `validate_acknowledgement`,
`bind_cli_origin`, `parse_signed_origin_json`, `rotate_bridge_key_if_idle` — each tested, none
called — while production uses different functions. The concrete gap:
`select_compatible_domain` checks rows against the caller's validated identity;
production's `choose_compatible_presentation_row` checks candidates only against each other.
And the bridge-key rotation success path has neither a caller nor a test. P9's gate
("invalid context always fails closed") was claimed against the tested-but-dead versions.

## 4. What the review's findings gate, and why they are P11

| Report finding(s) | Property it breaks | Gate it blocks |
| --- | --- | --- |
| #2 adopt, #3 migrate, #6 nine wez verbs, #4/#5 tmux `group_new`/`binding_epoch` | mutation on an unverified server | case 13, 27, 45; §22 "registry is the only authority"; §2.7 |
| #1 stale incarnation, #11 tmux dev/ino, #18 write-only binding epoch | replaced-server detection | case 25, 27; ADR 001/002 |
| #7, #10, #12 registry writes driven by unverified reads | durable rows minted from a stranger's word | case 7, 11, 13 |
| #8 `_context` marker minted from ambient `$TMUX` | marker schema §13.1 "validated against owner registry and live inventory" | case 28, 31 |
| #9 legacy `list.rs`/`attach.rs` list and activate the sentinel | §15.1 "sentinel … cannot be addressed by public commands" | §21 rollback (legacy path is the rollback target) |
| #13, #17 remote `spaces`/`new_lookup` from an unverified tmux | remote rows rendered `live` | case 24 (`--all-hosts`), 16–22 |
| #14 doctor cannot observe epochs; #19 C/D collapse; unachievable remedies | §15.3 "until `dmux doctor` directs…"; §3 "decision-explaining doctor" | §22; fresh-reader test answers about recovery state |

The review's fix shape (05) is adopted as written: the boundary goes at the **resolver**,
`InventoryScope` moves out of `backend/mod.rs`, the field goes private behind
`managed()`/`unmanaged_endpoint()`, and the provider-entry-point split is **phase 2 / P12**.
The report's measured blast radius (61 edit points, suite stays green at each step) is taken
as the estimate.

## 5. Workstreams

Ownership: per ADR 007's W7 reclamation the root holds every path; editing subagents get a
strict per-file subset per §19.3 and return through the root. Each workstream names its
gate; a workstream is closed only when its gate passes and the full suite is green.

### WS-A — Epoch-integrity remediation in the crate (report 05 steps 1–6, findings 2–22)

Ordered by blast radius, one commit per numbered item unless stated. No flag day.

1. **Move `InventoryScope` to `src/backend/scope.rs`**, `pub use` from `backend`. Zero
   behaviour. (Prerequisite: the adapters are child modules of `backend/mod.rs`, so privacy
   does not bite them there — report 05.)
2. **Private `expected_epoch`** + `InventoryScope::managed(backend, endpoint, ServerEpoch)` /
   `::unmanaged_endpoint(backend, endpoint)` + accessor. 61 edit points; the three
   pass-through `fn scope(Option<..>)` test helpers keep their signatures. Reword the two
   refusal strings asserted at `wez.rs:3800` and `tmux.rs:2498`. Replace the P1 doc comment
   at `backend/mod.rs:119–120` (report next-action 13).
3. **Audit test for the hatch**: a Rust test that scans `src/**` for `unmanaged_endpoint(` and
   holds the list equal to the two legitimate sites (`ls_cli.rs` first-contact tmux arm, the
   `space_cli --socket` test seam), in the style of
   `the_wrapper_verb_allowlist_matches_the_cli`. There is no CI here; the suite is the gate.
4. **Land `backend::scope::resolve_managed(&Registry, Backend) -> ManagedTarget`** with
   `ManagedTarget::{Managed{instance, scope}, Unpublished(uid), Unaddressable(uid),
   Unregistered}` promoted from `ls_cli`. The enum carries no `Option<ServerEpoch>`.
5. **Migrate the nine launder sites, one commit each, highest blast radius first**, each
   stating what `Unpublished` means for that verb:
   `adopt_cli.rs:238` (refuse), `migrate_cli.rs:743` (refuse; the `--commit` path must not
   write the cutover stamp), `space_cli.rs:631` (refuse — and drop the `.ok()` that swallows
   `RegistryError`), `space_cli.rs:1162` (refuse), `remote/agent.rs:1281` (`Unreachable` row;
   also removes the literal `None`), `new_cli.rs:362` (refuse before any reservation),
   `rm_cli.rs:1138` (refuse the `--row` tail's adopt advice), `gui_cli.rs:1430` (refuse),
   `remote/agent.rs:1417` (indeterminate, never "name free"). Delete the seven near-duplicate
   resolvers the report names. Error text from `space_cli.rs:1033`, code mapping from
   `ls_cli.rs:1209` (`BackendEpochChanged`).
6. **Fence the nine unfenced wez verbs** with `required_action_epoch(scope)`: `create`,
   `cas_rename_workspace`, `split_new`, `split_remove`, `group_rename`, `group_remove`,
   `split_list`, `normalize_plan`, `sole_window_id`. Add the wez analogue of
   `create_on_unepoched_server_is_a_typed_error`. The ~24 sentinel-bearing unit tests that
   drive these with `scope(None)` move to a pinned scope; record each changed assertion.
7. **`main.rs:1450` (`_context`) by hand**: add the endpoint-vs-`socket_path` and
   live-epoch-vs-published comparisons, copying `operations::validate_marker_context`
   (`operations.rs:3161–3181`). Four-way regression test from the report's repro (stranger
   endpoint, stale live epoch, registry NULL, rebind).
8. **`binding_epoch` tautology** (findings 5, 18): add `server_epoch` to `BINDING_COLUMNS`,
   make `binding_epoch` compare the registry value, stop synthesising the binding from the
   live scan at `operations.rs:2277`. Both adapters.
9. **tmux socket witness** (finding 11): `tmux_bootstrap` passes the dev/ino it already stats
   at `tmux.rs:392–397` into `publish_backend_server`; tmux readers compare them the way the
   five wez sites do.
10. **`bootstrap_issue` journals only a verified epoch** (finding 10): the `Some`-pinned scan
    outcome is the only one allowed to reach `bootstrap_requests.server_epoch`.
11. **`group_new` on tmux** (§3.4): fix, plus a test that asserts no window is created when the
    post-mutation `split_list` would refuse — the check moves *before* the mutation.
12. **Low findings in one commit**: `agent.rs:1405` stops fabricating `ServerEpoch(nil)`;
    `normalize_apply` pins to the resolver's epoch, not `plan.server_epoch`; orphaned doc at
    `ls_cli.rs:1187` moves to `scan_error_code`; `repair_scan_wez`'s `register_backend_instance`
    endpoint mismatch (report 07) becomes a typed refusal.
13. **`tests/provider_contract.rs:82`** stops teaching `None` = skip: the contract double refuses
    a managed read without a pin, and the harness gains the two missing assertions the report
    names — a managed *read* refuses on `None`, and an `ls` test that registers a tmux instance.

Gate: suite green after every commit; `grep -rn "verified_scan(scope, None)" src/backend/wez.rs`
→ 0; the audit test in A.3 holds; the report's nine executable reproductions re-run and
refuse (the report's "Independent spot-checks" block, inverted).

### WS-B — Stale incarnations, liveness, and the doctor (findings 1, 14, 19; 04 state F)

1. **Model state F.** `resolve_managed` (A.4) gains a liveness check for a published
   incarnation: pid alive *and* start token matches *and* socket dev/ino match a fresh
   `stat`. Failure classifies as `ManagedTarget::StaleIncarnation{uid, published, observed}`
   — a fourth non-`Managed` arm, refused by every mutation, rendered by `ls` as
   `unreachable` with a `stale_incarnation` detail. Readers never treat a published epoch as
   proof of a live server again.
2. **Distinguish C from D** (finding 19): `ls` consults the non-blocking shared `try_acquire`
   and `Registry::current_lease(Recovery(instance))` for an `Unpublished` target before
   choosing advice. Replace the "restart the managed mux service" text at `ls_cli.rs:1193–1200`
   and `gui_lifecycle.rs:977–984` with the state-correct wording that already exists at
   `ls_cli.rs:933–935`.
3. **Explicit clear.** Add `Registry::retire_backend_server(instance, expected_epoch)` (CAS on
   the published epoch; advances the revision chain like `publish_backend_server`). Two
   callers: the managed `mux-startup` path, immediately before publishing its fresh epoch —
   today it overwrites, which is fine, but the retire step makes the transition journaled — and
   `dmux recovery abort`/a new `dmux repair retire-incarnation` for the operator case
   where the service will not come back managed. Re-grade `registry/mod.rs:1586`'s doc from
   "stopped or never published" to the true three-state meaning.
4. **Doctor epoch probe** (finding 14): `dmux doctor` opens the registry read-only and
   reports, per backend instance, `{published epoch, pid, start_token, dev/ino}` versus the
   live descriptor, a `stat` of the socket, and the sentinel list — with the same
   A/B/C/D/E/F classification as `ls`. Both remedies that end "re-run `dmux doctor`" then
   terminate in a report that can actually distinguish the states. `doctor --format json`
   carries the classification so the fresh-reader test (WS-G.3) can cite it.
5. **Diagnose the `starting`-not-`failed` descriptor** on Macie before repairing it (§6.2):
   read the launchd log (`log show --predicate 'process == "wezterm-mux-server"'`) for the
   12:42 start. If `publish_descriptor('failed', recovery_fields(..))` threw, fix the handler
   so the missing-instance path cannot leave `starting`; add a Lua test under
   `wez/dmux_bridge/tests` or the mux test harness for it.

Gate: a test that publishes an epoch against a dead pid and asserts every verb refuses with
`stale_incarnation`; doctor JSON on a synthetic F row names it; the C/D A/B from the report
produces two different remedies.

### WS-C — The legacy default path (finding 9)

`src/list.rs` and `src/attach.rs` run whenever `DMUX_WEZ_FIRST` is unset, and they are the
path every rollback returns to (§21). Minimum before any rollback rehearsal: pin
`WEZTERM_UNIX_SOCKET` to the service socket when a descriptor exists, keep `--no-auto-start`,
add the dmux-side deadline, and filter `WEZ_SENTINEL_PREFIX` from rows and from attach
targets. No registry dependency is introduced — the legacy path stays registry-free — but it
must never present the sentinel as a target. Regression: `dmux ls --json` flag-off with a
sentinel-only server returns zero wez rows; `attach_wez` refuses a sentinel workspace.

Gate: the two tests above; baseline `cli::` tests untouched (they already clear the flag).

### WS-D — Surface still owed by the frozen grammar

1. **`dmux repair rebind SPACE_REF NATIVE_REF`** (§7.1, §10.3). Expert, confirmed, owner-local;
   uses the adoption CAS primitive under the same locks; prints both identities; finishes
   `unstamped`; refuses across hosts like `adopt` (ADR 011 D7). Remote refusal is typed
   `ProtocolMismatch`. This is the remedy `repair reconcile` already names for the orphan case.
2. **Adoption journal carries the source token** (ADR 011 "known limitation"):
   `reserve_space_kind` records `{name, backend_instance, source_native_token}` so a crashed
   Wez adopt is reversed to the *source*, not to the logical name, and so §10.3's
   source/destination/epoch reconciliation becomes real. Registry schema v3 appendix; lossless
   migration test like `migrate_v3.rs`.
3. **`resolve::resolve_space_ref` becomes the one resolver** (report 06 #6). Five inline
   re-implementations of §6.2's precedence (`rm_cli.rs:465`, `space_cli.rs:1098`,
   `gui_cli.rs:2695/2754`, `connect_cli.rs:153`) are replaced by calls; the truth table
   (`tests/resolver_truth_table.rs`) starts vouching for production. Drop the fixture-only
   alias literal at `resolve.rs:376`. Case 44 ("deprecated row indices cannot silently target
   a stable ID") is then asserted once, not five times.

Gate: case 13's "external native-key tampering becomes explicit conflict rather than silent
rebind" exercised end-to-end (tamper → `absent` → `repair rebind` → `unstamped` → stamp →
`healthy`); truth table green against production callers.

### WS-E — Test isolation, dead code, and coverage accountability

1. **Runtime-dir isolation** (§3.2). Every test that reaches `locks`/`dmux_runtime_dir()` gets
   a scratch runtime dir through the existing `DMUX_RUNTIME_DIR`/`--lock-dir` seams; find the
   paths that ignore the seam and make them honour it. Add a suite-level guard that snapshots
   the real `dmux_runtime_dir()` entry count before and after and fails on growth. Then delete
   the ~750 stale lock files on Macie (they are zero-length `fcntl` witnesses; deleting an
   *unheld* one is safe — verify with `lsof` on the directory first) and check Archie.
2. **Executable reproductions for the nine call-chain-only findings** (report 08 §7):
   `operations.rs:2243/2277/133`, `registry/mod.rs:2812`, `space_cli.rs:222 → normalize_apply`,
   `agent.rs:1405`, the five unproven `binding_epoch` tmux verbs, `wez.rs:2806/2744`, and the
   three shell/Lua sites (`94-dmux-context.zsh:216`, `91-tmux-attach.zsh:45`,
   `controller.lua:115`). Each becomes either a regression test landed with its WS-A fix or a
   recorded non-defect in `report/07`.
3. **Dead-code triage** (report 06). Disposition per item, each with a `baseline-tests.json`
   manifest entry where a vouching test is retired (case 46):

   | # | item | disposition |
   | --- | --- | --- |
   | 6 | `resolve_space_ref` | wire (WS-D.3) |
   | 7 | `operations::create_space` test-only preamble that auto-registers the instance | delete the preamble; route its 12 test call sites through `create_space_owner_fenced`; the duplicate epoch guard at `:362–374` goes |
   | 8 | epoch-pinned `unfinished_recovery`, `abort_recovery_generation`, both `record_*_intentional_empty_revision` | production calls the helper its doc names as "the production remove-path helper", or the helper and its tests are retired; the epoch-pinned lookup is used for same-epoch resume, the agnostic one for takeover discovery — decide and document in `registry/recovery.rs` |
   | 9 | `Provider::prepare_presentation`, `PresentationTarget::Wez`, `capabilities`, `group_list` | remove from the trait; retire the contract-harness cases via manifest |
   | 10 | `NativeSnapshot::recovery_titles` | delete |
   | 11 | `gui::discover_single_live_instance` | port its freshness check into `HeartbeatSource::live_instances`, then delete |
   | 12 | `gui_cli::present_cold_production` | delete; fix the false call claim at `connect_cli.rs:569` |
   | 13 | `recovery::atomic_publish_manifest` | delete (production publishes via `4d14486`'s path); retire `tests/recovery/manifest.rs` cases via manifest |
   | 14 | fixed-runtime descriptor readers in `runtime.rs` | delete; fix the doc that prefers them; remove the `expected_epoch: Option<Uuid>` skip shape from the `pub fn` |
   | 15 | five `gui.rs` helpers (§3.5) | production adopts `select_compatible_domain`'s identity check; the other four are unified with their production analogue or deleted; add the rotation success-path test |
   | 16 | `ClientState` | keep; it is P12 D4's type |

4. **Flag-on dispatch coverage** (report 08 §2): confirm `tests/{connect_cli,new_cli}_dispatch.rs`
   and the `ls`/`rm`/`adopt`/`migrate` tests exercise the *gated dispatch* with
   `DMUX_WEZ_FIRST=1` set on the child, not only the library entry points. Add the missing ones.

Gate: zero growth in the live runtime dir across a full suite run; every 06 row has a commit or
a manifest entry; 08 §7 is empty or converted.

### WS-F — Live hosts: repair, durable enablement, migration

Precondition for every step here: read-only evidence first, and no mux server is restarted
while it holds a user pane (§21 rollback rules). Today Macie's mux holds only the sentinel.

1. **Durable enablement mechanism** (§3.1), landed in dotfiles before any host is touched:
   - macOS: a `com.fredrir.dmux-env` LaunchAgent (`RunAtLoad`) that runs `launchctl setenv`
     for each `KEY=VALUE` in a host-local, untracked `~/.config/dmux/service.env`; and
     `dmux-mux-start.sh` sources the same file itself so the mux does not depend on agent
     ordering. The GUI gets the variable from launchd at its next launch, which is what
     `wezterm.lua:9` and the 28 read sites need.
   - Linux: `~/.config/environment.d/50-dmux.conf` (systemd-user reads it at session start and
     it reaches both the unit and the graphical session); the unit's `PassEnvironment` lines
     already accept it.
   - `dmux doctor` reports where the flag came from (process env / launchd / file) so a canary
     report can say whether enablement survived the last boot. Tested by a reboot on Macie
     before the canary starts.
   The same mechanism is the service half of §21 step 9 (ADR 010 §5), with the tracked default
   flipping to `1` at the flip; nothing is host-specific except the file.
2. **Repair Macie** (after WS-B lands and WS-F.1 is in place): verify zero user panes
   (`wezterm cli list` on the pinned socket), run `dmux doctor` and keep its JSON, write
   `DMUX_WEZ_FIRST=1` to `service.env`, `launchctl kickstart -k gui/$UID/com.fredrir.wezterm-mux`,
   wait for the descriptor to reach `ready` with a non-null `backend_instance_uid`, confirm the
   registry now publishes the live epoch/pid/dev/ino (WS-B.3's retire step journals the
   transition), and confirm `doctor` reports state E. Keep before/after doctor output as the
   evidence artifact.
3. **Repair Archie** the same way once its mux is confirmed sentinel-only (or schedule it for a
   moment when its user panes can be detached — never killed).
4. **Migration, both hosts** (§17, §21 step 2, case 45): `dmux migrate` preview on each host,
   inspect the deterministic mapping, `--commit --yes`, verify `migrated-v1.json` and that a
   second run is a no-op. The orphan `w6mac-smoke-20260817-archie` Space (`r5.md`) is resolved
   explicitly first — `rm` or keep — so the preview does not hide a collision.

Gate: doctor reports E on both hosts from a managed, flag-on mux; both migrations committed
and idempotent; a Macie reboot leaves the flag set and the descriptor `ready`.

### WS-G — The P11 gate itself

1. **Acceptance ledger**: `docs/adr/dmux/acceptance-matrix.json` mapping each of cases 1–46
   (17 as 17a/17b) to test IDs (`crate::test::path`), live evidence (rollout checkpoint or
   doctor artifact), and status. Same accountability rule as `baseline-tests.json`: a case is
   not "passed" by a green suite unless the ledger names what proves it. Build it from the 14
   cases already referenced by number, then fill the 32.
2. **r6 release through `dmux-rollout`**: `two_host` (now that `142cff5` pinned the proxy),
   and new verify steps the tool lacks — `canary.start`/`canary.end` with the 24 h floor
   recorded from wall clock, and `rollback.rehearsal` that sets `DMUX_LEGACY_POLICY=1`,
   proves new Spaces go tmux while existing Wez Spaces stay attachable, then clears it. Phase
   names beyond `mac_deployed` are added to the tool.
3. **Fresh-context reader test** (ADR 010 §3): question set covering §6.2, §8.3, §10.1,
   §13.2, §15.3, plus the A–F instance states from WS-B; a reader that is not the root
   answers from the plan and ADRs alone; questions, answers, and citations checked in at
   `docs/adr/dmux/reader-test-p11.md`.
4. **Canary, Macie then Archie**, each 24–48 h under durable `DMUX_WEZ_FIRST=1`, each followed
   by its rollback rehearsal; ledger rows 32–34 and 41–45 re-run live on each.
5. **Remote Wez over USB, then cable removal → same-ID Tailscale reconnect** (§21 step 8,
   cases 20–21) before canarying remote auto-selection.
6. **Docs**: correct `docs/scripts.md:1365–1376` (migrate exists; describe `--row`,
   `repair rebind`, the A–F doctor states, and the env-file enablement); `scripts/COMMANDS.md`
   from `--help` as `ca25073` did; ADR 010 §5's two-halves checklist becomes §21 step 9's text.
7. **The flip**, only after 1–6: `WEZ_FIRST_BY_DEFAULT = true`; tracked service defaults carry
   `DMUX_WEZ_FIRST=1`; `tests/cli.rs` keeps its `DMUX_LEGACY_POLICY=1` line so case 46 holds;
   `the_policy_resolver_answers_every_switch_combination` is re-evaluated against the new
   default; legacy path retained one release.

Gate: §22, every clause, with the ledger as the witness.

## 6. Order and dependencies

```text
WS-A.1–3 ──► WS-A.4 ──► WS-A.5 (9 commits) ──► WS-A.6–13
                  │
                  └──► WS-B.1–4 ──► WS-F.2/3 (host repair)
WS-C  (independent; before any rollback rehearsal)
WS-D.1–2 (after WS-A.4; before WS-G.1's case-13 row)
WS-D.3 (independent; medium; may run alongside WS-A.6+)
WS-E.1 (first — it is the guard every later run needs), WS-E.2 (with WS-A), WS-E.3–4 (after WS-A)
WS-F.1 (independent; needed before WS-F.2 and WS-G.4)
WS-B.5 (independent diagnosis; before WS-F.2)
WS-G.1 (start now; fill as each WS closes) ──► WS-G.2 ──► WS-G.3 ──► WS-G.4/5 ──► WS-G.6 ──► WS-G.7
```

Suggested dispatch (root holds all paths; one file to one agent at a time per ADR 011):

- **Wave 1** (parallel, disjoint files): WS-E.1 (`locks.rs`, `runtime.rs`, test harness
  helpers); WS-A.1–3 (`backend/{mod,scope}.rs` + the 61 mechanical edits — root-only, since it
  touches every CLI file); WS-C (`list.rs`, `attach.rs`); WS-F.1 (`macos/launchd/**`,
  `linux/arch/wezterm-mux/**`, `shared/wezterm/mux/dmux-mux-start.sh`); WS-B.5 (read-only).
- **Wave 2**: WS-A.4–5 (root, serial, nine commits); WS-D.3 (`resolve.rs` + five callers,
  one agent); WS-E.3 rows 9–14 (deletions, one agent).
- **Wave 3**: WS-A.6 (`backend/wez.rs`), WS-A.8–9 (`backend/tmux.rs`, `registry/mod.rs`),
  WS-A.7/10–12 (`main.rs`, `operations.rs`), WS-B.1–4 (`backend/scope.rs`, `ls_cli.rs`,
  `doctor.rs`, `gui_lifecycle.rs`, `registry/mod.rs`), WS-D.1–2, WS-E.3 rows 7–8/15, WS-E.2/4.
- **Wave 4** (root, serial, live): WS-F.2–4, WS-G.1–7.

Nothing in waves 1–3 touches a live host. Wave 4 is the only wave with `launchctl`/`systemctl`
in it.

## 7. Plan amendments this ADR requires (applied when accepted)

1. §21 step 7: the canary host's `DMUX_WEZ_FIRST=1` is set through the durable per-host
   mechanism (WS-F.1), not `launchctl setenv`/`set-environment` alone; a reboot during the
   window is part of the canary, not a reset of it.
2. §21 step 9: fold ADR 010 §5's two halves into the step text, naming the tracked service
   defaults as the second half.
3. §5.2/§8.1: add `stale_incarnation` as a distinguishable instance state (state F) with its
   operator advice; `observation=unreachable` carries it as detail.
4. §20.1: name `docs/adr/dmux/acceptance-matrix.json` as the case-accountability artifact
   beside `baseline-tests.json`, and add "suite runs leave the live runtime directory
   unchanged" to the full-repository test layer.
5. §19.3 handoff contract: a returning agent reports the runtime-dir growth check result.

## 8. Explicitly deferred to P12 (recorded so they are not re-litigated)

- Provider-entry-point split (`inventory_verified`/`inventory_discover`) — report 05 "phase 2".
- `"client"` attachment state (ADR 011 D4, ADR 008 §1.1).
- Remote `--all-hosts` child counts and unmanaged rows (ADR 011 D5).
- Durable child IDs (§2.8), quit-all UX (§13.4), fork user-var exposure (ADR 007 correction 2).
- Property/fuzz testing (report 08 §10) — desirable, not a §22 clause.

## 9. Risks and unknowns

- **The `starting` descriptor** (WS-B.5). If the missing-instance `failed` publish silently
  fails on macOS, the same path may silently fail on a *managed* start; diagnose before
  trusting any descriptor state in the canary.
- **Archie's pane state** is unverified today; WS-F.3 may have to wait for a natural detach.
- **WS-A.6's test churn** (~24 sentinel-bearing wez unit tests) is the one place the report's
  "no flag day" does not hold inside a single file; land it as its own commit with every
  assertion change listed in the message.
- **WS-D.3 is the largest discretionary item.** If it slips, the fallback is to keep the five
  inline resolvers and add a truth-table test per call site — worse, but it does not block §22.
- **Two-host work** (`two_host`, cases 16–22, WS-G.5) still has only one live transport matrix
  exercised (`r5.md`); USB removal with same-ID Tailscale reconnect has not been run live since
  the route split (`1a522dc`).

## 10. Dispatch log (root-recorded per §19.2; appended as waves start and close)

Decisions settled with the owner before dispatch, 2026-08-22:

- The root commits directly to `dmux`, one commit per numbered item; nothing is pushed unless asked.
- WS-A.5's nine sites are parallelised across file-disjoint agents in worktrees; integration stays serial, in the blast-radius order §5 gives. §6's "root, serial" is thereby amended.
- WS-B.3's operator verb is a new `dmux repair retire-incarnation`, recorded as a §7.1 grammar addition in the style of ADR 011 D8; `dmux recovery abort` keeps its existing meaning.
- WS-G.2's phase names beyond `mac_deployed`: `arch_deployed`, `migrated`, `canary_mac`, `canary_arch`, `flipped`.
- The orphan Space `w6mac-smoke-20260817-archie` is removed (`rm`), not kept, before WS-F.4's migration preview.
- WS-G.3's reader is a fresh subagent with no conversation context, given only the plan and `docs/adr/dmux/**`.
- Live-host steps each require a separate owner confirmation: Macie kickstart, anything on Archie, `migrate --commit` per host, canary start/rollback rehearsal, the flip. Waves 0–3 touch no host.

Refinement to WS-A.3: the audit test holds an explicit allowlist rather than "exactly two" from the outset, so the suite stays green at every commit between A.3 and the end of A.5; each A.5 commit removes one suspect entry, and the allowlist ends at the two legitimate sites. The burn-down is thereby visible in the diff.

Findings at dispatch time (root, read-only): Archie is sentinel-only, rebooted 2026-08-22, descriptor `starting` with `backend_instance_uid: null`, no `DMUX_*` in its systemd user environment, no `environment.d`; its journal shows `starting descriptor FAILED: native descriptor publisher returned a mismatched service witness` on every boot since 2026-08-18. Macie's per-pid mux log (`wezterm-mux-server-log-54528.txt`) shows the same two refusals at 12:42:45 on 2026-08-19; the handler returns at `dmux-mux.lua:1143` before the missing-instance guard at `:1147`, which is why `failed` never lands (answers report 08 §9). Tailscale SSH from Macie to Archie requires an interactive Tailscale check-in; USB works non-interactively. A full suite run with `DMUX_RUNTIME_DIR` exported still grew the live runtime dir by 18 entries (9 `backend_`, 8 `space_`, 1 `decision_` locks) — the seam is bypassed, not merely unused.

### Wave 1 — dispatched 2026-08-22, base `6451acd`

| agent | workstream | worktree / branch | owned paths |
| --- | --- | --- | --- |
| E1 | WS-E.1 | `~/packages/dmux-p11/e1`, `p11/e1` | `src/locks.rs`, `src/runtime.rs`, new guard files under `tests/`; line-scoped lock-path edits elsewhere only if uncentralisable |
| C | WS-C | `~/packages/dmux-p11/c`, `p11/c` | `src/list.rs`, `src/attach.rs`, one new test file under `tests/` |
| F1 | WS-F.1 | `~/packages/dmux-p11/f1`, `p11/f1` | `macos/launchd/com.fredrir.dmux-env.plist`, a new loader script, `shared/wezterm/mux/dmux-mux-start.sh`, `linux/arch/wezterm-mux/**`, `src/doctor.rs` (flag-source section only), one new `tests/cases/` file |
| B5 | WS-B.5 | `~/packages/dmux-p11/b5`, `p11/b5` | `shared/wezterm/mux/dmux-mux.lua`, cases under `shared/wezterm/wez/dmux_bridge/tests/`, `tests/recovery/mux_lua_contract.rs` only if source-pinning is chosen |
| root | WS-A.1–3, WS-G.1 seed, this record | `/Users/fredrir/dotfiles`, `dmux` | `backend/{mod,scope}.rs`, the 61 `InventoryScope` sites, `docs/**` |

Every agent's return must include the runtime-dir growth check (§7 amendment 5).

### Wave 1 closes

- **WS-B.5 — closed 2026-08-22** (`9e741f2`, `207ab92`, cherry-picked onto `dmux`). Root cause: the
  fork returns the published descriptor through mlua's serde bridge, which renders a Rust `None` as
  mlua's JSON-null *light userdata*, not Lua `nil`. A flag-off request omits `backend_instance_uid`,
  so the old clause `descriptor.backend_instance_uid ~= request.backend_instance_uid` compared
  `userdata ~= nil`, refused a descriptor the native side had already written
  (`publish_replace` precedes the Lua check), and the handler returned at `dmux-mux.lua:1143` before
  the only `failed` publish on that path. Managed starts send and receive a string and were never
  affected (all 17 flag-on starts logged on Macie reached `ready`). Fix is Lua-only:
  `service_witness_mismatch` names the disagreeing field, `native_absent` accepts `nil` or a
  metatable-less userdata only when no UID was requested, a refused `starting` now publishes
  `failed` with the refusal as its reason, and `error` text is normalised to the schema's bounds so a
  `failed` publish cannot be refused for its own formatting. Report 08 §9 is answered. Optional fork
  hardening (`serialize_none_to_null(false)` at `dmux_descriptor.rs:1332`) recorded, not required.
  Takes effect at the next service restart, i.e. at WS-F.2; expected flag-off log after it:
  `mux-startup BEGIN` → `sentinel spawned` → `mux-startup unavailable: no durable backend identity`
  and a descriptor reading `state: failed`, `backend_instance_uid: null`, sentinel fields present.
- Root-side wave 0 closed: WS-A.1 `027e777`, WS-A.2 `80ac3a7`, WS-A.3 `ce76a7c`, WS-A.4 (this
  record's parent commit), WS-G.1 seed `636ee66`. Suite 990/0/1 at WS-A.4.
- Operational note for every later worktree: the pre-commit hook needs `scripts/python/.venv`;
  run `uv sync --project scripts/python --locked` in a fresh worktree before its first commit.

### Wave 2 — dispatched 2026-08-22, base `f663972` (WS-A.4 `cb3c543` plus the WS-B.5 picks)

WS-A.5 fanned out by file, per the owner-approved amendment to §6; integration is serial in
blast-radius order: A5-a adopt → A5-a migrate → A5-b reconcile → A5-b group new → A5-c spaces →
A5-d new → A5-d rm → A5-e → A5-c new_lookup. WS-A.7 (`main.rs:1450`) stays with the root in wave 3.

| agent | sites (report finding) | worktree / branch | owned paths |
| --- | --- | --- | --- |
| A5-a | `adopt_cli::owner_scope` (#2), `migrate_cli::scan_target` + `Target` (#3) | `~/packages/dmux-p11/a5a`, `p11/a5a` | `src/{adopt_cli,migrate_cli}.rs`, `tests/{adopt_cli,adopt_flow,migrate_cli}.rs`, own audit entries |
| A5-b | `space_cli::reconcile_provider` tmux arm incl. the `.ok()` (#7), `group new` tmux arm (#4) | `a5b`, `p11/a5b` | `src/space_cli.rs`, `tests/{hierarchy_flow,registry/reconcile}.rs`, one new test file, own entries |
| A5-c | `remote/agent.rs` `spaces` tmux arm (#13), `owner_lookup_target` (#17) and WS-A.12's nil-epoch fabrication (#20) | `a5c`, `p11/a5c` | `src/remote/agent.rs`, `tests/remote_protocol/**`, own entries |
| A5-d | `new_cli::local_target` (#12), `rm_cli::local_scope` (#16) | `a5d`, `p11/a5d` | `src/{new_cli,rm_cli}.rs`, `tests/{new_cli,new_cli_dispatch,space_rm_cli}.rs`, own entries |
| A5-e | `gui_cli::local_opposite_create_target` (#15) | `a5e`, `p11/a5e` | `src/gui_cli.rs` (function, caller, tests module only), `tests/{gui_lifecycle,connect_cli,connect_cli_dispatch}.rs`, own entry |

Each commit states what `Unpublished` means for its verb; each deletes its `tests/scope_audit.rs`
allowlist entry; each lands a regression test that is the review's reproduction inverted.
- **WS-F.1 — closed 2026-08-22** (`9f6d469`, `f9fbdd9`, `6b15866`, `386fe55`, cherry-picked as
  `539ff49`…`fc38805`). Mechanism: the untracked `~/.config/dmux/service.env` is the durable source
  on macOS — parsed by one sourced helper (`shared/wezterm/mux/dmux-service-env.sh`: keys
  `^DMUX_[A-Z0-9_]*$`, values `^[A-Za-z0-9_./:@+,-]*$`, last assignment wins, one malformed line
  refuses the whole file, never eval/source), applied to the launchd session at login by the
  one-shot `com.fredrir.dmux-env` LaunchAgent (`dmux-env-load.sh`), and read by `dmux-mux-start.sh`
  itself so the mux never depends on agent ordering. Precedence: a non-empty process-environment
  value, then the file (Darwin only), then the tracked default `0`. On Linux the single knob is
  `~/.config/environment.d/50-dmux.conf`; the unit's `PassEnvironment=` lines are inert for a user
  manager (systemd.exec(5)) and the manager's block delivers the value, so no directive changed.
  `dmux doctor` reports the flag per layer (process / launchd-or-systemd / file) with a
  reboot-durability verdict, in human output and both JSON forms. 13 shell cases under
  `tests/cases/dmux-service-env.sh`, 3 doctor unit tests. Not proven here by design: a real
  `launchctl bootstrap`, a real reboot, and the Linux arm on a Linux host — those are WS-F.2/F.3's
  evidence. The loader only sets, never unsets: to state legacy, write `0`.
- **WS-E.1 — closed 2026-08-22** (`d7a0234`, `fa78d56`, cherry-picked as `d060f21`, `6af50cc`; root
  harness lines in the commit after). The bypass was one point: `runtime::dmux_runtime_dir()`
  deliberately ignored `DMUX_RUNTIME_DIR` (a note from `88f585e`; only `pane-bootstrap` read it),
  and every lock, socket, descriptor, bridge-key and bootstrap path is built relative to what that
  resolver returns. The resolver now honours the seam (absolute path used verbatim; relative refused;
  empty = unset), which the root ratifies against the `88f585e` note: ADR 009 §6 already classes the
  variable with `--data-dir`/`--lock-dir` as an owner-side seam, and the peer's `ssh <route> dmux
  _agent` command line carries those explicitly. Guards: `tests/runtime_dir_seam.rs` (a re-executed
  copy of the test binary proves the production constructors resolve to the seam and the platform dir
  is never touched) and `tests/run-isolated.sh` (fresh short seams, recursive before/after snapshot
  of the live dir, fails naming new entries; refuses a seam whose socket path would exceed
  `sun_path`). Proof: 989/0/1 under the wrapper with the live dir at 1367 → 1367. Four harnesses that
  export no seam still reach the live authority gate under bare `cargo test`: `json_envelope.rs` and
  `cli.rs` are fixed by the root in the next commit (the `cli.rs` line is a harness migration in ADR
  011 D1's sense, no assertion changes); `new_cli_dispatch.rs` and `connect_cli_dispatch.rs` follow
  when their wave-2 owners return. Follow-ups recorded: WS-B.4's doctor reports a set seam; the
  stale-lock deletion uses E1's catalogue (1327 uuid-named zero-byte locks matching no live identity;
  keep `backend_6ef8d4c9…`, the nine `decision_9d1950c7…`, `authority-gate.lock`, the socket,
  descriptor, lease, `.replace-*`, `bootstrap/`) and runs only when no suite is running anywhere.
- **WS-A.5 adopt + migrate (A5-a) — closed 2026-08-22** (`f78c391`, `6dfeada` → `9360d3f`, `5b00e13`).
  `adopt_cli::owner_scope` and `migrate_cli::{Target, scan_target}` deleted; both consume
  `resolve_managed`. Unpublished means refuse: `dmux adopt` refuses `backend_epoch_changed` before
  verify/reservation/CAS; `dmux migrate` surfaces a typed `backend_epoch_changed` blocker in preview
  and returns `blocked` from `--commit` before backup, adopt, or the cutover stamp. Three regressions
  (each failed on the pre-change code, proving it is the review's reproduction inverted). Ledger:
  case 13 and case 45 rows updated; both still blocked on WS-D.1/D.2 and WS-F.4 respectively.
  E1 gate on `dmux` before this pick: 998/0/1 under `run-isolated.sh`, live dir 1385 → 1385.

