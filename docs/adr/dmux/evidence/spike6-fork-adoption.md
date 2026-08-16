# Spike 6 — WezTerm fork primitive for atomic Wez workspace adoption (P0, plan §10.3 / §18 P3c)

Date: 2026-08-16
Worktree: `/Users/fredrir/packages/wezterm-dmux-p0`, branch `dmux-p0-spike`, base rev `9e6323bb5`
Spike commit: `d045ed94a491ab6862ec79316dd02b7bba92f264` (local only, never pushed)

## Verdict

**FEASIBLE — implemented, built, and demoed live.** A window-id-scoped, owner-server-side
compare-and-swap rename (`RenameWorkspaceIf`) closes the check-then-rename TOCTOU with a
~210-line additive diff over 5 files. Every failure mode is a typed outcome that guarantees
no mutation occurred. The plan's "rename-if-source-generation-and-epoch" maps cleanly onto
`(window_id, expected workspace name, sole-window assertion)` checked atomically server-side,
with epoch verification remaining dmux-side (bracketing the call), which is sufficient — see
"Generation/epoch mapping" below.

## Selected primitive shape

CLI (spike build):

```
wezterm cli rename-workspace --window-id N --if-workspace OLD [--if-sole-window] NEW
```

- `--window-id` requires `--if-workspace`; conflicts with `--workspace`/`--pane-id`
  (the unconditional legacy paths are untouched).
- Exit 0 + silence on success. Exit 1 with a stable stderr prefix on typed failure:
  - `rename-workspace-if failed: workspace_mismatch window_id=N actual="..."`
  - `rename-workspace-if failed: no_such_window window_id=N`
  - `rename-workspace-if failed: not_sole_window window_id=N other_window_ids=[...]`

Wire protocol (codec crate, idents 63/64, `CODEC_VERSION` 45 → 46):

```rust
pub struct RenameWorkspaceIf {
    pub window_id: WindowId,          // usize
    pub expected_workspace: String,   // CAS expectation
    pub new_workspace: String,        // opaque dmux key on adoption
    pub expect_sole_window: bool,     // also require window_id is the only window in expected_workspace
}

pub enum RenameWorkspaceCasOutcome {
    Renamed,                                          // precondition held; renamed exactly once
    NoSuchWindow,                                     // no mutation
    WorkspaceMismatch { actual: String },             // no mutation
    NotSoleWindow { other_window_ids: Vec<WindowId> },// no mutation
}

pub struct RenameWorkspaceIfResponse { pub outcome: RenameWorkspaceCasOutcome }
```

Server core (mux crate): `Mux::rename_workspace_for_window_if(window_id, expected, new, expect_sole_window) -> Result<(), WorkspaceCasError>` — existence check, expectation check, optional sole-window check, and the rename all under **one** `self.windows.write()` lock.

Semantics notes:
- Idempotent success when `expected == new` and the window is already there (`Window::set_workspace` no-ops on equal name — mux/src/window.rs:56-62).
- Idempotency recovery after an unknown outcome: a retry with the same arguments that returns
  `WorkspaceMismatch { actual == <dmux opaque key> }` positively proves the first attempt applied
  (opaque keys are reserved, unique, never guessed) — clean fit for the plan's
  "unknown mutation outcome may be retried only with the same request UID".
- In sole-window mode, clients whose `active_workspace` was the old name are retargeted to the
  new name (mirrors `Mux::rename_workspace` semantics, since the name provably fully migrated).
  In non-sole mode this is deliberately skipped, matching `SetWindowWorkspace` semantics.

## Code map (rev 9e6323bb5, pre-patch line refs unless noted)

Workspace concept on mux windows:
- `mux/src/window.rs:6-7` — `static WIN_ID: AtomicUsize; pub type WindowId = usize;` window ids are process-lifetime monotonic, never reused within one server process (fresh ids per server epoch).
- `mux/src/window.rs:14,36-38` — `Window.workspace: String`, `get_workspace()`.
- `mux/src/window.rs:56-62` — `Window::set_workspace`: no-op on equal name, else assigns and emits `MuxNotification::WindowWorkspaceChanged(id)` (notify while caller may hold windows write lock — established codebase pattern).

