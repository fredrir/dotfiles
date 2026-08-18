# ADR 011: P11 dispatch record — ownership, decisions, and the wiring order

Status: active (P11). Root-owned. Records the §19.2 W7 ownership starts and the
cross-cutting decisions the CLI-wiring agents were dispatched against.
Date: 2026-08-18
Plan refs: §7.1, §7.4, §16.2, §19.2 W7, §19.3, §20.2 cases 7/13/24/41/42/43/44/45, §21

## Why this record exists

§19.2 requires the root to record each ownership start before dispatch, and
ADR 007's ratification put every specialist glob back in root's hands for
P11-P12. This is that record for the CLI-wiring work, plus the four decisions
the parallel agents were told not to relitigate.

## What was found

Large parts of P4 (resolver/output) and P6 (adoption) were reported complete
but were never connected to the CLI. Verified at `60a87fd`:

- `inventory::reconcile`, `output::render_ls`, `output::unmanaged_row_json`,
  and `output::confirmation_required` have **no production caller** — only
  their own unit tests and `shadow_compare.rs`.
- `main.rs:565-570` dispatches `ls` unconditionally to legacy `list::run`.
- `list.rs:102` emits `serde_json::to_string(&rows)`, a bare unversioned
  array, while the §16.2 schema-versioned envelope sits unused at
  `output.rs:28`.
- `--tree`, `--all-hosts`, `--row`, `adopt`, and `inspect` are absent from the
  clap surface; `dmux adopt` falls through to the connect query.

So the phases that owned acceptance cases 13, 24, 41, 42, 43, 44, and 45 shipped
the libraries and the tests but not the surface. P11 therefore includes wiring
it, rather than inheriting it as done.

## Ownership (recorded per §19.2)

§19's path table predates `connect_cli.rs`, `new_cli.rs`, `space_cli.rs`, and
`gui_cli.rs`. The CLI orchestration set — `main.rs`, `list.rs`, `attach.rs`,
`doctor.rs`, `output.rs`, `inventory.rs`, `resolve.rs`, `policy.rs`,
`operations.rs`, `space_cli.rs`, `new_cli.rs`, `connect_cli.rs`, and the new
`ls_cli.rs` / `rm_cli.rs` / `adopt_cli.rs` / `migrate_cli.rs` — is root-owned
and sub-delegated by exact file, one file to one agent at a time.

## Decisions

### D1 — the feature gate is the existing `wez_first_enabled()` idiom

`main.rs:1417` already gates the Wez-first surface for `con` and `new`, with a
typed usage error when a Wez-first-only flag is passed with the gate off. Every
new arm follows that precedent.

This is what makes the wiring cost **zero baseline retirements**: `tests/cli.rs:70`
does `.env_remove("DMUX_WEZ_FIRST")` on every baseline invocation, so all 73
frozen `cli::` tests exercise the legacy path and are unaffected by anything the
Wez-first path does.

The collision is deferred to the flip (§21 step 9), where `wez_first_enabled()`
inverts to "unless `DMUX_LEGACY_POLICY=1`". The resolution then is one harness
line — `.env("DMUX_LEGACY_POLICY", "1")` beside the existing `DMUX_DRY_RUN` —
because the legacy path is still shipped for one release. Every assertion
survives verbatim. **Recorded now as a planned harness migration, not as
retirements**, so acceptance case 46 stays green through the flip.

### D2 — `--format json` is the envelope; `--json` stays the legacy bare payload

§7.1 makes `--format human|json` a global. Existing `--json` flags are not
repurposed.

| flag | shape | lifetime |
| --- | --- | --- |
| `--format json` (global) | `output::document(...)` envelope, §16.2 / ADR 008 §1 | permanent |
| `--json` on `ls`/`doctor`/`group ls`/`split ls`/`host ls`/`repair normalize` | today's bare payload plus a stderr deprecation hint | removed one release after the flip |

An env-var-dependent JSON *shape* would be a trap: the same command would emit
different documents depending on an ambient variable. Splitting by flag makes
case 43 satisfiable with no test churn, because every frozen test that pins a
bare payload asserts stdout only.

**Hard constraint:** deprecation hints go to **stderr only**.
`cli::a_numeric_target_resolves_against_the_listing` and
`cli::a_filtered_listing_keeps_the_merged_indices` assert stdout by exact
equality.

### D3 — the global `--format` collides with `recovery status --format`

clap panics on duplicate arg ids in one command path, so a global `--format` on
`Cli` cannot coexist with the local one on `RecoveryCmd::Status`. The local flag
and `RecoveryFormat` are deleted and `recovery status` reads `cli.format`.

This is a runtime panic, not a compile error, so it is caught only by invoking
the binary. The pre-pass verifies `--help` renders for every affected command.

### D4 — `"client"` stays `"unknown"`

ADR 008 §1's example shows `"client": "attached"`, but
`output::managed_row_json` hardcodes `"unknown"` and `backend::NativeSpaceRow`
carries no attachment field at all. Filling it truthfully is a provider-contract
change plus both adapters. No case in 1–46 requires the true value — case 31 is
the P9 GUI status line, a different surface. **P12.**

## Wiring order

The pre-pass exists because `main.rs` is touched by six items; doing it once,
alone, lets the rest run in parallel without any of them reopening that file.

| step | scope | parallel? |
| --- | --- | --- |
| W0 | case 7 stopped-service repeat (`new_cli.rs` only) | yes, with everything |
| W1 | root-only pre-pass: clap surface, dispatch skeleton, `output::parse_native_ref` / `render_tree`, `state::entries`, ADR 008 amendments | no — blocks W2 |
| W2-A | `ls` new path, `--tree`, `--all-hosts` (cases 24, 43 listing half) | yes, after W1 |
| W2-B | Space `rm`/`rename`, `--row`, exit 5, per-target JSON (cases 41, 42, 44) | yes, after W1 |
| W2-C | `--format json` envelope for the remaining verbs (rest of case 43) | yes, after W1 |
| W2-D | `dmux adopt` (case 13) | yes, after W1 |
| W3 | migration driver (case 45) | after W2-A and W2-D |
