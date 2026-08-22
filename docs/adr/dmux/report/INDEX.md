# dmux epoch-verification integrity review

Status: findings recorded, no code changed
Date: 2026-08-22
Scope: `scripts/rust/crates/dmux` at `493e92c` (crate is byte-identical at branch HEAD `b7d729f`)
Plan refs: §2.3, §2.7, §8.2, §11, §14, §20.2 acceptance case 25, §21
ADR refs: 001 (strict endpoint selection), 002 (service/sentinel startup)
Line numbers: every file:line in this report was re-opened and confirmed against the working tree
Method: independent adversarial multi-agent review — 265 agents, six enumeration sweeps,
per-site classification and reachability tracing, eight specialist audits, nine executable
reproductions, a four-way fix-shape panel, adversarial refutation, and a completeness critic

---

## Verdict

**The class is open, and it is wider than the brief describes.** 493e92c closed exactly one of
23 production scope-construction sites (`ls_cli.rs:746`) by making the epoch non-`Option` inside
a file-local `ManagedScope`; the other nine defective sites were untouched, and seven of the nine
were reproduced executably — including two that complete a **native mutation** on a server nothing
verified (`dmux adopt` CAS-renames a stranger's workspace and exits 0; `dmux migrate --commit`
batch-adopts two of them and writes the cutover stamp). The brief's own premise is wrong in two
directions: wez mutations do **not** require an epoch (nine verbs call `verified_scan(scope, None)`,
`create` and `cas_rename_workspace` among them), and tmux managed reads **are** epoch-verified in
three places, so the prior reviewer's summary sentence is false. The single most serious thing found
is not on any prior list: **this host is in a divergent state right now** — the registry publishes
epoch `40c99029…` against pid **5458, which is dead**, while the live managed mux (pid 54528) serves
epoch `895ca35a…`, a value that appears **zero times** in the registry; nothing in the crate can
clear, re-validate, or even observe a published incarnation whose process has exited, so the
laundering class is dormant here only because every site fails closed *on the wrong value*.
The deeper cause is a contract that drifted: `expected_epoch: Option<ServerEpoch>` was frozen in
`cb780bd` ("P1: frozen contracts") to mean *"the pin a caller already holds"*, and its doc comment
at `backend/mod.rs:119-120` is byte-identical today, seventeen commits after the registry began
supplying `NULL` for the same slot.

---

## Key summary

### Three headline corrections to the brief's premises

1. **wez mutations do not require an epoch.** Nine verbs call `verified_scan(scope, None)` —
   `create` (`wez.rs:2049`), `cas_rename_workspace` (`wez.rs:2814`), `split_new`, `split_remove`,
   `group_rename`, `group_remove`, `split_list`, `normalize_plan`, `sole_window_id`. Only **four**
   wez sites call the fail-closed `required_action_epoch` (1359, 1405, 1522, 1581). tmux has
   **thirteen** fail-closed callers. The strictness runs opposite to the assumption: where tmux
   loses verification it is the caller's doing; where wez loses it, it is the adapter's.
2. **"No managed-tmux read anywhere in the CLI is epoch-verified" is false.** Three
   counter-examples, `connect_cli.rs:1095` being the model implementation. The accurate statement
   is narrower — see [03-tmux-verification.md](03-tmux-verification.md).
3. **The known-wrong prior-list item is `space_cli.rs:221-233`.** It is not a literal
   `expected_epoch: None`; exactly four code literals exist and it is not among them. Wrong item
   does not mean clean — see [07-refuted-and-false-leads.md](07-refuted-and-false-leads.md).

### This host is in a divergent state right now

Confirmed read-only; the live registry's sha256 was identical before and after every check.

| | registry publishes | reality |
|---|---|---|
| epoch | `40c99029-a9f9-4de6-886d-72afa3000d82` | live mux serves `895ca35a…`, which occurs **0 times** in the registry |
| pid | `5458` | **dead** — no such process. The live mux is pid `54528` |
| socket dev/ino | `16777231 / 10519741` | `16777233 / 14788383` — same path, different inode |

`registry/mod.rs:1551` is the sole writer of `server_epoch`. There is no `SET server_epoch = NULL`,
no `DELETE FROM backend_instances`, and no liveness re-check, so a row naming a dead pid is
permanent and every reader treats it as authoritative-and-wrong. The laundering class is dormant on
this host only because every site fails closed *on the wrong value*.

### Scoreboard

- **23** production `InventoryScope` construction sites: **11** verified, **9** unverified against a
  managed instance, **2** legitimately unmanaged, **1** closed by `493e92c`.
- **22** ranked findings, **6** critical, **7** of the nine unverified sites reproduced executably.
- **7** candidate findings killed by adversarial verification, one of whose stated evidence was
  factually false.
