# The tmux answer

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`. Answers review-ask #4 (check the tmux side specifically).

---

**The prior reviewer's sentence — "no managed-tmux read anywhere in the CLI is epoch-verified" — is
FALSE.** Three counter-examples, each verified by reading the epoch's provenance rather than
pattern-matching:

1. `connect_cli.rs:1095` refuses a NULL with `"managed tmux instance has no published epoch"`, then
   builds a `TmuxServerIdentity` from the registry pid + start_token, calls
   `provider.verify_epoch` (1122), cross-checks the ambient
   `#{server_pid}|#{pane_id}|#{@dmux_server_epoch}` triple (1128-1160), and only then pins
   `expected_epoch: Some(epoch)` at 1167. This is the model implementation.
2. `rm_cli.rs:1115` pins `Some(target.server_epoch)`, a non-`Option` field whose only producer
   `ok_or_else`s the same registry read at 779-786. `target.backend` can be `Tmux`.
3. `space_cli.rs:646` pins the tmux reconcile scope, and `space_cli.rs:1774-1802` is a dedicated
   regression test asserting it (`cargo test -p dmux --bin dmux -- reconcile_scope` passes).

A fourth exists outside the CLI proper: `remote/attach.rs:1642` requires
`published.server_epoch == Some(record.server_epoch)`, requires non-NULL pid *and* start token, and
calls `verify_epoch` — the strictest managed-tmux read in the tree, and it additionally does the
endpoint check (`attach.rs:1632`) that `context_read` omits.

**The accurate statement is narrower:** *every managed-tmux read that builds its scope ad hoc —
from a bare `backend_server(..).server_epoch` or from a literal `None` — is unverified.* That is
`space_cli.rs:1162`, `main.rs:1453`, `remote/agent.rs:1281`, and the tmux arms of
`adopt_cli.rs:238`, `migrate_cli.rs:743`, `rm_cli.rs:1145`, `new_cli.rs:380`, `gui_cli.rs:1448`,
`space_cli.rs:631`, `agent.rs:1417`.

**On the two providers' relative strictness, the brief has it backwards.** tmux requires the scope
epoch on 13 methods including the read `split_list` (`tmux.rs:1903`), and pins three more to the
binding. wez requires it on **four**. Where tmux loses verification it is the caller's doing; where
wez loses it, it is the adapter's. The one thing wez has that tmux lacks is a structural floor: the
sentinel handshake (`wez.rs:1093-1108`) refuses a missing or duplicate `dmux:system:<epoch>`
*before* consulting `expected_epoch`, so an unpinned wez scan still rejects a non-dmux server. tmux
has no floor — but that is the frozen spec (plan §11.2 L639-640: *"`ls` never sets an option: if the
hook has not run, it lists sessions as `unmanaged:unepoched`"*), implemented at `inventory.rs:218`
and rendered at `output.rs:297`. That is why the "tmux has no sentinel equivalent" finding is
refuted ([Refuted findings and false leads](07-refuted-and-false-leads.md)) while the caller-side tmux findings stand.
