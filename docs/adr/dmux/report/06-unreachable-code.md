# "Exists, tested, unreachable" — further instances

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`. Answers review-ask #6 (widen once — find the pattern itself).

---

Beyond the five already known, with the test currently vouching for each:

| # | site | vouching test |
|---|---|---|
| 6 | `resolve::resolve_space_ref` (`resolve.rs:364`) + the whole `RefResolution` enum | `tests/resolver_truth_table.rs:417-439` (440 lines). Zero production callers; five CLI sites re-implement ref resolution inline (`rm_cli.rs:465`, `space_cli.rs:1098`, `gui_cli.rs:2695`/`2754`, `connect_cli.rs:153`). Proof it was never productionised: `resolve.rs:376` accepts exactly the single literal alias `"a"` used by the fixture. |
| 7 | `operations::create_space` (`operations.rs:320`) | `tests/operations_flow.rs:151,208,253,273,287,302,317`, `hierarchy_flow.rs:139,533`, `json_envelope.rs:404`, `normalize_flow.rs:221`, `space_rm_cli.rs:517`. Production creates via `create_space_owner_fenced`. Its test-only preamble *auto-registers the instance from the caller's scope* (`operations.rs:328-330`) — the opposite of production, which refuses a mismatch at `:1014-1028`. Its epoch guard at `:362-374` is a second, test-only implementation of `scan_epoch_for_create`. |
| 8 | Four registry recovery-journal APIs: `unfinished_recovery` (`registry/recovery.rs:705`, the **epoch-pinned** one), `abort_recovery_generation` (`:884`), `record_current_intentional_empty_revision` (`:567`), `record_intentional_empty_revision` (`:513`) | `tests/registry/recovery.rs:147-236, 802-877, 1134-1388`, `migrate_v3.rs:177,182`. `:562-566` documents #3 as *"the production remove-path helper"* — it has no production caller, and the production abort re-implements its SELECT-head-then-UPDATE-floor sequence inline at `:976-991`. Production uses the epoch-**agnostic** `unfinished_recovery_for_instance` everywhere. |
| 9 | `Provider::prepare_presentation` (`backend/mod.rs:315`) and `PresentationTarget::Wez` | `tests/provider_contract.rs:257`, `tests/provider_tmux.rs:603`, `wez.rs:3475`, `tmux.rs:3029/3049/3062`. `WezProvider::prepare_presentation` unconditionally returns the same error (`wez.rs:2127-2134`), and `PresentationTarget::Wez` is constructed exactly once in the crate — at `tests/provider_contract.rs:139`, by a fake. Also no production caller: `Provider::capabilities` and `Provider::group_list`. |
| 10 | `NativeSnapshot::recovery_titles` (`recovery.rs:765`) | **none** — zero callers, zero tests. A dead duplicate of the ADR-004 reserved-title correlation the coordinator actually performs at `recovery.rs:3860`/`4219-4232`. Listed because it is a verification primitive on the recovery path. |
| 11 | `gui::discover_single_live_instance` (`gui.rs:792`) | **none**. Its doc calls it "the zero-window `summon` seam"; `summon` uses `HeartbeatSource::live_instances` (`gui_cli.rs:3708`). The dead one performs a freshness check (`gui.rs:814`) the live one does not. |
| 12 | `gui_cli::present_cold_production` (`gui_cli.rs:6530`) | **none**, and `connect_cli.rs:569` documents a call relationship that does not exist. |
| 13 | `recovery::atomic_publish_manifest` (`recovery.rs:596`) | `tests/recovery/manifest.rs:10,291`. Its doc states a lock/lease precondition no production caller can honour, because there is none. |
| 14 | Both fixed-runtime descriptor readers (`runtime.rs:632`, `:478`) | **none**, and `runtime.rs:639-641` tells production callers to prefer the one with no callers. Production uses only the `_in(..)` forms. `read_verified_ready_wez_descriptor`'s `expected_epoch: Option<Uuid>` preserves the exact skip shape this review is about, on a `pub fn`. |
| 15 | Five gui.rs security helpers: `select_compatible_domain` (`gui.rs:1246`), `validate_acknowledgement` (`:2016`), `bind_cli_origin` (`:730`), `parse_signed_origin_json` (`:913`), `rotate_bridge_key_if_idle` (`:457`) | `gui.rs:4010/4012, 4209/4215, 3846, 3856/3861, 4114`. Each has a *different* production analogue. Semantic gap: `select_compatible_domain` checks rows against the caller's validated identity (`gui.rs:1258-1262`); production's `choose_compatible_presentation_row` checks candidates only against each other (`gui_cli.rs:993-1001`). Coverage gap: the sole `rotate_bridge_key_if_idle` test asserts the *refusal* branch, so the rotation success path at `gui.rs:466-470` has neither caller nor test. |
| 16 | `ClientState` (`model.rs:135`) | `model.rs:353` — a serde-spelling assertion, the type's only reference in the crate. |

Also: four default `Provider` trait bodies (`backend/mod.rs:375/385/396/408`) can never be selected —
both providers override all four and no third impl exists — but `normalize_plan`/`normalize_apply`'s
defaults *are* live, because tmux does not override them. Defensible scaffolding, recorded for
completeness. And `is_eligible` (`remote/wez_compat.rs:617`) is tautological: the second conjunct
can never be false given the constructors at `:649-653` and `:714-718`, and it is asserted green at
`tests/remote_protocol/capability_gate.rs:288`.
