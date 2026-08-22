# The fence and the instance state machine

Part of the [dmux epoch-verification integrity review](INDEX.md). Reviewed tree `493e92c`. Answers review-ask #5 (test the fence interaction).

---

Six distinguishable states of a `backend_instances` row. `ls` collapses two of them.

| state | registry | live server | fence | currently distinguished? | safe operator advice |
|---|---|---|---|---|---|
| A. not registered | no row | any/none | n/a | yes — `ScanTarget::Unregistered` | "nothing enrolled; `dmux adopt` or bootstrap" |
| B. registered, no endpoint | `socket_path` NULL | n/a | n/a | yes — `Unaddressable` | "re-register the instance" |
| C. registered, unpublished, idle | epoch NULL | may be up | free | **no** — merged with D | "run `_tmux-bootstrap` / wait for the mux to coordinate" |
| D. registered, unpublished, **exclusive lock held** | epoch NULL | starting | held excl. | **no** — merged with C | "a bootstrap/recovery is in flight; wait, re-run `dmux ls`" |
| E. published, live agrees | epoch `E` | `E` | shared ok | yes — `Managed`, scan succeeds | — |
| F. published, live disagrees **or process dead** | epoch `E` | `F`/dead | shared ok | partially — scan says `backend_epoch_changed`; a dead pid is **never** detected | "the published incarnation is stale; republish" — but see below |

**The C/D collapse is the confirmed defect.** `ScanTarget::instance()` (`ls_cli.rs:781-786`) returns
`None` for `Unpublished`, so that instance never enters `fenced` (911-925) and the scan arm
(940-942) never consults it. `unpublished_detail` (1193-1200) then emits *"restart the managed mux
service"* unconditionally. I confirmed the two states are otherwise indistinguishable by A/B with an
identical held exclusive lock: NULL → the restart advice; published → *"backend instance is
recovering or mutating"*. The window is real and is exactly a first bootstrap:
`operations.rs:88` registers, `:96-98` takes the exclusive lock, `:129` publishes; the wez path is
`recovery.rs:1892` registers, `:3316` locks, `:3507` publishes. Both distinguishers ls needs are
already available and unused — the same non-blocking shared `try_acquire` it takes for `Managed`,
and `Registry::current_lease(&LeaseScope::Recovery(instance))`. The correct wording already exists
17 lines above at `ls_cli.rs:933-935`.

**Two corrections to how this was reported.** (a) It does *not* bypass a safety fence: an
`Unpublished` target is never probed, so not holding the lock reads nothing. The harm is purely the
misdiagnosis and the destructive remedy. (b) A *journaled restore* cannot be in flight in state D,
because `begin_recovery` calls `require_published_epoch` (`registry/recovery.rs:625`) which hard-errors
on NULL. What a restart destroys is a coordinator between `InstanceLeaseGuard::acquire`
(`recovery.rs:1814`) and `publish_incarnation_if_needed` (`recovery.rs:2673`) — a held
`recovery:<uid>` lease for a killed pid, forcing the next coordinator through TakeoverProof.

**State F is the one nothing models.** The published epoch is write-once-and-never-invalidated
(`registry/mod.rs:1551` is the sole writer; no NULL-restore, no `DELETE`, no liveness check), so a
row naming a dead pid is permanent and every reader treats it as authoritative-and-wrong. On this
host that makes the advice in row F unachievable rather than merely useless: with `DMUX_WEZ_FIRST`
absent from the launchd job environment, `dmux-mux.lua:1148-1160` can only ever publish `failed`,
never `ready`, so no restart can republish. That is the same unachievable-remedy shape as C/D, in a
second place (`gui_lifecycle.rs:977-984`), and 493e92c made that path **non-retryable**.
