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

## Decisions escalated by the design pass and settled here

### D5 — remote `--all-hosts` rows carry null child counts

`protocol::methods::SPACES` returns durable `SpaceInfo` plus a `ScanSummary` and nothing per-row:
no group/split counts, no unmanaged rows. So `ls --all-hosts` renders remote counts as `-` (human)
and `null` (JSON), and omits remote unmanaged resources behind a visible per-host note.

Filling them means a new remote capability and a frozen-contract change. That is not P11 work, and
case 24 does not need it — case 24 is about the four scopes being *distinct and documented*, and
case 23's counts are explicitly local ("Two Wez tabs/four panes report two Groups/four Splits").
Recorded rather than silently rendered as zero, because a zero would be a lie and a `null` is a
statement that the owner was not asked.

### D6 — the `RM`/`RENAME` client wrapper is pulled forward from P7

`resolve_live` already resolves remote owners over `call_over_routes`, but the mutation half of
`methods::{RM,RENAME}` has an agent handler and no client caller. Cases 41 and 42 need it, so it
lands in P11 rather than waiting. It is kept local to `rm_cli.rs`: `remote/client.rs` is
remote-agent-owned and `new_cli.rs` is being edited in the same window, and neither needs to change
for this.

### D7 — remote `adopt` is refused, not implemented

There is no `ADOPT` method in the protocol, and adoption is owner-local by §2.6 — the owner host is
the sole authority for adoption, mutation journals, and tombstones. `dmux adopt --host b` therefore
returns a typed `ProtocolMismatch` naming the limitation, the same shape `space_cli.rs:513` already
uses for cross-host refs. Case 13 does not require remote adoption; it requires that adoption be
explicit, fenced, and once-only, which the owner-local path delivers.

## D8 — `repair reconcile` is added to the frozen §7.1 grammar

`registry::reconcile` shipped with zero production callers: `resume_duty`, `decide_rename` and
`decide_create` were exercised only by `tests/registry/journal.rs`. The damage was reachable — a
crash between `reserve_space_kind` and `abort_create` left a `reserved` row and a `prepared`
operation that no verb could reap, burning the logical name permanently. Case 13 says "adoption
crash states reconcile"; they did not.

Adding a verb to a frozen contract needs recording. §7.1 gains
`dmux repair reconcile [SPACE_REF...]` and §7.4 gains its behavioural rule. It sits beside
`normalize` and `rebind` because all three are explicit, confirmed, owner-local repairs of managed
state — §10.3's category, not a new one.

Two properties are load-bearing and are written into §7.4 so they cannot be traded away later:

1. **`resume_duty` decides.** The verb gathers evidence and applies an outcome; it does not form
   its own opinion about what a crashed operation means. A second judgement would be a second
   contract.
2. **It never binds an orphan to a reserved key.** `resume_duty` permits `RebindAndFinalize` for
   the one-conforming-match case, but binding requires the bootstrap acknowledgement `create_space`
   performs and `repair` cannot, and on tmux the key is a mutable name a stranger could hold. The
   verb takes only the weaker half of that permission — release, never bind — and refuses the rest
   naming `repair rebind`.

`repair rebind` remains unimplemented. It is now load-bearing as the named remedy for the orphan
case, so it is no longer optional polish.

### Known limitation, recorded rather than hidden

§10.3 says the adoption journal covers the source token, but `reserve_space_kind` records only
`{name, backend_instance}`. So when reconcile compensates a crashed Wez adopt by reversing the CAS
rename, it renames to the reservation's **logical name** — byte-identical to the source unless
`dmux adopt --name` was used. Making it exact requires a registry payload change carrying the
source token, which also unblocks a real source/destination/epoch reconciliation. Not done here.
