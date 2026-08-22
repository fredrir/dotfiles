# Suggested next actions

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`.

---

1. **Deal with the live host first.** The registry publishes a dead incarnation and the descriptor
   has been `starting` for three days. Re-grade `registry/mod.rs:1586` from info to critical, and
   add a liveness re-check: nothing invalidates a published incarnation whose pid has exited.
2. **Give `dmux doctor` an epoch probe** comparing `backend_server(instance)`'s
   {epoch, pid, start_token, dev, ino} against the live descriptor and a `stat` of the socket. Until
   it exists, both epoch remedies in the crate terminate in a green report that proves nothing.
3. **Fix the two unachievable remedies.** `ls_cli.rs:1193-1200` must branch on the fence before
   telling an operator to restart a mux that is bootstrapping; `gui_lifecycle.rs:977-984` must not
   tell an operator to restart a service that structurally cannot republish. Both have correct
   wording available 20 lines away.
4. **Land the fix shape, steps 1-2** (move the type; private field + constructors). One commit,
   61 edit points, provably semantics-free, and it turns `grep` into a working audit.
5. **Land `resolve_managed` and migrate the 9 launder sites, one commit each**, highest-blast-radius
   first: `adopt_cli.rs:238`, `migrate_cli.rs:743`, `space_cli.rs:631`, `space_cli.rs:1162`,
   `remote/agent.rs:1281`, then the reads.
6. **Close the wez mutation fence independently** — nine verbs, starting with `create` (2049),
   `cas_rename_workspace` (2814) and `split_new` (2445). Add the wez analogue of
   `create_on_unepoched_server_is_a_typed_error`, which tmux has and wez does not.
7. **Fix `main.rs:1450` by hand** (step 5 of the migration): add the endpoint and epoch comparisons
   from `operations::validate_marker_context`.
8. **Review or gate `src/list.rs` and `src/attach.rs`.** They are the default path, they violate all
   three ADR-001 rules, and they will `activate-pane` into the reserved sentinel. At minimum pin
   `WEZTERM_UNIX_SOCKET`, add a deadline, and filter `WEZ_SENTINEL_PREFIX`.
9. **Populate tmux `socket_dev`/`socket_ino`** at `operations.rs:133` (the values are already read
   and discarded at `tmux.rs:392-397`) and compare them — or delete the columns for tmux and say so.
10. **Add `server_epoch` to `BINDING_COLUMNS`** and make `binding_epoch` compare the registry value
    rather than the scan-minted one, which collapses the `operations.rs:2277` tautology.
11. **Fix `tests/provider_contract.rs:82`** so the reference implementation stops teaching new
    adapters that `None` means skip, and add the missing coverage: no test anywhere asserts that a
    managed *read* refuses on `None`, and no `ls` test registers a tmux instance at all.
12. **Re-run the repro budget on the nine call-chain-only findings** in [What remains untested](08-untested.md) item 7, starting with
    `space_cli.rs:222` → `normalize_plan`/`normalize_apply` and the five unproven
    `binding_epoch` verbs.
13. **Update the stale P1 doc comment** at `backend/mod.rs:119-120`, which has described
    `expected_epoch` as "the pin a caller already holds" since `cb780bd` and is the root of the
    drift.
