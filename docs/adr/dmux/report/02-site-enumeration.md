# Complete site enumeration

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`. Answers review-ask #1 (enumerate every construction site).

---

**How managed was told from genuinely-unmanaged, at every site, one rule:** a site is *managed* iff
the endpoint was derived from a registry instance — `backend_instance_for_backend(b)` or
`space_row.backend_instance` → `backend_instance_info(..).socket_path` — i.e. the registry itself
asserts an addressable instance it owns. It is *unmanaged* iff the endpoint came from a compile-time
first-contact namespace reached only after `backend_instance_for_backend` returned `None`, from an
operator flag, or from ambient env with no registry subject. One override: an ambient endpoint is
still managed if the *subject* is a registry Space (`main.rs:1450` pairs `$TMUX` with
`DMUX_SPACE_UID` and `context_read` looks that Space up). Applying this, **exactly one** production
site is legitimately unmanaged.

Arithmetic: `rg -n 'InventoryScope\s*\{' src` → 44 lines; minus the struct definition
(`backend/mod.rs:115`) and 7 fn-signature braces (`agent.rs:716`, `ls_cli.rs:745`,
`operations.rs:4987`, `operations.rs:5097`, `tmux.rs:2135`, `tmux.rs:2143`, `wez.rs:2936`) = **36
literals = 23 production + 13 under `#[cfg(test)]`**. No clone-then-mutate exists, no `Default`
impl, no macro construction (verified by grep).

## Production sites — all 23, complete

| # | site | epoch source | class | why |
|---|---|---|---|---|
| 1 | `adopt_cli.rs:235-239` | `backend_server(i)?.server_epoch` | **unverified** | instance + endpoint `ok_or_else`'d; only the epoch may be absent |
| 2 | `connect_cli.rs:1164-1168` | `.ok_or_else(..)` at 1095 | verified | + `verify_epoch` w/ pid+start_token, + ambient `$TMUX` triple cross-check |
| 3 | `gui_cli.rs:1389-1393` | non-`Option` param | verified | all 5 callers source it from a fails-closed read |
| 4 | `gui_cli.rs:1445-1449` | `backend_server(i)?.server_epoch` | **unverified** | unregistered/wrong-kind/no-endpoint all refuse first |
| 5 | `gui_lifecycle.rs:1004-1008` | descriptor epoch | verified | registry mismatch is `fatal` at 972; socket/pid/token/dev/ino checked |
| 6 | `ls_cli.rs:746-750` | `ManagedScope.epoch: ServerEpoch` | verified | 493e92c; type forbids the NULL |
| 7 | `ls_cli.rs:843-847` | literal `None` | **intentionally-unmanaged** | only reachable when `backend_instance_for_backend` = `None`; tmux only |
| 8 | `main.rs:1450-1454` | literal `None` | **unverified** | managed subject via `DMUX_SPACE_UID`; endpoint also unchecked |
| 9 | `main.rs:1460-1464` | `verified_wez_target` | verified | in-function foil to #8 |
| 10 | `migrate_cli.rs:740-744` | `backend_server(i)?.server_epoch` | **unverified** | `Unregistered`/`Unaddressable` already peeled off |
| 11 | `new_cli.rs:377-381` | `backend_server(i)?.server_epoch` (362) | **unverified** | `Ok(None)` for unregistered, error for no-endpoint |
| 12 | `remote/agent.rs:717-721` | non-`Option` `Target.epoch` | verified | normally from `verified_wez_target`; see #20 for the nil hazard |
| 13 | `remote/agent.rs:881-885` | `ready_wez_identity` | verified | result re-compared at 899 |
| 14 | `remote/agent.rs:1278-1282` | literal `None` | **unverified** | instance from `find_instance`; fence taken; epoch never read |
| 15 | `remote/agent.rs:1314-1318` | `ready_wez_identity` | verified | in-function foil to #14 |
| 16 | `remote/agent.rs:1414-1418` | `backend_server(i)?.server_epoch` | **unverified** | `find_instance` + endpoint `ok_or_else`'d |
| 17 | `rm_cli.rs:1112-1116` | `FrozenConnectTarget.server_epoch` | verified | producer `ok_or_else`s at 779-786 |
| 18 | `rm_cli.rs:1142-1146` | `backend_server(i)?.server_epoch` | **unverified** | `Ok(None)` for unregistered and for no-endpoint |
| 19 | `space_cli.rs:230-234` | `None` on `--socket`, else `Some` | **operator-unmanaged** | `#[arg(long, hide=true)]` "Test seam"; residual in [Refuted findings and false leads](07-refuted-and-false-leads.md) |
| 20 | `space_cli.rs:628-635` | `.ok().and_then(..)` | **unverified** | also swallows `RegistryError`; unique in the crate |
| 21 | `space_cli.rs:643-647` | `verified_wez_target` | verified | in-function foil to #20 |
| 22 | `space_cli.rs:1159-1163` | literal `None` | **unverified** | Active Space + FK instance; epoch available, never fetched |
| 23 | `space_cli.rs:1171-1175` | `verified_wez_target` | verified | in-function foil to #22 |

**Totals: 11 verified, 9 unverified-managed (1, 4, 8, 10, 11, 14, 16, 18, 20, 22 — ten entries, of
which #22 and #8 are literal `None`s and #14 is a literal `None`), 2 legitimately unmanaged
(7, 19), 1 fixed by 493e92c (6).** Precisely: 6 launder a registry `Option` (1, 4, 10, 11, 16, 18),
1 launders *and* swallows the error (20), 3 hardcode `None` against a managed subject (8, 14, 22).

## The consumption side — where `None` becomes "skip"

`if let Some(expected)` sinks: `wez.rs:1113` (scan), `wez.rs:1184` (`binding_epoch`),
`tmux.rs:478` (`binding_epoch`), `tmux.rs:598` (`read_markers`), `tmux.rs:1521` (inventory),
`operations.rs:640` (`scan_epoch_for_create`), `operations.rs:2223` (unreachable, see [Refuted findings and false leads](07-refuted-and-false-leads.md)),
`new_cli.rs:663` (guarded by `if let Some(witness)` and `backend == Wez`).
Fail-closed sinks: `wez.rs:1271` (4 callers only), `tmux.rs:466` (13 callers),
`operations.rs:1233`, `1348`, `2766`.

## Test sites — 13 in `src` `#[cfg(test)]`, 27 in `tests/`

Builders that default to `None`: `tests/adopt_flow.rs:306` (2 tests), `tests/normalize_flow.rs:96`
(3 tests), `tests/hierarchy_flow.rs:513` (1 test, which explicitly waits for
`inv.server_epoch.is_some()` and then declines to pin it),
`tests/remote_protocol/wez_agent.rs:156` (a registered-but-unpublished instance asserted to scan
`Complete` — the exact state 493e92c forbids). Parameterised builders passing `None` against
sentinel-bearing canned servers: `wez.rs:2936` (49 call sites / 42 tests, 24 sentinel-bearing),
`tmux.rs:2135` (14 sites). Clean, non-`Option`-parameter builders: `tests/hierarchy_flow.rs:74`,
`tests/operations_flow.rs:76`, `tests/space_rm_cli.rs:512`, `tests/recovery/coordinator.rs:253`,
`tests/provider_wez.rs:222/226`, and all 11 in `operations.rs`'s test module.
`tests/provider_contract.rs:82` is the worst of them: the shared contract double reimplements
`expected_epoch.is_some() && …`, promoting "None skips verification" from adapter accident to
documented Provider contract.
