# What remains untested, and why

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`.

---

This section is exhaustive and unhedged.

1. **No production `dmux` verb was ever executed against the live registry.** Every repro used a
   scratch `XDG_DATA_HOME` copy, a scratch `-L` namespace under a private `TMUX_TMPDIR`, or a
   library-level driver linked against the crate. The live file's sha256 is `88835a1a…` before and
   after every run in every session.
2. **`DMUX_WEZ_FIRST` was never set**, by anyone. `adopt`, `migrate`, `new`, `rm`, `ls --format
   json`, `--row` and `--tree` are all gated behind it (`main.rs:1724` `WEZ_FIRST_BY_DEFAULT =
   false`). Every repro of a gated verb therefore called the *library entry point the gate
   dispatches to* — `dmux::adopt_cli::adopt(..)`, `migrate_in(..)`, `ls_cli::render(..)`,
   `rm_cli::remove(..)` — never the gated dispatch. The gate itself is a two-env-var boolean
   (`main.rs:1730-1776`) read but not executed in the `true` state.
3. **No managed mux server was stopped, started, kickstarted or contacted for writing.** Plan §21
   forbids it. Consequences: the wez production NULL window (`dmux-mux-start.sh:106` registers,
   `recovery.rs:3507` publishes) was never observed live — it was reproduced by driving the *tmux*
   equivalent (`_tmux-bootstrap` against a dead namespace, which really does leave
   `tmux|nosuchsrv|||`) and by SQL on a copy. Its wall-clock width is unmeasured.
4. **No real replacement wez server.** `wez.rs:2806 cas_rename_workspace` and `wez.rs:2744
   sole_window_id` were exercised against stubs and canned runners, never against a genuine second
   `wezterm-mux-server` on the managed socket. The frozen fork CAS stderr shapes were reproduced
   from `wez.rs:504-560 classify_cas_rename`, not observed from a fork build.
5. **The remote/peer half is untested.** `remote/agent.rs:1281` was proven live on the *owner* side
   (a replacement tmux server answered `outcome:"complete"`), but no second enrolled host exists, so
   `dmux ls <peer>` rendering those rows as `live` (`ls_cli.rs:1148`) is proven only by reading. No
   SSH route, no `--host`, no peer capability negotiation was exercised.
6. **State D (unpublished + exclusive lock) was constructed synthetically.** The A/B that proves
   ls cannot distinguish C from D used a purpose-built lock-holder taking `BackendInstance`
   exclusive through dmux's own `locks` module. A real coordinator mid-flight was never observed;
   that needs a mux restart.
7. **Nine confirmed findings have call-chain proof only**, and are the likeliest to be wrong:
   `operations.rs:2243`, `operations.rs:2277`, `operations.rs:133`, `registry/mod.rs:2812`,
   `space_cli.rs:222` → `normalize_apply`, `remote/agent.rs:1405`, five of the six
   `binding_epoch`-fenced tmux verbs (only `group_new` was proven live), `wez.rs:2806`/`:2744`, and
   the three out-of-crate shell/Lua findings (`94-dmux-context.zsh:216`,
   `91-tmux-attach.zsh:45`, `controller.lua:115`) whose sweep ran nothing at all.
8. **Five items were capped out of adversarial verification entirely** and are neither confirmed nor
   refuted: `ls_cli.rs:850/857` (addressability checked before epoch, so a doubly-unpublished
   instance reports the wrong fault class; and the epoch is read before the fence is taken),
   `gui_lifecycle.rs:880` (the doc comment's justification for `fatal` is wrong even though the
   conclusion holds), `tests/remote_protocol/route_matrix.rs:1` (all 9 tests use `hello`; the string
   "epoch" does not occur in the file), `registry/mod.rs:1586` (the "stopped/never published" doc —
   **this one should be re-graded, it is the mechanism of finding #1**), and the HEAD discrepancy.
9. **Why the live descriptor is frozen at `starting` rather than `failed` is unknown.** The sentinel
   *was* spawned (the live `wezterm cli list` shows it), so `dmux-mux.lua:1140-1146` ran, yet the
   `DMUX_BACKEND_INSTANCE` guard at `:1148-1160` that should publish `failed` evidently did not
   land. I did not read the launchd log; doing so would not change any crate-level finding.
10. **Not audited at all:** the Lua GUI bridge beyond the three flagged lines; whether the deployed
    `~/.local/bin/dmux` behaves as reviewed (it *contains* the fix — `strings` finds
    `"has published no server epoch"`, mtime Aug 20 00:45, post-`493e92c` — but was not run through
    the gated paths); `cargo test -p dmux` as a whole under the default parallel harness (only
    `--test-threads=1` was run, twice, 984/0/1); and any property or fuzz testing, which does not
    exist in this crate (`grep -rn 'proptest|quickcheck|fuzz'` finds only prose).