- **11** further instances of the "exists, is tested, is not reachable" pattern, beyond the five
  already known.
- Baseline held: `cargo test -p dmux -- --test-threads=1` → **984 passed, 0 failed, 1 ignored**.
  Tree clean, live registry byte-identical, no mux server stopped or started.

---

## Ranked findings

Full detail and per-finding evidence in [01-findings.md](01-findings.md).

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

---

## Contents

| file | what is in it | answers |
|---|---|---|
| [01-findings.md](01-findings.md) | the 22 ranked findings with mechanism, invocation, consequence and evidence for each | ask #2 |
| [02-site-enumeration.md](02-site-enumeration.md) | all 23 production sites plus the test sites, classified, with the rule used to tell managed from genuinely unmanaged | ask #1 |
| [03-tmux-verification.md](03-tmux-verification.md) | whether any managed-tmux read is epoch-verified, and the two providers' relative strictness | ask #4 |
| [04-fence-and-instance-states.md](04-fence-and-instance-states.md) | the six instance states, which are distinguishable at read time, and which operator advice is safe in each | ask #5 |
| [05-fix-shape.md](05-fix-shape.md) | the recommended boundary, argued against the three rejected alternatives, with measured blast radius and migration order | ask #3 |
| [06-unreachable-code.md](06-unreachable-code.md) | eleven further "exists, tested, unreachable" instances with the test vouching for each | ask #6 |
| [07-refuted-and-false-leads.md](07-refuted-and-false-leads.md) | every reported site that is not a defect, the killed findings, and the confirmed-clean list | — |
| [08-untested.md](08-untested.md) | exactly what was not verified and why, itemised and unhedged | — |
| [09-next-actions.md](09-next-actions.md) | thirteen ordered, concrete next steps | — |

---

## How this was produced

Six independent enumeration sweeps ran first — by type name, by field name, by the
`Option<ServerEpoch>` dataflow out of `Registry::backend_server`, by non-`InventoryScope` epoch
carriers, by the provider-method matrix, and by the non-Rust surface (shell, Lua, service units).
Their union was deduplicated to a canonical site list; every site was then independently
re-derived by a classifier that was not shown the enumerator's conclusion, and every site
classified as defective had its call chain traced upward to a real CLI subcommand or RPC route.

Findings were then adversarially refuted rather than confirmed. Critical and high findings were
attacked by three independent verifiers with distinct lenses — reachability, code-reading, and
severity-inflation — and survived only on a majority; medium and low findings received one
verifier running all three checks. Each verifier's default was REFUTED. Seven findings did not
survive and are recorded in [07-refuted-and-false-leads.md](07-refuted-and-false-leads.md) so they
are not re-reported later.

### Constraints observed

No live state was mutated. No mux server was stopped, restarted or kickstarted (plan §21).
`DMUX_WEZ_FIRST` was never set. Every reproduction ran against a copy of the registry under a
scratch `XDG_DATA_HOME`, `DMUX_RUNTIME_DIR` and `TMUX_TMPDIR` exported in the same command as the
binary, with the live registry's sha256 compared before and after. Scratch tmux servers used a
private `-L` namespace and only servers started by the review were killed.

### Known limits of the method

Nine findings have call-chain proof only and never ran; they are named individually in
[08-untested.md](08-untested.md) and are the likeliest to be wrong. Five candidates were
capped out of adversarial verification and are neither confirmed nor refuted — one of them,
`registry/mod.rs:1586`, is the mechanism of finding #1 and should be re-graded. Because
`WEZ_FIRST_BY_DEFAULT = false`, reproductions of gated verbs called the library entry point the
gate dispatches to rather than the gated dispatch itself.

---

## Independent spot-checks

These were re-run outside the agent fan-out, against the working tree, and are the cheapest
claims to re-verify:

```console
$ grep -c "verified_scan(scope, None)" src/backend/wez.rs          # 9  unpinned wez scans
$ grep -c "required_action_epoch(scope)" src/backend/wez.rs        # 4  fail-closed wez sites
$ grep -c "Self::required_epoch(scope)" src/backend/tmux.rs        # 13 fail-closed tmux sites
$ grep -rn "expected_epoch: None" src/                             # 4 code literals + 1 doc comment
$ ps -p 5458                                                       # dead; registry still publishes it
```

The socket identity check that would have caught the live divergence does exist and is enforced in
roughly six places for wez, including the five-way comparison at `space_cli.rs:1042-1053`. It is
absent specifically for tmux: `operations.rs:132-133` passes `None, None` for
`socket_dev`/`socket_ino` on the only production tmux publish path. That is finding #11.