Existing rename-workspace flow end-to-end (unconditional, racy for adoption):
- CLI: `wezterm/src/cli/rename_workspace.rs` — `wezterm cli rename-workspace [--workspace W|--pane-id P] NEW`; resolves old name from a client-side `list_panes` snapshot (the exact TOCTOU the plan forbids), then sends `codec::RenameWorkspace`.
- CLI registration: `wezterm/src/cli/mod.rs:17,161,200`.
- Client rpc: `wezterm-client/src/client.rs:1384` — `rpc!(rename_workspace, RenameWorkspace, UnitResponse)`.
- PDU: `codec/src/lib.rs:500` (`RenameWorkspace: 58`), struct at `codec/src/lib.rs:779-783`.
- Server handler: `wezterm-mux-server-impl/src/sessionhandler.rs:417-432` → `Mux::rename_workspace`.
- Mux: `mux/src/lib.rs:652-674` — `rename_workspace(old, new)`: renames **every** window whose workspace == old (whole-name rename), fixes client active workspaces, notifies.

Existing window-scoped unconditional primitive (the CAS's closest relative):
- `codec/src/lib.rs:485,773-777` — `SetWindowWorkspace { window_id, workspace }` (ident 43).
- `wezterm-mux-server-impl/src/sessionhandler.rs:279-297` — handler: `mux.get_window_mut(window_id)` + `set_workspace`; no precondition → unconditional, hence insufficient for adoption.
- `wezterm-client/src/client.rs:1378`, used by GUI domain sync at `wezterm-client/src/domain.rs:262,324`.

Codec / PDU machinery:
- `codec/src/lib.rs:444` — `CODEC_VERSION: usize = 45` ("must be bumped when backwards incompatible changes are made").
- `codec/src/lib.rs:450-507` — `pdu!` macro registration `Name: ident`; decode maps unknown idents to `Pdu::Invalid { ident }` (additive changes are wire-safe in decode).
- Manual `impl Pdu` blocks (`is_user_input`, `pane_id`) have `_ =>` catch-alls; the **server's** `process_one` match is exhaustive (response-type PDUs enumerated at `sessionhandler.rs:1028-1053`) — adding a response PDU requires adding it there (caught by `cargo check`).
- Version handshake: `wezterm-client/src/client.rs:1148-1176` `verify_version_compat` — **exact equality** `info.codec_vers == CODEC_VERSION` or hard error (`IncompatibleVersionError`). No capability negotiation exists.
- SSH path: `wezterm/src/cli/proxy.rs:16-60` — `wezterm cli proxy` is a raw byte pump after its initial `SetClientId`; new PDUs traverse proxied connections untouched.

## PDU dispatch atomicity analysis (citations)

The headless mux server executes **every** PDU handler serially on a single thread; a handler
closure runs to completion before any other client's PDU (or any other handler) runs:

1. `wezterm-mux-server/src/main.rs:79` — `config::designate_this_as_the_main_thread()`;
   `:231` — `let executor = promise::spawn::SimpleExecutor::new();`;
   `:248-250` — the entire process main loop is `loop { executor.tick()?; }`.
2. `promise/src/spawn.rs:186-224` — `SimpleExecutor`: one unbounded channel of `SpawnFunc`;
   `set_schedulers` routes both the normal and low-priority main-thread queues into that same
   channel; `tick()` receives **one** task poll at a time and runs it on the sole main thread.
3. `promise/src/spawn.rs:126-137` — `spawn_into_main_thread` schedules the future's polls onto
   that channel (from any thread).
4. `wezterm-mux-server-impl/src/local.rs:20-31` — each accepted unix-socket connection's whole
   `dispatch::process(stream)` future is itself spawned into the main thread; so even PDU
   decode and `process_one` run on the main thread, and connections interleave only at
   `.await` points.
5. `wezterm-mux-server-impl/src/dispatch.rs:67-86` — per-connection loop: read one PDU
   (`Pdu::decode_async`), then `handler.process_one(decoded)` (line 85).
6. `wezterm-mux-server-impl/src/sessionhandler.rs:247-297,417-432` — every mutating arm is
   `spawn_into_main_thread(async { catch(move || { /* synchronous closure, zero awaits */ },
   send_response) })`. A zero-await closure completes within a single `tick()`, mutually
   excluded from every other handler closure and every other connection's read loop.

Conclusion: **a single PDU's handler is atomic w.r.t. all other clients' PDUs** on the
headless server. The same property holds in the GUI-hosted mux endpoint (the GUI serves the
same `wezterm-mux-server-impl` dispatch/sessionhandler on its gui sock, with
`spawn_into_main_thread` targeting the GUI main-thread executor, which also polls one task at
a time).

Belt-and-suspenders: the spike primitive does not rely solely on executor serialization —
`Mux::rename_workspace_for_window_if` (post-patch `mux/src/lib.rs:687-752`) performs all
checks **and** the rename under one `self.windows.write()` (parking_lot RwLock) critical
section, so even a hypothetical off-main-thread window mutator could not interleave.
Notify-under-write-lock is the established pattern (`Mux::rename_workspace`
mux/src/lib.rs:661-665; `SetWindowWorkspace` handler holding a `MappedRwLockWriteGuard`
through `set_workspace`, sessionhandler.rs:279-297).

## Generation/epoch mapping (plan §10.3 "rename-if-source-generation-and-epoch")

- **epoch** — the server has no native epoch concept; `WindowId` is monotonic per server
  process (mux/src/window.rs:6-7), so `(backend server epoch, window_id)` is a stable native
  ref. Epoch verification stays **dmux-side**: verify runtime descriptor / socket identity /
  sentinel epoch nonce before and after the CAS call, per §8.1/§15.1. A server restart
  resets window ids; dmux detects it as `backend_epoch_changed` and discards the plan.
- **source generation (name/window-set)** — checked **server-side atomically**:
  `window_id` existence + `expected_workspace` equality + `expect_sole_window` (no other
  window carries the source name at execution time). This is *stronger* than the plan's
  minimum (single-window check dmux-side under the exclusive instance lock would already be
  acceptable); with `--if-sole-window` the window-count race is closed inside the atomic
  section itself, so a concurrently created second window in the source workspace makes the
  CAS fail typed rather than being caught only by the post-rename re-scan.
- **explicit conclusion**: no per-window generation counter needs to be added to the fork.
  The tuple (window_id + expected-workspace + sole-window) is the entire mutable source
  state relevant to adoption identity; pane/tab-set drift is covered by the plan's
  before/after one-window re-scan and does not affect rename identity.

## Codec versioning / compatibility story

- Adding a PDU: register `Name: ident` in `pdu!` (codec/src/lib.rs:450-507), define the
  serde struct(s), add a server handler arm, add the response variant to the server's
  "expected a request" arm (sessionhandler.rs:1028-1053, exhaustive match), add
  `rpc!` on the client. Decode of unknown idents degrades to `Pdu::Invalid`, so the change
  is additive on the wire.
- Version gate: exact-match `CODEC_VERSION` at connect (client.rs:1148-1176). Spike bumps
  45 → 46, so a spike CLI **fails closed** against the installed 45-codec server and vice
  versa — desirable strictness per plan §8.1. Both hosts run the same fork; lockstep upgrade
  is available and required. (Choosing *not* to bump would keep old-GUI ↔ new-server working
  during a staggered rollout at the cost of a silent capability gap; rejected for the spike.)
- `wezterm cli proxy` forwards raw bytes → conditional rename works across SSH-proxied
  connections with no extra work, provided both ends are the same fork build.
- No capability negotiation mechanism exists to make the verb optional; exact version
  equality is the only gate. Fine for a lockstep local fork.

## Patch

Commit `d045ed94a` on `dmux-p0-spike` — `git show --stat`:

```
commit d045ed94a491ab6862ec79316dd02b7bba92f264
Author: fredrir <fhansteen@gmail.com>
Date:   Sun Aug 16 13:48:06 2026 +0200

    dmux P0 spike: atomic conditional workspace rename (RenameWorkspaceIf CAS)
    ...
 codec/src/lib.rs                              | 42 ++++++++++++++-
 mux/src/lib.rs                                | 75 +++++++++++++++++++++++++++
 wezterm-client/src/client.rs                  |  5 ++
 wezterm-mux-server-impl/src/sessionhandler.rs | 39 ++++++++++++++
 wezterm/src/cli/rename_workspace.rs           | 49 +++++++++++++++++
 5 files changed, 209 insertions(+), 1 deletion(-)
```

Full diff (also at `../spike6/spike-patch.diff`):

```diff
diff --git a/codec/src/lib.rs b/codec/src/lib.rs
index 7869a47bd..e8620414c 100644
--- a/codec/src/lib.rs
+++ b/codec/src/lib.rs
@@ -441,7 +441,7 @@ macro_rules! pdu {
 /// The overall version of the codec.
 /// This must be bumped when backwards incompatible changes
 /// are made to the types and protocol.
-pub const CODEC_VERSION: usize = 45;
+pub const CODEC_VERSION: usize = 46;
 
 // Defines the Pdu enum.
 // Each struct has an explicit identifying number.
@@ -502,6 +502,8 @@ pdu! {
     GetPaneDirection: 60,
     GetPaneDirectionResponse: 61,
     AdjustPaneSize: 62,
+    RenameWorkspaceIf: 63,
+    RenameWorkspaceIfResponse: 64,
 }
 
 impl Pdu {
@@ -781,6 +783,44 @@ pub struct RenameWorkspace {
     pub new_workspace: String,
 }
 
+/// Conditionally rename the workspace of a single window:
+/// a compare-and-swap that succeeds only if the window's current
+/// workspace matches `expected_workspace` at the moment the server
+/// executes the request. Used to close check-then-rename races
+/// between multiple mux clients.
+#[derive(Deserialize, Serialize, PartialEq, Debug)]
+pub struct RenameWorkspaceIf {
+    pub window_id: WindowId,
+    /// The workspace the window is expected to be in
+    pub expected_workspace: String,
+    /// The workspace name to assign if the expectation holds
+    pub new_workspace: String,
+    /// When true, additionally require that `window_id` is the only
+    /// window in `expected_workspace` at execution time
+    pub expect_sole_window: bool,
+}
+
+/// Typed outcome of a `RenameWorkspaceIf` request.
+/// Any variant other than `Renamed` guarantees that no mutation
+/// was performed.
+#[derive(Deserialize, Serialize, PartialEq, Debug)]
+pub enum RenameWorkspaceCasOutcome {
+    /// The precondition held; the window now has the new workspace name
+    Renamed,
+    /// No window with that id exists
+    NoSuchWindow,
+    /// The window exists but is in a different workspace
+    WorkspaceMismatch { actual: String },
+    /// `expect_sole_window` was requested but other windows share
+    /// the expected workspace
+    NotSoleWindow { other_window_ids: Vec<WindowId> },
+}
+
+#[derive(Deserialize, Serialize, PartialEq, Debug)]
+pub struct RenameWorkspaceIfResponse {
+    pub outcome: RenameWorkspaceCasOutcome,
+}
+
 /// This is used both as a notification from server->client
 /// and as a configuration request from client->server when
 /// the client's preferred configuration changes
diff --git a/mux/src/lib.rs b/mux/src/lib.rs
index dbaf123a3..9eeb50081 100644
--- a/mux/src/lib.rs
+++ b/mux/src/lib.rs
@@ -674,6 +674,72 @@ impl Mux {
         }
     }
 
+    /// Compare-and-swap rename of a single window's workspace.
+    ///
+    /// Atomically (under a single write lock over the window map, and,
+    /// in practice, serialized on the mux main thread with every other
+    /// PDU handler) verifies that `window_id` exists and currently has
+    /// workspace `expected_workspace`, then assigns `new_workspace`.
+    /// When `expect_sole_window` is true it additionally requires that
+    /// no other window shares `expected_workspace`.
+    ///
+    /// On any error, no mutation has been performed.
+    pub fn rename_workspace_for_window_if(
+        &self,
+        window_id: WindowId,
+        expected_workspace: &str,
+        new_workspace: &str,
+        expect_sole_window: bool,
+    ) -> Result<(), WorkspaceCasError> {
+        {
+            let mut windows = self.windows.write();
+
+            let actual = match windows.get(&window_id) {
+                Some(window) => window.get_workspace().to_string(),
+                None => return Err(WorkspaceCasError::NoSuchWindow),
+            };
+            if actual != expected_workspace {
+                return Err(WorkspaceCasError::WorkspaceMismatch { actual });
+            }
+
+            if expect_sole_window {
+                let other_window_ids: Vec<WindowId> = windows
+                    .values()
+                    .filter(|w| {
+                        w.window_id() != window_id && w.get_workspace() == expected_workspace
+                    })
+                    .map(|w| w.window_id())
+                    .collect();
+                if !other_window_ids.is_empty() {
+                    return Err(WorkspaceCasError::NotSoleWindow { other_window_ids });
+                }
+            }
+
+            windows
+                .get_mut(&window_id)
+                .expect("checked above under the same write lock")
+                .set_workspace(new_workspace);
+        }
+
+        self.recompute_pane_count();
+
+        if expect_sole_window {
+            // The expected workspace name is proven to have fully migrated
+            // to `new_workspace`, so retarget clients that were following
+            // the old name, mirroring `rename_workspace` semantics.
+            for client in self.clients.write().values_mut() {
+                if client.active_workspace.as_deref() == Some(expected_workspace) {
+                    client.active_workspace.replace(new_workspace.to_string());
+                    self.notify(MuxNotification::ActiveWorkspaceChanged(
+                        client.client_id.clone(),
+                    ));
+                }
+            }
+        }
+
+        Ok(())
+    }
+
     /// Overrides the current client identity.
     /// Returns `IdentityHolder` which will restore the prior identity
     /// when it is dropped.
@@ -1408,6 +1474,15 @@ impl Mux {
     }
 }
 
+/// Failure modes of `Mux::rename_workspace_for_window_if`.
+/// Every variant guarantees that no mutation was performed.
+#[derive(Debug, PartialEq)]
+pub enum WorkspaceCasError {
+    NoSuchWindow,
+    WorkspaceMismatch { actual: String },
+    NotSoleWindow { other_window_ids: Vec<WindowId> },
+}
+
 pub struct IdentityHolder {
     prior: Option<Arc<ClientId>>,
 }
diff --git a/wezterm-client/src/client.rs b/wezterm-client/src/client.rs
index 9dc242062..1d71a3ee4 100644
--- a/wezterm-client/src/client.rs
+++ b/wezterm-client/src/client.rs
@@ -1382,6 +1382,11 @@ impl Client {
     rpc!(set_tab_title, TabTitleChanged, UnitResponse);
     rpc!(set_window_title, WindowTitleChanged, UnitResponse);
     rpc!(rename_workspace, RenameWorkspace, UnitResponse);
+    rpc!(
+        rename_workspace_if,
+        RenameWorkspaceIf,
+        RenameWorkspaceIfResponse
+    );
     rpc!(erase_scrollback, EraseScrollbackRequest, UnitResponse);
     rpc!(
         get_pane_direction,
diff --git a/wezterm-mux-server-impl/src/sessionhandler.rs b/wezterm-mux-server-impl/src/sessionhandler.rs
index 5c8e29577..41ba65715 100644
--- a/wezterm-mux-server-impl/src/sessionhandler.rs
+++ b/wezterm-mux-server-impl/src/sessionhandler.rs
@@ -431,6 +431,44 @@ impl SessionHandler {
                 .detach();
             }
 
+            Pdu::RenameWorkspaceIf(RenameWorkspaceIf {
+                window_id,
+                expected_workspace,
+                new_workspace,
+                expect_sole_window,
+            }) => {
+                spawn_into_main_thread(async move {
+                    catch(
+                        move || {
+                            use mux::WorkspaceCasError;
+                            let mux = Mux::get();
+                            let outcome = match mux.rename_workspace_for_window_if(
+                                window_id,
+                                &expected_workspace,
+                                &new_workspace,
+                                expect_sole_window,
+                            ) {
+                                Ok(()) => RenameWorkspaceCasOutcome::Renamed,
+                                Err(WorkspaceCasError::NoSuchWindow) => {
+                                    RenameWorkspaceCasOutcome::NoSuchWindow
+                                }
+                                Err(WorkspaceCasError::WorkspaceMismatch { actual }) => {
+                                    RenameWorkspaceCasOutcome::WorkspaceMismatch { actual }
+                                }
+                                Err(WorkspaceCasError::NotSoleWindow { other_window_ids }) => {
+                                    RenameWorkspaceCasOutcome::NotSoleWindow { other_window_ids }
+                                }
+                            };
+                            Ok(Pdu::RenameWorkspaceIfResponse(RenameWorkspaceIfResponse {
+                                outcome,
+                            }))
+                        },
+                        send_response,
+                    );
+                })
+                .detach();
+            }
+
             Pdu::WriteToPane(WriteToPane { pane_id, data }) => {
                 let sender = self.to_write_tx.clone();
                 let per_pane = self.per_pane(pane_id);
@@ -1010,6 +1048,7 @@ impl SessionHandler {
             | Pdu::MovePaneToNewTabResponse { .. }
             | Pdu::TabAddedToWindow { .. }
             | Pdu::GetPaneRenderableDimensionsResponse { .. }
+            | Pdu::RenameWorkspaceIfResponse { .. }
             | Pdu::ErrorResponse { .. } => {
                 send_response(Err(anyhow!("expected a request, got {:?}", decoded.pdu)))
             }
diff --git a/wezterm/src/cli/rename_workspace.rs b/wezterm/src/cli/rename_workspace.rs
index 03c3018fa..926fe7b93 100644
--- a/wezterm/src/cli/rename_workspace.rs
+++ b/wezterm/src/cli/rename_workspace.rs
@@ -18,12 +18,61 @@ pub struct RenameWorkspace {
     #[arg(long)]
     pane_id: Option<PaneId>,
 
+    /// Conditionally rename the workspace of just this window,
+    /// atomically on the server, and only if its current workspace
+    /// matches --if-workspace at execution time.
+    /// Requires --if-workspace.
+    #[arg(long, requires = "if_workspace", conflicts_with_all = ["workspace", "pane_id"])]
+    window_id: Option<mux::window::WindowId>,
+
+    /// The workspace the window identified by --window-id is expected
+    /// to be in. If the window is in a different workspace at the time
+    /// the server executes the request, nothing is renamed and the
+    /// command fails with a workspace_mismatch error.
+    #[arg(long, requires = "window_id")]
+    if_workspace: Option<String>,
+
+    /// Additionally require that --window-id is the only window in the
+    /// expected workspace at execution time.
+    #[arg(long, requires = "window_id")]
+    if_sole_window: bool,
+
     /// The new name for the workspace
     new_workspace: String,
 }
 
 impl RenameWorkspace {
     pub async fn run(self, client: Client) -> anyhow::Result<()> {
+        if let Some(window_id) = self.window_id {
+            let expected_workspace = self
+                .if_workspace
+                .clone()
+                .expect("clap enforces that --window-id requires --if-workspace");
+            let response = client
+                .rename_workspace_if(codec::RenameWorkspaceIf {
+                    window_id,
+                    expected_workspace,
+                    new_workspace: self.new_workspace,
+                    expect_sole_window: self.if_sole_window,
+                })
+                .await?;
+            use codec::RenameWorkspaceCasOutcome as Outcome;
+            return match response.outcome {
+                Outcome::Renamed => Ok(()),
+                Outcome::NoSuchWindow => anyhow::bail!(
+                    "rename-workspace-if failed: no_such_window window_id={window_id}"
+                ),
+                Outcome::WorkspaceMismatch { actual } => anyhow::bail!(
+                    "rename-workspace-if failed: workspace_mismatch \
+                     window_id={window_id} actual={actual:?}"
+                ),
+                Outcome::NotSoleWindow { other_window_ids } => anyhow::bail!(
+                    "rename-workspace-if failed: not_sole_window \
+                     window_id={window_id} other_window_ids={other_window_ids:?}"
+                ),
+            };
+        }
+
         let panes = client.list_panes().await?;
 
         let mut pane_id_to_workspace = HashMap::new();
```

## Build results

- `cargo check -p codec -p mux -p wezterm-mux-server-impl -p wezterm-client -p wezterm`
  — cold: **1:48.37 wall** (183s user, 231% CPU). One compile error caught: the server's
  `process_one` match over `Pdu` is exhaustive; added `Pdu::RenameWorkspaceIfResponse` to the
  "expected a request" arm. Re-check: **0.61s**, clean.
- `cargo check -p wezterm-mux-server -p wezterm-gui` — **47.2s**, clean (proves no other
  exhaustive-match sites break; GUI-hosted endpoint compiles with the new PDUs).
- `cargo build -p wezterm -p wezterm-mux-server` (debug) — **24.5s wall** (warm after the
  checks; 825% CPU). Binaries: `target/debug/wezterm`, `target/debug/wezterm-mux-server`.

## Live demo transcript

Scratch server from the spike build; user's live sockets untouched. Note: the absolute
scratch path exceeds macOS `SUN_LEN` (104), so the socket is addressed relatively (`d/s`)
with cwd pinned to the spike6 scratch dir — physically it lives at
`.../scratchpad/spike6/d/s`. Every CLI call uses
`env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=d/s wezterm --config-file
.../spike6/wezterm.lua cli --no-auto-start ...`. Server PID 79345, killed at the end;
`pgrep` confirmed no spike processes remained.

```
=== start scratch mux server (spike build) ===
server pid: 79345
socket: srwx-----T@ 1 fredrir  wheel  0 Aug 16 13:47 d/s

=== initial state ===
WINID TABID PANEID WORKSPACE SIZE  TITLE CWD                   
    0     0      0 default   80x24 sleep file:///Users/fredrir/
window_id=0 workspace=default

=== (a) CAS success: expectation matches ===
exit=0
WINID TABID PANEID WORKSPACE             SIZE  TITLE CWD                   
    0     0      0 dmux:sp:01TESTKEYAAAA 80x24 sleep file:///Users/fredrir/

=== (b) concurrent external rename, then CAS with stale expectation ===
--- external (unconditional) rename to 'external-moved' happens first:
exit=0
--- dmux's CAS still expects 'dmux:sp:01TESTKEYAAAA' (stale):
13:47:49.119  ERROR  wezterm > rename-workspace-if failed: workspace_mismatch window_id=0 actual="external-moved"; terminating
exit=1
--- proof of no mutation (workspace still 'external-moved'):
WINID TABID PANEID WORKSPACE      SIZE  TITLE CWD                   
    0     0      0 external-moved 80x24 sleep file:///Users/fredrir/

=== (c) nonexistent window id ===
13:47:49.182  ERROR  wezterm > rename-workspace-if failed: no_such_window window_id=4242; terminating
exit=1

=== (d) --if-sole-window: second window shares the workspace ===
WINID TABID PANEID WORKSPACE      SIZE  TITLE CWD                   
    0     0      0 external-moved 80x24 sleep file:///Users/fredrir/
    1     1      1 external-moved 80x24 sleep file:///Users/fredrir/
13:47:50.353  ERROR  wezterm > rename-workspace-if failed: not_sole_window window_id=0 other_window_ids=[1]; terminating
exit=1
--- proof of no mutation:
WINID TABID PANEID WORKSPACE      SIZE  TITLE CWD                   
    0     0      0 external-moved 80x24 sleep file:///Users/fredrir/
    1     1      1 external-moved 80x24 sleep file:///Users/fredrir/

=== (e) CAS the interloper away, then sole-window CAS succeeds ===
interloper window_id=1
exit=0
exit=0
WINID TABID PANEID WORKSPACE             SIZE  TITLE CWD                   
    0     0      0 dmux:sp:01TESTKEYBBBB 80x24 sleep file:///Users/fredrir/
    1     1      1 split-off             80x24 sleep file:///Users/fredrir/

=== teardown ===
killed server pid 79345
done
```

Demo artifacts: `../spike6/demo.sh`, `../spike6/demo-transcript.txt`,
`../spike6/wezterm.lua`, `../spike6/server.log`.

## Fork ownership path globs (for ADR 000, P3c fork agent)

Exact files touched by this primitive:

```
codec/src/lib.rs
mux/src/lib.rs
wezterm-client/src/client.rs
wezterm-mux-server-impl/src/sessionhandler.rs
wezterm/src/cli/rename_workspace.rs
```

Suggested ownership globs (cover this primitive plus the natural blast radius of the other
candidate fork verbs, which live in the same crates):

```
codec/src/**
mux/src/lib.rs
mux/src/window.rs
wezterm-client/src/client.rs
wezterm-mux-server-impl/src/**
wezterm/src/cli/**
```

## Assessment of the other P0 conditional fork primitives (code-level findings only)

1. **Strict socket selector — fork likely NOT needed.**
   `Client::compute_unix_domain` (wezterm-client/src/client.rs:1222-1255): a set, non-empty
   `WEZTERM_UNIX_SOCKET` short-circuits to exactly that socket path — GUI sock discovery
   (`discovery::resolve_gui_sock_path`) and config-order fallback are bypassed entirely.
   `--no-auto-start` is plumbed through `new_default_unix_domain` →
   `Reconnectable::connect(initial, ui, no_auto_start)` (client.rs:1257-1280). So the stock
   CLI already gives exact-socket, no-spawn selection; the plan's remaining strictness
   (socket device/inode, PID/start-token, sentinel epoch nonce) is dmux-side verification
   bracketing the call. Caveats: empty-string `WEZTERM_UNIX_SOCKET` falls through to
   discovery (dmux must always set a non-empty value), and a socket-file replacement between
   connect and use is exactly what the dmux descriptor/inode handshake detects. The endpoint
   spike owns the final verdict.

2. **attach-domain / detach-domain CLI verbs — feasible, moderate diff, only if the
   presentation spike needs them.** No mux PDUs exist for these; they are GUI-side
   `KeyAssignment`s (`config/src/keyassignment.rs:633-634` `DetachDomain(SpawnTabDomain)`,
   `AttachDomain(String)`) handled in `wezterm-gui` (commands.rs, termwindow/mod.rs,
   overlay/launcher.rs). However the GUI hosts the same `wezterm-mux-server-impl`
   sessionhandler on its gui sock (what `wezterm cli` reaches by default), running on the
   GUI main thread with full access to `Mux::get()` and its `ClientDomain`s — so new
   `Pdu::AttachDomain{name}` / `Pdu::DetachDomain{name}` handled via
   `mux.get_domain_by_name(...)` + `domain.attach()/detach()` are implementable in the same
   pattern as this spike; `attach` is async, needing the `SpawnV2`-style
   `promise::spawn::spawn` shimmy (sessionhandler.rs:536-545). Detach-with-verification for
   the §11 quit flow can stay dmux-side (poll list-clients/list after issuing detach).

3. **activate-existing-workspace CLI verb — needs design; defer to the bridge/presentation
   spike.** Mux tracks active workspace per client (`Mux::set_active_workspace_for_client`
   mux/src/lib.rs:636-643) and the GUI honors `MuxNotification::ActiveWorkspaceChanged`
   (wezterm-gui/src/frontend.rs:69, termwindow/mod.rs:1319,1524). But a CLI client setting
   *its own* active workspace does not move the GUI: the verb would have to target the GUI
   client's identity (choose which, from `list-clients`) or broadcast. The GUI-side
   `SwitchToWorkspace` (config/src/keyassignment.rs:612) creates-if-absent, violating the
   plan's atomic no-create requirement, so a fork verb would add
   `ActivateWorkspace { workspace, no_create: true, target_client }` with an existence check
   inside the atomic handler section. Plumbing is straightforward; the open question is
   client-identity targeting, which belongs to the attach/presentation design (the plan's
   current §12 bridge prefers GUI-local correlation + domain import instead).

## Risks / unknowns

- **Codec bump sequencing**: 45→46 fails closed against the currently installed build
  (20260813-114614-18a44cb7, codec 45). Rolling the fork out requires restarting the mux
  server and GUI on both hosts together (lockstep is available). Until then the spike CLI
  cannot talk to the live server — by design.
- **CLI error contract is stringly-typed**: the wire outcome is a typed enum, but
  `wezterm cli` surfaces failures as exit 1 + stable stderr prefixes
  (`workspace_mismatch` / `no_such_window` / `not_sole_window`). If dmux wants distinct exit
  codes or `--format json` for the failure payload, that is a small follow-up in
  `wezterm/src/cli/rename_workspace.rs` (+ possibly `wezterm/src/main.rs` exit-code path).
- **Attached-GUI behavior not demoed**: notifications (`WindowWorkspaceChanged`,
  `ActiveWorkspaceChanged`) flow to attached GUIs via dispatch.rs:153-168/205, and the GUI
  compiles against the new codec, but no GUI was attached during the headless demo.
- **Sole-window scope**: `expect_sole_window` counts mux windows only (not tabs/panes);
  a dead/empty window in the source workspace still blocks — fail-closed, which is correct
  for adoption.
- **Upstreamability**: the primitive is small, additive, and upstream-shaped (typed PDU +
  CLI flags on an existing verb); proposing it upstream would eliminate long-term fork carry.
- Environmental note: macOS `SUN_LEN` (104 bytes) limits socket paths; irrelevant for real
  deployments (sockets under `~/.local/share/wezterm`), but scratch demos need short/relative
  paths.
