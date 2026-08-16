//! tmux provider adapter (plan §11.2, P3a) — implements the frozen
//! [`Provider`] contract against a managed tmux server namespace.
//!
//! Endpoint semantics: `InventoryScope.endpoint` carries the tmux **`-L`
//! socket namespace name** (not a filesystem path). Every spawned command is
//! `tmux -L <endpoint> ...` built as an argv vector — cwd, names, and
//! commands are argv entries, never interpolated into a shell string
//! (plan §11.2). `TMUX`/`TMUX_PANE` are scrubbed from the child environment
//! so a provider running inside a tmux client can never leak its own server.
//!
//! Targeting is ID-only: sessions by `$N`, windows (Groups) by `@N`, panes
//! (Splits) by `%N`. ADR 004 proved user config (`base-index 1`) leaks into
//! managed servers and breaks `name:index` targeting; name/index forms are
//! never emitted.
//!
//! Epoch discipline (plan §11.2): the server epoch is the global user option
//! `@dmux_server_epoch`, read with `show-options -gqv` (absent/empty → an
//! unepoched server, `server_epoch = None`, children unaddressable).
//! Inventory **never** sets the option — `ls` never brings a server under
//! management; the P5 bootstrap hook owns epoch installation. Every mutation
//! re-reads the option immediately before acting and fails typed
//! (`EpochChanged`) on mismatch. `create` requires `scope.expected_epoch`;
//! it never boots the server itself.
//!
//! P5 epoch-bootstrap primitives (plan §11.2): [`TmuxProvider::server_identity`],
//! [`TmuxProvider::set_epoch_if_absent`], and [`TmuxProvider::verify_epoch`]
//! are the building blocks the root's `dmux _tmux-bootstrap` orchestration
//! calls, in that order, around its registry publish. They take no lock —
//! the CALLER holds the exclusive tmux-backend kernel lock for the whole
//! sequence, so dmux-vs-dmux races cannot happen; the verify-after-write
//! readback in `set_epoch_if_absent` covers non-dmux external writers.
//! `set_epoch_if_absent` is the **only** function in this module that writes
//! `@dmux_server_epoch`; identity probing and verification are read-only.
//!
//! `create` asymmetry (registry `native_kind='tmux_session_id'`):
//! `CreateSpec.native_token` is the requested **session name** passed to
//! `new-session -s`; the returned `NativeBinding.native_token` is the
//! immutable **session id** `$N`, which survives external `rename-session`.
//!
//! Handle kinds are positional (model.rs `ProviderHandle::Tx`): handles in
//! `group_*` positions and `split_list`'s parent are windows (`@N`); the
//! parent handle of `split_new` and the handles of `split_activate`/
//! `split_remove` are panes (`%N`), matching plan §11.3 ("new Split inherits
//! the target Split cwd").
//!
//! Managed-session option assertion (ADR 004/005): `allow-set-title on` and
//! `allow-passthrough all` are window/pane-scoped options in tmux, so they
//! are stamped per managed window (`set-option -w -t @N`) at `create` and
//! `group_new`; panes inherit from their window. Windows created behind
//! dmux's back are re-asserted when later managed operations touch them.
//!
//! Specialist-owned (plan §19, P3a); the trait and result types in
//! `backend/mod.rs` are the frozen root-owned contract.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::backend::{
    Capabilities, CreateSpec, GroupActivationResult, InventoryOutcome, InventoryScope,
    NativeBinding, NativeGroupRow, NativeInventory, NativeSpaceRow, NativeSplitRow,
    PresentationTarget, Provider, ProviderError, ProviderResult, SplitDirection,
    SplitDirectionResult, SplitResizeResult, SplitSpec, SplitZoomResult,
};
use crate::model::{Backend, ProviderHandle, ServerEpoch};

/// Unit separator used in `-F` format strings. It is passed as a literal
/// argv byte (never through a shell) and cannot be produced by tmux's own
/// `#{...}` expansions. Arbitrary-content fields (names, titles) are always
/// the **last** field of a row and parsed as the remainder, so even a title
/// that somehow embeds the byte cannot shift the ID fields.
const SEP: char = '\u{1f}';

/// Global user option holding the owner-assigned server incarnation UUID.
const EPOCH_OPTION: &str = "@dmux_server_epoch";

/// Frozen spawn-return format (ADR 004): all three IDs atomically.
const SPAWN_FORMAT: &str = "#{session_id}|#{window_id}|#{pane_id}";

/// Server-incarnation identity probe (plan §11.2, P5). All three fields are
/// **server-scoped** tmux formats, read via `list-sessions` because a tmux
/// server only runs while it has at least one session — so on any running
/// server this listing yields at least one row, whereas `display-message`
/// needs a client/target heuristic. The socket path is last (remainder
/// field) since it is the only field with arbitrary content.
const IDENTITY_FORMAT: &str = "#{pid}\u{1f}#{start_time}\u{1f}#{socket_path}";

const SESSIONS_FORMAT: &str = "#{session_id}\u{1f}#{session_name}";
const WINDOWS_FORMAT: &str = "#{session_id}\u{1f}#{window_id}\u{1f}#{window_name}";
const PANES_FORMAT: &str =
    "#{session_id}\u{1f}#{window_id}\u{1f}#{pane_id}\u{1f}#{pane_current_path}\u{1f}#{pane_title}";
/// Numeric-only action scans.  These are deliberately separate from the
/// user-content inventory formats so an embedded separator in a title can
/// never affect focus/layout postcondition parsing.
const ACTION_WINDOWS_FORMAT: &str = "#{session_id}\u{1f}#{window_id}\u{1f}#{window_active}";
const ACTION_PANES_FORMAT: &str = "#{session_id}\u{1f}#{window_id}\u{1f}#{pane_id}\u{1f}\
#{pane_active}\u{1f}#{pane_width}\u{1f}#{pane_height}\u{1f}#{pane_left}\u{1f}\
#{pane_top}\u{1f}#{window_zoomed_flag}";

/// Session marker options stamped at adoption/creation (plan §10.3). Exact
/// markers plus the immutable `$N` preserve identity across external rename.
pub const MARKER_HOST_UID: &str = "@dmux_host_uid";
pub const MARKER_REGISTRY_UID: &str = "@dmux_registry_uid";
pub const MARKER_SPACE_UID: &str = "@dmux_space_uid";
pub const MARKER_SPACE_NO: &str = "@dmux_space_no";

/// Default per-child-process deadline. Every spawned tmux command gets a
/// dmux-imposed deadline — the stock CLI can hang forever (plan §8.1).
const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);

/// Bounded kill/re-list convergence rounds for remove (plan §14, ADR 005).
const REMOVE_ROUNDS: usize = 3;

// ---------------------------------------------------------------------------
// Command-runner seam
// ---------------------------------------------------------------------------

/// Completed child process observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// Exit code; `-1` when terminated by a signal.
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl RunOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The tmux binary could not be spawned (`ENOENT`).
    MissingBinary { detail: String },
    /// The dmux-imposed deadline elapsed; the child was killed.
    Timeout { detail: String },
    /// Any other spawn/IO failure.
    Io { detail: String },
}

/// Injectable execution seam: the provider builds exact argv vectors and the
/// runner executes them. Unit tests substitute a scripted runner asserting
/// exact argv and feeding canned output; production uses [`SystemRunner`].
pub trait TmuxRunner {
    fn run(&self, argv: &[String], deadline: Duration) -> Result<RunOutput, RunError>;
}

/// Real runner: `std::process::Command` over argv arrays, never a shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl TmuxRunner for SystemRunner {
    fn run(&self, argv: &[String], deadline: Duration) -> Result<RunOutput, RunError> {
        let (program, args) = argv.split_first().ok_or_else(|| RunError::Io {
            detail: "empty argv".into(),
        })?;
        let mut child = Command::new(program)
            .args(args)
            // A provider running inside a tmux client must never let the
            // ambient server leak into targeting or spawned panes.
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    RunError::MissingBinary {
                        detail: format!("{program}: {e}"),
                    }
                } else {
                    RunError::Io {
                        detail: format!("spawn {program}: {e}"),
                    }
                }
            })?;

        let mut stdout_pipe = child.stdout.take().expect("piped stdout");
        let mut stderr_pipe = child.stderr.take().expect("piped stderr");
        let stdout_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buf);
            buf
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf);
            buf
        });

        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stdout = stdout_reader.join().unwrap_or_default();
                    let stderr = stderr_reader.join().unwrap_or_default();
                    return Ok(RunOutput {
                        status: status.code().unwrap_or(-1),
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => {
                    if started.elapsed() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(RunError::Timeout {
                            detail: format!(
                                "{program} exceeded {}ms deadline",
                                deadline.as_millis()
                            ),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(RunError::Io {
                        detail: format!("wait {program}: {e}"),
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// tmux adapter over an injectable runner. One provider instance serves one
/// managed backend instance; `endpoint` (the `-L` namespace) is held for the
/// scope-less `capabilities()` probe, while every scoped operation uses
/// `scope.endpoint` as passed by the caller.
pub struct TmuxProvider<R: TmuxRunner> {
    runner: R,
    endpoint: String,
    deadline: Duration,
}

impl TmuxProvider<SystemRunner> {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self::with_runner(endpoint, SystemRunner)
    }
}

impl<R: TmuxRunner> TmuxProvider<R> {
    pub fn with_runner(endpoint: impl Into<String>, runner: R) -> Self {
        TmuxProvider {
            runner,
            endpoint: endpoint.into(),
            deadline: DEFAULT_DEADLINE,
        }
    }

    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    fn argv(endpoint: &str, args: &[&str]) -> Vec<String> {
        let mut v = Vec::with_capacity(args.len() + 3);
        v.push("tmux".to_string());
        v.push("-L".to_string());
        v.push(endpoint.to_string());
        v.extend(args.iter().map(|s| s.to_string()));
        v
    }

    fn run(&self, endpoint: &str, args: &[&str]) -> Result<RunOutput, RunError> {
        self.runner.run(&Self::argv(endpoint, args), self.deadline)
    }

    /// Run a mutation/lookup command that must exit 0; map failures typed.
    fn run_ok(&self, endpoint: &str, args: &[&str]) -> ProviderResult<String> {
        let out = self.run(endpoint, args).map_err(map_run_error)?;
        if !out.ok() {
            let stderr = lossy(&out.stderr);
            if absence_stderr(&stderr) {
                return Err(ProviderError::NotFound {
                    native_ref: stderr.trim().to_string(),
                });
            }
            return Err(ProviderError::NativeFailure {
                detail: format!(
                    "tmux {}: {}",
                    args.first().copied().unwrap_or(""),
                    stderr.trim()
                ),
            });
        }
        utf8(&out.stdout).map_err(|detail| ProviderError::NativeFailure { detail })
    }

    /// Read `@dmux_server_epoch` (`show-options -gqv`): absent/empty → None.
    fn read_epoch(&self, endpoint: &str) -> Result<Option<ServerEpoch>, EpochFailure> {
        let out = self
            .run(endpoint, &["show-options", "-gqv", EPOCH_OPTION])
            .map_err(|e| match e {
                RunError::MissingBinary { detail } => EpochFailure::MissingBinary(detail),
                RunError::Timeout { detail } => EpochFailure::Timeout(detail),
                RunError::Io { detail } => EpochFailure::Malformed(detail),
            })?;
        if !out.ok() {
            let stderr = lossy(&out.stderr);
            if no_server_stderr(&stderr) {
                return Err(EpochFailure::NoServer(stderr.trim().to_string()));
            }
            return Err(EpochFailure::Malformed(format!(
                "show-options {EPOCH_OPTION}: {}",
                stderr.trim()
            )));
        }
        let text = utf8(&out.stdout).map_err(EpochFailure::Malformed)?;
        let value = text.trim();
        if value.is_empty() {
            return Ok(None);
        }
        let uuid = Uuid::parse_str(value).map_err(|e| {
            EpochFailure::Malformed(format!("unparseable {EPOCH_OPTION} {value:?}: {e}"))
        })?;
        Ok(Some(ServerEpoch(uuid)))
    }

    /// Re-read the epoch immediately before a native-ID mutation and require
    /// it to equal `expected` (plan §11.2; mismatch discards native IDs).
    /// (Internal helper; the public [`Self::verify_epoch`] additionally
    /// rechecks the server incarnation identity.)
    fn check_epoch(&self, endpoint: &str, expected: ServerEpoch) -> ProviderResult<()> {
        let observed = self.read_epoch(endpoint).map_err(map_epoch_failure)?;
        if observed != Some(expected) {
            return Err(ProviderError::EpochChanged { expected, observed });
        }
        Ok(())
    }

    // -- P5 epoch-bootstrap primitives (plan §11.2) --------------------------
    //
    // Called by the root's `dmux _tmux-bootstrap` orchestration in the order
    // identity → (caller takes kernel lock) → set_epoch_if_absent →
    // registry publish → verify_epoch. None of these functions locks; the
    // caller holds the exclusive tmux-backend kernel lock throughout.

    /// Probe the exact socket's server incarnation: PID plus start token
    /// (plan §11.2). Read via `list-sessions -F` with server-scoped formats —
    /// a tmux server only runs while it has at least one session, so a
    /// running server always yields at least one row (and `display-message`
    /// would need a client/session heuristic instead). All rows carry
    /// identical server-scoped fields; any disagreement means the output is
    /// malformed, never guessed at.
    ///
    /// Start-token derivation: `#{start_time}` — the server process's own
    /// wall-clock start time in whole seconds since the epoch, recorded by
    /// tmux at server birth (probed present on tmux 3.7b). A restarted
    /// server is a new process started at a later wall-clock second, so the
    /// token changes across incarnations; paired with the PID, a false match
    /// would need same-second restart *and* immediate PID reuse. If a
    /// (pre-2.2) tmux expands `#{start_time}` to empty, the fallback token
    /// is `ino:<dev>:<inode>` of the resolved `#{socket_path}` — a new
    /// server unlinks and re-binds the socket, allocating a fresh inode.
    pub fn server_identity(&self, endpoint: &str) -> ProviderResult<TmuxServerIdentity> {
        let out = self
            .run(endpoint, &["list-sessions", "-F", IDENTITY_FORMAT])
            .map_err(map_run_error)?;
        if !out.ok() {
            let stderr = lossy(&out.stderr);
            if no_server_stderr(&stderr) {
                return Err(ProviderError::NativeFailure {
                    detail: format!("no tmux server for this namespace: {}", stderr.trim()),
                });
            }
            return Err(ProviderError::NativeFailure {
                detail: format!("tmux list-sessions: {}", stderr.trim()),
            });
        }
        let text = utf8(&out.stdout).map_err(|detail| ProviderError::NativeFailure { detail })?;
        let (pid, start_time, socket_path) =
            parse_identity(&text).map_err(|detail| ProviderError::NativeFailure { detail })?;
        let start_token = if start_time.is_empty() {
            // Pre-#{start_time} fallback: the socket inode is allocated when
            // this incarnation bound the (freshly re-created) socket file.
            use std::os::unix::fs::MetadataExt;
            let meta =
                std::fs::metadata(&socket_path).map_err(|e| ProviderError::NativeFailure {
                    detail: format!("stat tmux socket {socket_path:?}: {e}"),
                })?;
            format!("ino:{}:{}", meta.dev(), meta.ino())
        } else {
            start_time
        };
        Ok(TmuxServerIdentity { pid, start_token })
    }

    /// Atomically bring a previously unepoched server incarnation under
    /// management (plan §11.2, P5). Read `@dmux_server_epoch`; if absent or
    /// empty, `set-option -g` the caller's epoch, then read back and verify
    /// the write survived. The caller holds the exclusive kernel lock, so a
    /// dmux-vs-dmux race is impossible; the readback covers a non-dmux
    /// external writer racing the set — if the observed value differs from
    /// what was written, the external racer won and its value is returned as
    /// [`EpochSetOutcome::AlreadySet`]. A malformed (non-UUID) existing value
    /// is a typed error, never overwritten. This is the only writer of the
    /// option in this module; `inventory` never sets it.
    pub fn set_epoch_if_absent(
        &self,
        endpoint: &str,
        epoch: ServerEpoch,
    ) -> ProviderResult<EpochSetOutcome> {
        if let Some(existing) = self.read_epoch(endpoint).map_err(map_epoch_failure)? {
            return Ok(EpochSetOutcome::AlreadySet(existing));
        }
        let value = epoch.0.to_string();
        self.run_ok(endpoint, &["set-option", "-g", EPOCH_OPTION, &value])?;
        match self.read_epoch(endpoint).map_err(map_epoch_failure)? {
            Some(observed) if observed == epoch => Ok(EpochSetOutcome::Set),
            Some(observed) => Ok(EpochSetOutcome::AlreadySet(observed)),
            None => Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "{EPOCH_OPTION} absent immediately after set on {endpoint}: \
                     server restarted mid-bootstrap or option externally unset"
                ),
            }),
        }
    }

    /// Recheck socket, PID/start-token, and epoch immediately before child
    /// mutations (plan §11.2). Identity is checked first — a changed
    /// PID/start token means a different server incarnation entirely
    /// ([`ProviderError::WrongInstance`]); only then is the epoch option
    /// compared ([`ProviderError::EpochChanged`] on mismatch). Read-only.
    pub fn verify_epoch(
        &self,
        endpoint: &str,
        expected: ServerEpoch,
        expected_identity: &TmuxServerIdentity,
    ) -> ProviderResult<()> {
        let observed = self.server_identity(endpoint)?;
        if observed != *expected_identity {
            return Err(ProviderError::WrongInstance {
                detail: format!(
                    "tmux server incarnation changed on {endpoint}: expected pid {} \
                     start {:?}, observed pid {} start {:?}",
                    expected_identity.pid,
                    expected_identity.start_token,
                    observed.pid,
                    observed.start_token
                ),
            });
        }
        self.check_epoch(endpoint, expected)
    }

    /// Managed handle/child operations require the caller-held epoch: an
    /// unepoched server is listable but its children are unaddressable.
    fn required_epoch(scope: &InventoryScope) -> ProviderResult<ServerEpoch> {
        scope.expected_epoch.ok_or(ProviderError::WrongInstance {
            detail: "managed tmux mutation requires scope.expected_epoch; \
                     an unepoched server has no addressable children (plan §11.2)"
                .into(),
        })
    }

    /// Cross-check a stale binding against the caller's scope before use.
    fn binding_epoch(
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<ServerEpoch> {
        if let Some(expected) = scope.expected_epoch
            && expected != binding.server_epoch
        {
            return Err(ProviderError::EpochChanged {
                expected,
                observed: Some(binding.server_epoch),
            });
        }
        Ok(binding.server_epoch)
    }

    fn scope_check(scope: &InventoryScope) -> ProviderResult<()> {
        if scope.backend != Backend::Tmux {
            return Err(ProviderError::WrongInstance {
                detail: format!("tmux provider handed a {} scope", scope.backend),
            });
        }
        Ok(())
    }

    /// Assert the ADR 004/005 managed options on one window. Both are
    /// window/pane-scoped in tmux, so they are stamped per managed window;
    /// panes inherit from their window.
    fn assert_window_options(&self, endpoint: &str, window: u64) -> ProviderResult<()> {
        let target = format!("@{window}");
        self.run_ok(
            endpoint,
            &["set-option", "-w", "-t", &target, "allow-set-title", "on"],
        )?;
        self.run_ok(
            endpoint,
            &[
                "set-option",
                "-w",
                "-t",
                &target,
                "allow-passthrough",
                "all",
            ],
        )?;
        Ok(())
    }

    fn list_session_ids(&self, endpoint: &str) -> ProviderResult<Vec<String>> {
        let out = self.run_ok(endpoint, &["list-sessions", "-F", "#{session_id}"])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    /// Session/window/pane rows for one exact session.
    fn session_rows(&self, endpoint: &str, session: &str) -> ProviderResult<Vec<NativeGroupRow>> {
        let windows = self.run_ok(
            endpoint,
            &["list-windows", "-t", session, "-F", WINDOWS_FORMAT],
        )?;
        let panes = self.run_ok(
            endpoint,
            &["list-panes", "-s", "-t", session, "-F", PANES_FORMAT],
        )?;
        let windows = parse_windows(&windows).map_err(malformed_scan)?;
        let panes = parse_panes(&panes).map_err(malformed_scan)?;
        let rows = assemble_rows(&[(session.to_string(), String::new())], &windows, &panes)
            .map_err(malformed_scan)?;
        Ok(rows
            .into_iter()
            .next()
            .map(|r| r.groups)
            .unwrap_or_default())
    }

    // -- markers (plan §10.3) ------------------------------------------------

    /// Stamp the four dmux identity markers on one exact session (`$N`).
    /// Verified by readback; markers plus the immutable session id preserve
    /// identity across external `rename-session` (proven in ADR 004).
    pub fn stamp_markers(
        &self,
        scope: &InventoryScope,
        session: &str,
        markers: &SpaceMarkers,
    ) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        validate_session_token(session)?;
        let expected = Self::required_epoch(scope)?;
        self.check_epoch(&scope.endpoint, expected)?;
        for (option, value) in [
            (MARKER_HOST_UID, &markers.host_uid),
            (MARKER_REGISTRY_UID, &markers.registry_uid),
            (MARKER_SPACE_UID, &markers.space_uid),
            (MARKER_SPACE_NO, &markers.space_no),
        ] {
            self.run_ok(
                &scope.endpoint,
                &["set-option", "-t", session, option, value],
            )?;
        }
        let readback = self.read_markers(scope, session)?;
        let stamped = SpaceMarkerReadback {
            host_uid: Some(markers.host_uid.clone()),
            registry_uid: Some(markers.registry_uid.clone()),
            space_uid: Some(markers.space_uid.clone()),
            space_no: Some(markers.space_no.clone()),
        };
        if readback != stamped {
            return Err(ProviderError::PostconditionFailed {
                detail: format!("marker readback mismatch on {session}: {readback:?}"),
            });
        }
        Ok(())
    }

    /// Read the four markers from one exact session; absent → None per
    /// marker. Epoch is verified only when the caller holds one, so
    /// adoption-time discovery of unmanaged sessions can still read stamps.
    pub fn read_markers(
        &self,
        scope: &InventoryScope,
        session: &str,
    ) -> ProviderResult<SpaceMarkerReadback> {
        Self::scope_check(scope)?;
        validate_session_token(session)?;
        if let Some(expected) = scope.expected_epoch {
            self.check_epoch(&scope.endpoint, expected)?;
        }
        let mut values = [const { None }; 4];
        for (slot, option) in [
            MARKER_HOST_UID,
            MARKER_REGISTRY_UID,
            MARKER_SPACE_UID,
            MARKER_SPACE_NO,
        ]
        .iter()
        .enumerate()
        {
            let out = self.run_ok(
                &scope.endpoint,
                &["show-options", "-t", session, "-qv", option],
            )?;
            let trimmed = out.trim_end_matches('\n');
            values[slot] = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
        let [host_uid, registry_uid, space_uid, space_no] = values;
        Ok(SpaceMarkerReadback {
            host_uid,
            registry_uid,
            space_uid,
            space_no,
        })
    }

    /// Bounded kill/verify convergence for one exact native ref
    /// (plan §14, ADR 005): a benign "can't find" from the kill verb is
    /// success-equivalent only once verified absence confirms it.
    fn kill_converge(
        &self,
        endpoint: &str,
        kill_args: &[&str],
        absent: impl Fn(&Self) -> ProviderResult<bool>,
    ) -> ProviderResult<()> {
        for _ in 0..REMOVE_ROUNDS {
            let out = self.run(endpoint, kill_args).map_err(map_run_error)?;
            if !out.ok() {
                let stderr = lossy(&out.stderr);
                if !absence_stderr(&stderr) && !no_server_stderr(&stderr) {
                    return Err(ProviderError::NativeFailure {
                        detail: format!("tmux {}: {}", kill_args[0], stderr.trim()),
                    });
                }
            }
            if absent(self)? {
                return Ok(());
            }
        }
        Err(ProviderError::PostconditionFailed {
            detail: format!(
                "{} did not converge to absence within {REMOVE_ROUNDS} rounds",
                kill_args[0]
            ),
        })
    }

    /// Absence check that treats a stopped server as verified absence:
    /// killing the last session terminates the server itself.
    fn listing_absent(&self, endpoint: &str, args: &[&str], needle: &str) -> ProviderResult<bool> {
        let out = self.run(endpoint, args).map_err(map_run_error)?;
        if !out.ok() {
            let stderr = lossy(&out.stderr);
            if no_server_stderr(&stderr) {
                return Ok(true);
            }
            return Err(ProviderError::NativeFailure {
                detail: format!("tmux {}: {}", args[0], stderr.trim()),
            });
        }
        let text = utf8(&out.stdout).map_err(|detail| ProviderError::NativeFailure { detail })?;
        Ok(!text.lines().any(|l| l == needle))
    }

    fn action_windows(&self, endpoint: &str) -> ProviderResult<Vec<ActionWindowRow>> {
        let listing = self.run_ok(
            endpoint,
            &["list-windows", "-a", "-F", ACTION_WINDOWS_FORMAT],
        )?;
        parse_action_windows(&listing).map_err(malformed_scan)
    }

    fn action_panes(&self, endpoint: &str) -> ProviderResult<Vec<ActionPaneRow>> {
        let listing = self.run_ok(endpoint, &["list-panes", "-a", "-F", ACTION_PANES_FORMAT])?;
        parse_action_panes(&listing).map_err(malformed_scan)
    }

    /// Activate one exact immutable window ID under an epoch-pinned tmux
    /// namespace, then prove that same window is active in the same session.
    pub fn activate_group_exact(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
    ) -> ProviderResult<GroupActivationResult> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = window_target(group)?;
        let group_id = tmux_handle_id(group)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let before = self.action_windows(&scope.endpoint)?;
        let before = before
            .iter()
            .find(|row| row.window == group_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: target.clone(),
            })?;
        self.check_epoch(&scope.endpoint, expected)?;
        self.run_ok(&scope.endpoint, &["select-window", "-t", &target])?;
        let after = self.action_windows(&scope.endpoint)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let after = after
            .iter()
            .find(|row| row.window == before.window)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("tmux group activation target {target} vanished after select"),
            })?;
        if after.session != before.session || !after.active {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "tmux group activation postcondition failed for {target}: before session {}, \
                     after session {} active={}",
                    before.session, after.session, after.active
                ),
            });
        }
        Ok(GroupActivationResult {
            server_epoch: expected,
            target: group.clone(),
        })
    }

    /// Select an adjacent pane relative to one exact pane ID. At an edge the
    /// origin remains active and `target=None`; no pane ordinal is guessed.
    pub fn select_split_direction(
        &self,
        scope: &InventoryScope,
        origin: &ProviderHandle,
        direction: SplitDirection,
    ) -> ProviderResult<SplitDirectionResult> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let origin_target = pane_target(origin)?;
        let origin_id = tmux_handle_id(origin)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let before = self.action_panes(&scope.endpoint)?;
        let before = before
            .iter()
            .find(|row| row.pane == origin_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: origin_target.clone(),
            })?;
        self.check_epoch(&scope.endpoint, expected)?;

        // Make the supplied origin authoritative before applying the native
        // directional verb. This also makes the edge result deterministic if
        // another pane happened to be active beforehand.
        self.run_ok(&scope.endpoint, &["select-pane", "-t", &origin_target])?;
        let flag = match direction {
            SplitDirection::Left => "-L",
            SplitDirection::Right => "-R",
            SplitDirection::Up => "-U",
            SplitDirection::Down => "-D",
        };
        self.run_ok(
            &scope.endpoint,
            &["select-pane", "-t", &origin_target, flag],
        )?;
        let after = self.action_panes(&scope.endpoint)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let origin_after = after
            .iter()
            .find(|row| row.pane == before.pane)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("tmux directional origin {origin_target} vanished after select"),
            })?;
        if origin_after.session != before.session || origin_after.window != before.window {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "tmux directional origin {origin_target} changed parent from {}:@{} to {}:@{}",
                    before.session, before.window, origin_after.session, origin_after.window
                ),
            });
        }
        let active: Vec<&ActionPaneRow> = after
            .iter()
            .filter(|row| {
                row.session == before.session && row.window == before.window && row.active
            })
            .collect();
        let [active] = active.as_slice() else {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "tmux directional select for {origin_target} found {} active panes in {}:@{}",
                    active.len(),
                    before.session,
                    before.window
                ),
            });
        };
        let target = (active.pane != before.pane).then(|| ProviderHandle::Tx(active.pane));
        Ok(SplitDirectionResult {
            server_epoch: expected,
            origin: origin.clone(),
            target,
        })
    }

    /// Resize one exact pane by a positive cell amount and re-list its exact
    /// identity/geometry. A native boundary no-op is represented explicitly.
    pub fn resize_split_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
        direction: SplitDirection,
        amount: u16,
    ) -> ProviderResult<SplitResizeResult> {
        Self::scope_check(scope)?;
        if amount == 0 {
            return Err(ProviderError::NativeFailure {
                detail: "tmux split resize amount must be greater than zero".into(),
            });
        }
        let expected = Self::required_epoch(scope)?;
        let target = pane_target(split)?;
        let split_id = tmux_handle_id(split)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let before = self.action_panes(&scope.endpoint)?;
        let before = before
            .iter()
            .find(|row| row.pane == split_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: target.clone(),
            })?;
        self.check_epoch(&scope.endpoint, expected)?;
        let flag = match direction {
            SplitDirection::Left => "-L",
            SplitDirection::Right => "-R",
            SplitDirection::Up => "-U",
            SplitDirection::Down => "-D",
        };
        let amount = amount.to_string();
        self.run_ok(
            &scope.endpoint,
            &["resize-pane", "-t", &target, flag, &amount],
        )?;
        let after = self.action_panes(&scope.endpoint)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let after = after
            .iter()
            .find(|row| row.pane == before.pane)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("tmux resize target {target} vanished after resize-pane"),
            })?;
        if after.session != before.session || after.window != before.window {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "tmux resize target {target} changed parent from {}:@{} to {}:@{}",
                    before.session, before.window, after.session, after.window
                ),
            });
        }
        Ok(SplitResizeResult {
            server_epoch: expected,
            target: split.clone(),
            changed: before.geometry() != after.geometry(),
        })
    }

    /// Toggle zoom for one exact pane and require the window zoom flag to
    /// flip in a same-epoch postcondition scan.
    pub fn toggle_split_zoom_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
    ) -> ProviderResult<SplitZoomResult> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = pane_target(split)?;
        let split_id = tmux_handle_id(split)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let before = self.action_panes(&scope.endpoint)?;
        let before = before
            .iter()
            .find(|row| row.pane == split_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: target.clone(),
            })?;
        self.check_epoch(&scope.endpoint, expected)?;
        self.run_ok(&scope.endpoint, &["resize-pane", "-Z", "-t", &target])?;
        let after = self.action_panes(&scope.endpoint)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let after = after
            .iter()
            .find(|row| row.pane == before.pane)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("tmux zoom target {target} vanished after resize-pane -Z"),
            })?;
        if after.session != before.session
            || after.window != before.window
            || after.zoomed == before.zoomed
        {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "tmux zoom postcondition failed for {target}: parent {}:@{} -> {}:@{}, \
                     zoomed {} -> {}",
                    before.session,
                    before.window,
                    after.session,
                    after.window,
                    before.zoomed,
                    after.zoomed
                ),
            });
        }
        Ok(SplitZoomResult {
            server_epoch: expected,
            target: split.clone(),
            zoomed: after.zoomed,
        })
    }
}

/// One tmux server incarnation on one exact socket (plan §11.2, P5): the
/// server process's PID plus a start token stable for the incarnation's
/// lifetime and different for every restart (see
/// [`TmuxProvider::server_identity`] for the exact derivation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxServerIdentity {
    pub pid: u32,
    pub start_token: String,
}

/// Outcome of [`TmuxProvider::set_epoch_if_absent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpochSetOutcome {
    /// The option was absent; the caller's epoch was written and verified
    /// by readback.
    Set,
    /// The option was already present (or an external racer's write won the
    /// readback); the carried value is the epoch actually on the server.
    AlreadySet(ServerEpoch),
}

/// Identity markers stamped on a managed/adopted session (plan §10.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMarkers {
    pub host_uid: String,
    pub registry_uid: String,
    pub space_uid: String,
    pub space_no: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceMarkerReadback {
    pub host_uid: Option<String>,
    pub registry_uid: Option<String>,
    pub space_uid: Option<String>,
    pub space_no: Option<String>,
}

enum EpochFailure {
    NoServer(String),
    MissingBinary(String),
    Timeout(String),
    Malformed(String),
}

fn map_epoch_failure(e: EpochFailure) -> ProviderError {
    match e {
        EpochFailure::NoServer(detail) => ProviderError::NativeFailure {
            detail: format!("no tmux server for this namespace: {detail}"),
        },
        EpochFailure::MissingBinary(detail) => ProviderError::NativeFailure {
            detail: format!("tmux binary missing: {detail}"),
        },
        EpochFailure::Timeout(detail) => ProviderError::Timeout { detail },
        EpochFailure::Malformed(detail) => ProviderError::NativeFailure { detail },
    }
}

fn map_run_error(e: RunError) -> ProviderError {
    match e {
        RunError::MissingBinary { detail } => ProviderError::NativeFailure {
            detail: format!("tmux binary missing: {detail}"),
        },
        RunError::Timeout { detail } => ProviderError::Timeout { detail },
        RunError::Io { detail } => ProviderError::NativeFailure { detail },
    }
}

fn malformed_scan(detail: String) -> ProviderError {
    ProviderError::NativeFailure {
        detail: format!("malformed tmux listing: {detail}"),
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn utf8(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec()).map_err(|e| format!("non-utf8 tmux output: {e}"))
}

/// tmux 3.7b liveness classification, probed live: a missing socket says
/// "error connecting to <path> (No such file or directory)", a stale socket
/// "no server running on <path>".
fn no_server_stderr(stderr: &str) -> bool {
    stderr.contains("no server running") || stderr.contains("error connecting to")
}

/// Benign already-dead classification for kill/lookup verbs (ADR 005):
/// tmux 3.7b says "can't find session: $N" / window / pane.
fn absence_stderr(stderr: &str) -> bool {
    stderr.contains("can't find session")
        || stderr.contains("can't find window")
        || stderr.contains("can't find pane")
        || stderr.contains("session not found")
}

fn validate_session_token(token: &str) -> ProviderResult<()> {
    if parse_sigil_id(token, '$').is_some() {
        return Ok(());
    }
    Err(ProviderError::WrongInstance {
        detail: format!("not an immutable tmux session id (`$N`): {token:?}"),
    })
}

fn window_target(handle: &ProviderHandle) -> ProviderResult<String> {
    match handle {
        ProviderHandle::Tx(n) => Ok(format!("@{n}")),
        other => Err(ProviderError::WrongInstance {
            detail: format!("not a tmux window handle: {other}"),
        }),
    }
}

fn tmux_handle_id(handle: &ProviderHandle) -> ProviderResult<u64> {
    match handle {
        ProviderHandle::Tx(n) => Ok(*n),
        other => Err(ProviderError::WrongInstance {
            detail: format!("not a tmux native handle: {other}"),
        }),
    }
}

fn pane_target(handle: &ProviderHandle) -> ProviderResult<String> {
    match handle {
        ProviderHandle::Tx(n) => Ok(format!("%{n}")),
        other => Err(ProviderError::WrongInstance {
            detail: format!("not a tmux pane handle: {other}"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Listing parsers (fixture-tested)
// ---------------------------------------------------------------------------

fn parse_sigil_id(token: &str, sigil: char) -> Option<u64> {
    let rest = token.strip_prefix(sigil)?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

/// `#{pid}\x1f#{start_time}\x1f#{socket_path}` per line; the socket path is
/// the remainder field. All fields are server-scoped, so every row must be
/// identical; a disagreement means the output raced/garbled and is reported
/// malformed. Returns `(pid, start_time, socket_path)` with `start_time`
/// possibly empty (older tmux without the variable).
fn parse_identity(text: &str) -> Result<(u32, String, String), String> {
    let mut lines = text.lines();
    let first = lines
        .next()
        .ok_or_else(|| "empty identity listing from a running server".to_string())?;
    for other in lines {
        if other != first {
            return Err(format!(
                "server-scoped identity rows disagree: {first:?} vs {other:?}"
            ));
        }
    }
    let mut parts = first.splitn(3, SEP);
    let (Some(pid), Some(start), Some(socket)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("identity row with missing fields: {first:?}"));
    };
    let pid: u32 = pid
        .parse()
        .map_err(|e| format!("bad server pid in identity row {first:?}: {e}"))?;
    if !start.is_empty() && !start.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("bad start_time in identity row: {first:?}"));
    }
    if socket.is_empty() {
        return Err(format!("empty socket_path in identity row: {first:?}"));
    }
    Ok((pid, start.to_string(), socket.to_string()))
}

/// `#{session_id}\x1f#{session_name}` per line; name is the remainder.
fn parse_sessions(text: &str) -> Result<Vec<(String, String)>, String> {
    text.lines()
        .map(|line| {
            let (sid, name) = line
                .split_once(SEP)
                .ok_or_else(|| format!("session row without separator: {line:?}"))?;
            if parse_sigil_id(sid, '$').is_none() {
                return Err(format!("bad session id in row: {line:?}"));
            }
            Ok((sid.to_string(), name.to_string()))
        })
        .collect()
}

/// `#{session_id}\x1f#{window_id}\x1f#{window_name}`; name is the remainder.
fn parse_windows(text: &str) -> Result<Vec<(String, u64, String)>, String> {
    text.lines()
        .map(|line| {
            let mut parts = line.splitn(3, SEP);
            let (Some(sid), Some(wid), Some(name)) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(format!("window row with missing fields: {line:?}"));
            };
            if parse_sigil_id(sid, '$').is_none() {
                return Err(format!("bad session id in window row: {line:?}"));
            }
            let window = parse_sigil_id(wid, '@')
                .ok_or_else(|| format!("bad window id in row: {line:?}"))?;
            Ok((sid.to_string(), window, name.to_string()))
        })
        .collect()
}

type PaneRow = (String, u64, u64, Option<String>, Option<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActionWindowRow {
    session: String,
    window: u64,
    active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActionPaneRow {
    session: String,
    window: u64,
    pane: u64,
    active: bool,
    width: u32,
    height: u32,
    left: u32,
    top: u32,
    zoomed: bool,
}

impl ActionPaneRow {
    fn geometry(&self) -> (u32, u32, u32, u32) {
        (self.width, self.height, self.left, self.top)
    }
}

fn parse_tmux_bool(field: &str, row: &str) -> Result<bool, String> {
    match field {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("bad boolean {field:?} in action row {row:?}")),
    }
}

fn parse_action_u32(field: &str, name: &str, row: &str) -> Result<u32, String> {
    field
        .parse()
        .map_err(|e| format!("bad {name} in action row {row:?}: {e}"))
}

fn parse_action_windows(text: &str) -> Result<Vec<ActionWindowRow>, String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut fields = line.split(SEP);
        let (Some(session), Some(window), Some(active), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!(
                "window action row with wrong field count: {line:?}"
            ));
        };
        if parse_sigil_id(session, '$').is_none() {
            return Err(format!("bad session id in window action row: {line:?}"));
        }
        let window = parse_sigil_id(window, '@')
            .ok_or_else(|| format!("bad window id in action row: {line:?}"))?;
        if rows
            .iter()
            .any(|row: &ActionWindowRow| row.window == window)
        {
            return Err(format!("duplicate window id @{window} in action listing"));
        }
        rows.push(ActionWindowRow {
            session: session.to_string(),
            window,
            active: parse_tmux_bool(active, line)?,
        });
    }
    Ok(rows)
}

fn parse_action_panes(text: &str) -> Result<Vec<ActionPaneRow>, String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split(SEP).collect();
        let [
            session,
            window,
            pane,
            active,
            width,
            height,
            left,
            top,
            zoomed,
        ] = fields.as_slice()
        else {
            return Err(format!("pane action row with wrong field count: {line:?}"));
        };
        if parse_sigil_id(session, '$').is_none() {
            return Err(format!("bad session id in pane action row: {line:?}"));
        }
        let window = parse_sigil_id(window, '@')
            .ok_or_else(|| format!("bad window id in pane action row: {line:?}"))?;
        let pane = parse_sigil_id(pane, '%')
            .ok_or_else(|| format!("bad pane id in action row: {line:?}"))?;
        if rows.iter().any(|row: &ActionPaneRow| row.pane == pane) {
            return Err(format!("duplicate pane id %{pane} in action listing"));
        }
        rows.push(ActionPaneRow {
            session: (*session).to_string(),
            window,
            pane,
            active: parse_tmux_bool(active, line)?,
            width: parse_action_u32(width, "pane width", line)?,
            height: parse_action_u32(height, "pane height", line)?,
            left: parse_action_u32(left, "pane left", line)?,
            top: parse_action_u32(top, "pane top", line)?,
            zoomed: parse_tmux_bool(zoomed, line)?,
        });
    }
    Ok(rows)
}

/// `#{session_id}\x1f#{window_id}\x1f#{pane_id}\x1f#{pane_current_path}\x1f
/// #{pane_title}`; the title is the remainder field, so a title embedding
/// the separator cannot corrupt the ID fields.
fn parse_panes(text: &str) -> Result<Vec<PaneRow>, String> {
    text.lines()
        .map(|line| {
            let mut parts = line.splitn(5, SEP);
            let (Some(sid), Some(wid), Some(pid), Some(cwd), Some(title)) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) else {
                return Err(format!("pane row with missing fields: {line:?}"));
            };
            if parse_sigil_id(sid, '$').is_none() {
                return Err(format!("bad session id in pane row: {line:?}"));
            }
            let window = parse_sigil_id(wid, '@')
                .ok_or_else(|| format!("bad window id in pane row: {line:?}"))?;
            let pane =
                parse_sigil_id(pid, '%').ok_or_else(|| format!("bad pane id in row: {line:?}"))?;
            let cwd = (!cwd.is_empty()).then(|| cwd.to_string());
            let title = (!title.is_empty()).then(|| title.to_string());
            Ok((sid.to_string(), window, pane, cwd, title))
        })
        .collect()
}

/// Assemble one consistent scan. Any cross-listing inconsistency (a window
/// under an unlisted session, a pane under an unlisted window, a session
/// with no windows, a window with no panes) means the scan raced server
/// mutation and is reported malformed/indeterminate, never guessed at.
fn assemble_rows(
    sessions: &[(String, String)],
    windows: &[(String, u64, String)],
    panes: &[PaneRow],
) -> Result<Vec<NativeSpaceRow>, String> {
    let mut rows: Vec<NativeSpaceRow> = sessions
        .iter()
        .map(|(sid, name)| NativeSpaceRow {
            native_token: sid.clone(),
            native_name: name.clone(),
            groups: Vec::new(),
            multi_window: false,
        })
        .collect();
    for (sid, window, name) in windows {
        let row = rows
            .iter_mut()
            .find(|r| r.native_token == *sid)
            .ok_or_else(|| format!("window @{window} under unlisted session {sid}"))?;
        row.groups.push(NativeGroupRow {
            handle: ProviderHandle::Tx(*window),
            title: (!name.is_empty()).then(|| name.clone()),
            splits: Vec::new(),
        });
    }
    for (sid, window, pane, cwd, title) in panes {
        let row = rows
            .iter_mut()
            .find(|r| r.native_token == *sid)
            .ok_or_else(|| format!("pane %{pane} under unlisted session {sid}"))?;
        let group = row
            .groups
            .iter_mut()
            .find(|g| g.handle == ProviderHandle::Tx(*window))
            .ok_or_else(|| format!("pane %{pane} under unlisted window @{window}"))?;
        group.splits.push(NativeSplitRow {
            handle: ProviderHandle::Tx(*pane),
            title: title.clone(),
            cwd: cwd.clone(),
        });
    }
    for row in &rows {
        if row.groups.is_empty() {
            return Err(format!(
                "session {} listed with no windows",
                row.native_token
            ));
        }
        for group in &row.groups {
            if group.splits.is_empty() {
                return Err(format!("window {} listed with no panes", group.handle));
            }
        }
    }
    Ok(rows)
}

/// `$N|@N|%N` spawn return (ADR 004 frozen format).
fn parse_spawn_return(stdout: &str) -> ProviderResult<(String, u64, u64)> {
    let line = stdout.trim_end_matches('\n');
    let mut parts = line.split('|');
    let (Some(sid), Some(wid), Some(pid), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ProviderError::NativeFailure {
            detail: format!("unexpected spawn return: {line:?}"),
        });
    };
    if parse_sigil_id(sid, '$').is_none() {
        return Err(ProviderError::NativeFailure {
            detail: format!("bad session id in spawn return: {line:?}"),
        });
    }
    let window = parse_sigil_id(wid, '@').ok_or_else(|| ProviderError::NativeFailure {
        detail: format!("bad window id in spawn return: {line:?}"),
    })?;
    let pane = parse_sigil_id(pid, '%').ok_or_else(|| ProviderError::NativeFailure {
        detail: format!("bad pane id in spawn return: {line:?}"),
    })?;
    Ok((sid.to_string(), window, pane))
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

impl<R: TmuxRunner> Provider for TmuxProvider<R> {
    /// Capability probing (plan §17): probed by running against the real
    /// server on this provider's namespace, never by parsing a version
    /// string. Probes run on a scratch `dmux-probe-<uuid>` session that is
    /// created and removed here; a dead/absent server yields no probes.
    /// `cas_rename` is always false: tmux has no compare-and-swap rename and
    /// does not need one (session id `$N` is the immutable identity).
    fn capabilities(&self) -> Capabilities {
        let mut probed = Vec::new();
        let probe_name = format!("dmux-probe-{}", Uuid::new_v4());
        let create = self.run(
            &self.endpoint,
            &[
                "new-session",
                "-d",
                "-P",
                "-F",
                SPAWN_FORMAT,
                "-s",
                &probe_name,
                "--",
                "/bin/sh",
                "-c",
                "sleep 30",
            ],
        );
        let Ok(out) = create else {
            return Capabilities {
                backend: Backend::Tmux,
                cas_rename: false,
                probed,
            };
        };
        if !out.ok() {
            return Capabilities {
                backend: Backend::Tmux,
                cas_rename: false,
                probed,
            };
        }
        if let Ok(stdout) = utf8(&out.stdout)
            && let Ok((sid, wid, _)) = parse_spawn_return(&stdout)
        {
            let wid_target = format!("@{wid}");

            if let Ok(echo) = self.run_ok(
                &self.endpoint,
                &["display-message", "-p", "-t", &sid, "#{session_id}"],
            ) && echo.trim_end_matches('\n') == sid
            {
                probed.push("exact_id_targeting".to_string());
            }

            if self
                .run_ok(
                    &self.endpoint,
                    &["set-option", "-t", &sid, "@dmux_probe", "ok"],
                )
                .is_ok()
                && self
                    .run_ok(
                        &self.endpoint,
                        &["show-options", "-t", &sid, "-qv", "@dmux_probe"],
                    )
                    .is_ok_and(|v| v.trim_end_matches('\n') == "ok")
            {
                probed.push("session_options".to_string());
            }

            if self
                .run_ok(
                    &self.endpoint,
                    &[
                        "set-option",
                        "-w",
                        "-t",
                        &wid_target,
                        "allow-passthrough",
                        "all",
                    ],
                )
                .is_ok()
                && self
                    .run_ok(
                        &self.endpoint,
                        &[
                            "show-options",
                            "-w",
                            "-t",
                            &wid_target,
                            "-qv",
                            "allow-passthrough",
                        ],
                    )
                    .is_ok_and(|v| v.trim_end_matches('\n') == "all")
            {
                probed.push("allow_passthrough_all".to_string());
            }

            // With zero attached clients tmux 3.7b answers "no current
            // client" — which still proves the verb parses and resolves;
            // an unknown verb would say "unknown command".
            match self.run(&self.endpoint, &["detach-client", "-s", &sid]) {
                Ok(out) if out.ok() || lossy(&out.stderr).contains("no current client") => {
                    probed.push("detach_client".to_string());
                }
                _ => {}
            }

            let _ = self.run(&self.endpoint, &["kill-session", "-t", &sid]);
        }
        Capabilities {
            backend: Backend::Tmux,
            cas_rename: false,
            probed,
        }
    }

    /// Complete owner-side scan of one namespace under one epoch. The epoch
    /// is read before and after the listings; a change mid-scan makes the
    /// scan indeterminate (`Malformed`), never a half-truth. `ls` NEVER
    /// writes the epoch option (plan §11.2).
    fn inventory(&self, scope: &InventoryScope) -> InventoryOutcome {
        if scope.backend != Backend::Tmux {
            return InventoryOutcome::Malformed {
                detail: format!("tmux provider handed a {} scope", scope.backend),
            };
        }
        let epoch = match self.read_epoch(&scope.endpoint) {
            Ok(epoch) => epoch,
            Err(EpochFailure::NoServer(detail)) => {
                // Determinate for this owner-local namespace: the socket
                // probe itself classified the server as not running.
                return InventoryOutcome::ServerStopped { detail };
            }
            Err(EpochFailure::MissingBinary(detail)) => {
                return InventoryOutcome::CommandMissing { detail };
            }
            Err(EpochFailure::Timeout(detail)) => return InventoryOutcome::Timeout { detail },
            Err(EpochFailure::Malformed(detail)) => return InventoryOutcome::Malformed { detail },
        };
        if let Some(expected) = scope.expected_epoch
            && epoch != Some(expected)
        {
            return InventoryOutcome::Malformed {
                detail: format!(
                    "backend_epoch_changed: expected {} observed {}",
                    expected.0,
                    epoch.map_or("unepoched".to_string(), |e| e.0.to_string())
                ),
            };
        }

        let mut listings = Vec::with_capacity(3);
        for args in [
            vec!["list-sessions", "-F", SESSIONS_FORMAT],
            vec!["list-windows", "-a", "-F", WINDOWS_FORMAT],
            vec!["list-panes", "-a", "-F", PANES_FORMAT],
        ] {
            let out = match self.run(&scope.endpoint, &args) {
                Ok(out) => out,
                Err(RunError::MissingBinary { detail }) => {
                    return InventoryOutcome::CommandMissing { detail };
                }
                Err(RunError::Timeout { detail }) => return InventoryOutcome::Timeout { detail },
                Err(RunError::Io { detail }) => return InventoryOutcome::Malformed { detail },
            };
            if !out.ok() {
                let stderr = lossy(&out.stderr);
                if no_server_stderr(&stderr) {
                    // The server exited between the epoch read and this
                    // listing (e.g. its last session died).
                    return InventoryOutcome::ServerStopped {
                        detail: stderr.trim().to_string(),
                    };
                }
                return InventoryOutcome::Malformed {
                    detail: format!("tmux {}: {}", args[0], stderr.trim()),
                };
            }
            match utf8(&out.stdout) {
                Ok(text) => listings.push(text),
                Err(detail) => return InventoryOutcome::Malformed { detail },
            }
        }

        match self.read_epoch(&scope.endpoint) {
            Ok(after) if after == epoch => {}
            Ok(after) => {
                return InventoryOutcome::Malformed {
                    detail: format!(
                        "backend_epoch_changed during scan: {:?} -> {:?}",
                        epoch.map(|e| e.0),
                        after.map(|e| e.0)
                    ),
                };
            }
            Err(EpochFailure::NoServer(detail)) => {
                return InventoryOutcome::ServerStopped { detail };
            }
            Err(EpochFailure::MissingBinary(detail)) => {
                return InventoryOutcome::CommandMissing { detail };
            }
            Err(EpochFailure::Timeout(detail)) => return InventoryOutcome::Timeout { detail },
            Err(EpochFailure::Malformed(detail)) => return InventoryOutcome::Malformed { detail },
        }

        let sessions = match parse_sessions(&listings[0]) {
            Ok(s) => s,
            Err(detail) => return InventoryOutcome::Malformed { detail },
        };
        let windows = match parse_windows(&listings[1]) {
            Ok(w) => w,
            Err(detail) => return InventoryOutcome::Malformed { detail },
        };
        let panes = match parse_panes(&listings[2]) {
            Ok(p) => p,
            Err(detail) => return InventoryOutcome::Malformed { detail },
        };
        match assemble_rows(&sessions, &windows, &panes) {
            Ok(rows) => InventoryOutcome::Complete(NativeInventory {
                server_epoch: epoch,
                rows,
            }),
            Err(detail) => InventoryOutcome::Malformed { detail },
        }
    }

    /// `new-session -d -P -F '$N|@N|%N' -s <name> [-c cwd] -- <bootstrap...>`
    /// on the exact namespace. Requires `scope.expected_epoch` (a managed
    /// create on an unepoched server is a typed error; P5 owns server
    /// bootstrap). Note the token asymmetry documented at module level:
    /// `spec.native_token` is the requested session NAME, the returned
    /// binding's `native_token` is the immutable session ID `$N`.
    fn create(&self, scope: &InventoryScope, spec: &CreateSpec) -> ProviderResult<NativeBinding> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        if spec.native_token.is_empty() {
            return Err(ProviderError::NativeFailure {
                detail: "create requires a non-empty session name".into(),
            });
        }
        if spec.bootstrap_argv.is_empty() {
            return Err(ProviderError::NativeFailure {
                detail: "create requires the bootstrap helper argv (ADR 004); \
                         the provider never spawns a bare default shell"
                    .into(),
            });
        }
        self.check_epoch(&scope.endpoint, expected)?;

        let mut args: Vec<String> = vec![
            "new-session".into(),
            "-d".into(),
            "-P".into(),
            "-F".into(),
            SPAWN_FORMAT.into(),
            "-s".into(),
            spec.native_token.clone(),
        ];
        if let Some(cwd) = &spec.cwd {
            args.push("-c".into());
            args.push(cwd.clone());
        }
        args.push("--".into());
        args.extend(spec.bootstrap_argv.iter().cloned());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let stdout = self.run_ok(&scope.endpoint, &arg_refs)?;
        let (sid, window, pane) = parse_spawn_return(&stdout)?;

        self.assert_window_options(&scope.endpoint, window)?;

        // Postcondition: same incarnation, session verifiably present.
        self.check_epoch(&scope.endpoint, expected)?;
        if !self
            .list_session_ids(&scope.endpoint)?
            .iter()
            .any(|s| *s == sid)
        {
            return Err(ProviderError::PostconditionFailed {
                detail: format!("created session {sid} absent from post-create listing"),
            });
        }
        Ok(NativeBinding {
            native_token: sid,
            server_epoch: expected,
            root_group: ProviderHandle::Tx(window),
            root_split: ProviderHandle::Tx(pane),
        })
    }

    /// Read-only validation returning the exact attach argv for a detached
    /// client: `tmux -L <namespace> attach -t '$N'`. The client execs this
    /// argv verbatim; it never builds native target strings itself.
    /// A caller already inside a tmux client must be presented via
    /// `switch-client` instead of nested attach (plan §11.2) — that choice
    /// is orchestration-layer work above this provider, as is focusing the
    /// optional child after attach.
    fn prepare_presentation(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
        child: Option<&ProviderHandle>,
    ) -> ProviderResult<PresentationTarget> {
        Self::scope_check(scope)?;
        validate_session_token(&binding.native_token)?;
        let expected = Self::binding_epoch(scope, binding)?;
        self.check_epoch(&scope.endpoint, expected)?;
        if !self
            .list_session_ids(&scope.endpoint)?
            .iter()
            .any(|s| *s == binding.native_token)
        {
            return Err(ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            });
        }
        if let Some(handle) = child {
            let groups = self.session_rows(&scope.endpoint, &binding.native_token)?;
            let present = groups
                .iter()
                .any(|g| g.handle == *handle || g.splits.iter().any(|s| s.handle == *handle));
            if !present {
                return Err(ProviderError::NotFound {
                    native_ref: handle.to_string(),
                });
            }
        }
        Ok(PresentationTarget::Tmux {
            exact_argv: vec![
                "tmux".to_string(),
                "-L".to_string(),
                scope.endpoint.clone(),
                "attach".to_string(),
                "-t".to_string(),
                binding.native_token.clone(),
            ],
        })
    }

    /// `rename-session -t '$N' <new name>`, verified by re-listing.
    fn rename(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
        new_native_name: &str,
    ) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        validate_session_token(&binding.native_token)?;
        let expected = Self::binding_epoch(scope, binding)?;
        self.check_epoch(&scope.endpoint, expected)?;
        self.run_ok(
            &scope.endpoint,
            &[
                "rename-session",
                "-t",
                &binding.native_token,
                new_native_name,
            ],
        )?;
        let listing = self.run_ok(&scope.endpoint, &["list-sessions", "-F", SESSIONS_FORMAT])?;
        let sessions = parse_sessions(&listing).map_err(malformed_scan)?;
        match sessions
            .iter()
            .find(|(sid, _)| *sid == binding.native_token)
        {
            Some((_, name)) if name == new_native_name => Ok(()),
            Some((_, name)) => Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "rename verified mismatch: {} is named {name:?}, wanted {new_native_name:?}",
                    binding.native_token
                ),
            }),
            None => Err(ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            }),
        }
    }

    /// `kill-session -t '$N'` with verified absence. Absence after a benign
    /// "can't find session" is success (ADR 005); killing the last session
    /// legitimately stops the whole server, which also verifies absence.
    fn remove(&self, scope: &InventoryScope, binding: &NativeBinding) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        validate_session_token(&binding.native_token)?;
        let expected = Self::binding_epoch(scope, binding)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let sid = binding.native_token.clone();
        let needle = sid.clone();
        self.kill_converge(
            &scope.endpoint,
            &["kill-session", "-t", &sid],
            move |this| {
                this.listing_absent(
                    &scope.endpoint,
                    &["list-sessions", "-F", "#{session_id}"],
                    &needle,
                )
            },
        )
    }

    fn group_list(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<Vec<NativeGroupRow>> {
        Self::scope_check(scope)?;
        validate_session_token(&binding.native_token)?;
        let expected = Self::binding_epoch(scope, binding)?;
        self.check_epoch(&scope.endpoint, expected)?;
        self.session_rows(&scope.endpoint, &binding.native_token)
    }

    /// `new-window -P -F '$N|@N|%N' -t '$N' [-n name] [-c cwd] -- <argv>` on
    /// the exact session, epoch-verified immediately before mutation.
    /// `spec.native_token`, when non-empty, becomes the window name.
    fn group_new(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
        spec: &CreateSpec,
    ) -> ProviderResult<ProviderHandle> {
        Self::scope_check(scope)?;
        validate_session_token(&binding.native_token)?;
        let expected = Self::binding_epoch(scope, binding)?;
        if spec.bootstrap_argv.is_empty() {
            return Err(ProviderError::NativeFailure {
                detail: "group_new requires the bootstrap helper argv (ADR 004)".into(),
            });
        }
        self.check_epoch(&scope.endpoint, expected)?;
        let mut args: Vec<String> = vec![
            "new-window".into(),
            "-P".into(),
            "-F".into(),
            SPAWN_FORMAT.into(),
            "-t".into(),
            binding.native_token.clone(),
        ];
        if !spec.native_token.is_empty() {
            args.push("-n".into());
            args.push(spec.native_token.clone());
        }
        if let Some(cwd) = &spec.cwd {
            args.push("-c".into());
            args.push(cwd.clone());
        }
        args.push("--".into());
        args.extend(spec.bootstrap_argv.iter().cloned());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let stdout = self.run_ok(&scope.endpoint, &arg_refs)?;
        let (_, window, _) = parse_spawn_return(&stdout)?;
        self.assert_window_options(&scope.endpoint, window)?;
        Ok(ProviderHandle::Tx(window))
    }

    fn group_activate(
        &self,
        scope: &InventoryScope,
        handle: &ProviderHandle,
    ) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = window_target(handle)?;
        self.check_epoch(&scope.endpoint, expected)?;
        self.run_ok(&scope.endpoint, &["select-window", "-t", &target])?;
        Ok(())
    }

    /// `rename-window -t '@N' <title>`, verified by re-listing.
    fn group_rename(
        &self,
        scope: &InventoryScope,
        handle: &ProviderHandle,
        title: &str,
    ) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = window_target(handle)?;
        self.check_epoch(&scope.endpoint, expected)?;
        self.run_ok(&scope.endpoint, &["rename-window", "-t", &target, title])?;
        let listing = self.run_ok(
            &scope.endpoint,
            &["list-windows", "-a", "-F", WINDOWS_FORMAT],
        )?;
        let windows = parse_windows(&listing).map_err(malformed_scan)?;
        let wanted = window_target(handle)?;
        match windows.iter().find(|(_, w, _)| format!("@{w}") == wanted) {
            Some((_, _, name)) if name == title => Ok(()),
            Some((_, _, name)) => Err(ProviderError::PostconditionFailed {
                detail: format!("group rename verified mismatch: {wanted} is named {name:?}"),
            }),
            None => Err(ProviderError::NotFound { native_ref: wanted }),
        }
    }

    /// `kill-window -t '@N'` with verified absence (ADR 005 semantics).
    fn group_remove(&self, scope: &InventoryScope, handle: &ProviderHandle) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = window_target(handle)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let needle = target.clone();
        self.kill_converge(
            &scope.endpoint,
            &["kill-window", "-t", &target],
            move |this| {
                this.listing_absent(
                    &scope.endpoint,
                    &["list-windows", "-a", "-F", "#{window_id}"],
                    &needle,
                )
            },
        )
    }

    fn split_list(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
    ) -> ProviderResult<Vec<NativeSplitRow>> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = window_target(group)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let listing = self.run_ok(
            &scope.endpoint,
            &["list-panes", "-t", &target, "-F", PANES_FORMAT],
        )?;
        let panes = parse_panes(&listing).map_err(malformed_scan)?;
        Ok(panes
            .into_iter()
            .map(|(_, _, pane, cwd, title)| NativeSplitRow {
                handle: ProviderHandle::Tx(pane),
                title,
                cwd,
            })
            .collect())
    }

    /// `split-window -P -F '$N|@N|%N' -t '%N' [-c cwd] -- <argv>` on the
    /// exact **pane** (the handle in this position is a Split handle `%N`;
    /// kind is positional per model.rs — plan §11.3, the new Split inherits
    /// from a target Split). Direction/percent arrive with P8a.
    fn split_new(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
        split: &SplitSpec,
    ) -> ProviderResult<ProviderHandle> {
        let spec = &split.spec;
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = pane_target(group)?;
        if spec.bootstrap_argv.is_empty() {
            return Err(ProviderError::NativeFailure {
                detail: "split_new requires the bootstrap helper argv (ADR 004)".into(),
            });
        }
        self.check_epoch(&scope.endpoint, expected)?;
        let mut args: Vec<String> = vec![
            "split-window".into(),
            "-P".into(),
            "-F".into(),
            SPAWN_FORMAT.into(),
        ];
        // Deterministic placement argv (plan §7.2): axis flag always
        // explicit, `-b` for the before-side directions.
        match split.direction {
            SplitDirection::Down => args.push("-v".into()),
            SplitDirection::Up => {
                args.push("-v".into());
                args.push("-b".into());
            }
            SplitDirection::Right => args.push("-h".into()),
            SplitDirection::Left => {
                args.push("-h".into());
                args.push("-b".into());
            }
        }
        if let Some(percent) = split.percent {
            args.push("-l".into());
            args.push(format!("{percent}%"));
        }
        args.push("-t".into());
        args.push(target);
        if let Some(cwd) = &spec.cwd {
            args.push("-c".into());
            args.push(cwd.clone());
        }
        args.push("--".into());
        args.extend(spec.bootstrap_argv.iter().cloned());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let stdout = self.run_ok(&scope.endpoint, &arg_refs)?;
        let (_, _, pane) = parse_spawn_return(&stdout)?;
        Ok(ProviderHandle::Tx(pane))
    }

    fn split_activate(
        &self,
        scope: &InventoryScope,
        handle: &ProviderHandle,
    ) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = pane_target(handle)?;
        self.check_epoch(&scope.endpoint, expected)?;
        self.run_ok(&scope.endpoint, &["select-pane", "-t", &target])?;
        Ok(())
    }

    fn activate_group_exact(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
    ) -> ProviderResult<GroupActivationResult> {
        TmuxProvider::activate_group_exact(self, scope, group)
    }

    fn select_split_direction(
        &self,
        scope: &InventoryScope,
        origin: &ProviderHandle,
        direction: SplitDirection,
    ) -> ProviderResult<SplitDirectionResult> {
        TmuxProvider::select_split_direction(self, scope, origin, direction)
    }

    fn resize_split_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
        direction: SplitDirection,
        amount: u16,
    ) -> ProviderResult<SplitResizeResult> {
        TmuxProvider::resize_split_exact(self, scope, split, direction, amount)
    }

    fn toggle_split_zoom_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
    ) -> ProviderResult<SplitZoomResult> {
        TmuxProvider::toggle_split_zoom_exact(self, scope, split)
    }

    /// `kill-pane -t '%N'` with verified absence (ADR 005 semantics).
    fn split_remove(&self, scope: &InventoryScope, handle: &ProviderHandle) -> ProviderResult<()> {
        Self::scope_check(scope)?;
        let expected = Self::required_epoch(scope)?;
        let target = pane_target(handle)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let needle = target.clone();
        self.kill_converge(
            &scope.endpoint,
            &["kill-pane", "-t", &target],
            move |this| {
                this.listing_absent(
                    &scope.endpoint,
                    &["list-panes", "-a", "-F", "#{pane_id}"],
                    &needle,
                )
            },
        )
    }

    fn inspect(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<NativeSpaceRow> {
        Self::scope_check(scope)?;
        validate_session_token(&binding.native_token)?;
        let expected = Self::binding_epoch(scope, binding)?;
        self.check_epoch(&scope.endpoint, expected)?;
        let listing = self.run_ok(&scope.endpoint, &["list-sessions", "-F", SESSIONS_FORMAT])?;
        let sessions = parse_sessions(&listing).map_err(malformed_scan)?;
        let Some((sid, name)) = sessions
            .into_iter()
            .find(|(sid, _)| *sid == binding.native_token)
        else {
            return Err(ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            });
        };
        let groups = self.session_rows(&scope.endpoint, &sid)?;
        Ok(NativeSpaceRow {
            native_token: sid,
            native_name: name,
            groups,
            multi_window: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    const NS: &str = "dmux-test-ns";
    const EPOCH: Uuid = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/tmux/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"))
    }

    struct ScriptedRunner {
        calls: RefCell<Vec<Vec<String>>>,
        script: RefCell<VecDeque<Result<RunOutput, RunError>>>,
    }

    impl ScriptedRunner {
        fn new(script: Vec<Result<RunOutput, RunError>>) -> Self {
            ScriptedRunner {
                calls: RefCell::new(Vec::new()),
                script: RefCell::new(script.into()),
            }
        }
    }

    impl TmuxRunner for &ScriptedRunner {
        fn run(&self, argv: &[String], _deadline: Duration) -> Result<RunOutput, RunError> {
            self.calls.borrow_mut().push(argv.to_vec());
            self.script
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| panic!("unscripted call: {argv:?}"))
        }
    }

    fn ok(stdout: &str) -> Result<RunOutput, RunError> {
        Ok(RunOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }

    fn fail(status: i32, stderr: &str) -> Result<RunOutput, RunError> {
        Ok(RunOutput {
            status,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    fn provider(runner: &ScriptedRunner) -> TmuxProvider<&ScriptedRunner> {
        TmuxProvider::with_runner(NS, runner)
    }

    fn scope(expected: Option<ServerEpoch>) -> InventoryScope {
        InventoryScope {
            backend: Backend::Tmux,
            endpoint: NS.into(),
            expected_epoch: expected,
        }
    }

    fn epoched_scope() -> InventoryScope {
        scope(Some(ServerEpoch(EPOCH)))
    }

    fn argv(args: &[&str]) -> Vec<String> {
        let mut v = vec!["tmux".to_string(), "-L".to_string(), NS.to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        v
    }

    fn epoch_read_argv() -> Vec<String> {
        argv(&["show-options", "-gqv", "@dmux_server_epoch"])
    }

    fn epoch_ok() -> Result<RunOutput, RunError> {
        ok(&format!("{EPOCH}\n"))
    }

    fn binding() -> NativeBinding {
        NativeBinding {
            native_token: "$5".into(),
            server_epoch: ServerEpoch(EPOCH),
            root_group: ProviderHandle::Tx(7),
            root_split: ProviderHandle::Tx(9),
        }
    }

    fn action_windows(rows: &[(&str, u64, bool)]) -> String {
        rows.iter()
            .map(|(session, window, active)| {
                format!("{session}{SEP}@{window}{SEP}{}", u8::from(*active))
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    /// `(session, window, pane, active, width, height, left, top, zoomed)`.
    fn action_panes(rows: &[(&str, u64, u64, bool, u32, u32, u32, u32, bool)]) -> String {
        rows.iter()
            .map(
                |(session, window, pane, active, width, height, left, top, zoomed)| {
                    format!(
                        "{session}{SEP}@{window}{SEP}%{pane}{SEP}{}{SEP}{width}{SEP}{height}\
                         {SEP}{left}{SEP}{top}{SEP}{}",
                        u8::from(*active),
                        u8::from(*zoomed)
                    )
                },
            )
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    // -- inventory ----------------------------------------------------------

    #[test]
    fn inventory_issues_exact_argv_and_parses_fixture_rows() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&fixture("list-sessions.txt")),
            ok(&fixture("list-windows.txt")),
            ok(&fixture("list-panes.txt")),
            epoch_ok(),
        ]);
        let outcome = provider(&runner).inventory(&scope(None));
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&["list-sessions", "-F", "#{session_id}\u{1f}#{session_name}"]),
                argv(&[
                    "list-windows",
                    "-a",
                    "-F",
                    "#{session_id}\u{1f}#{window_id}\u{1f}#{window_name}",
                ]),
                argv(&[
                    "list-panes",
                    "-a",
                    "-F",
                    "#{session_id}\u{1f}#{window_id}\u{1f}#{pane_id}\u{1f}#{pane_current_path}\u{1f}#{pane_title}",
                ]),
                epoch_read_argv(),
            ],
        );
        let InventoryOutcome::Complete(inv) = outcome else {
            panic!("expected complete inventory, got {outcome:?}");
        };
        assert_eq!(inv.server_epoch, Some(ServerEpoch(EPOCH)));
        assert_eq!(
            inv,
            NativeInventory {
                server_epoch: Some(ServerEpoch(EPOCH)),
                rows: vec![
                    NativeSpaceRow {
                        native_token: "$0".into(),
                        native_name: "alpha".into(),
                        multi_window: false,
                        groups: vec![
                            NativeGroupRow {
                                handle: ProviderHandle::Tx(0),
                                title: Some("sh".into()),
                                splits: vec![NativeSplitRow {
                                    handle: ProviderHandle::Tx(0),
                                    title: Some("host.example".into()),
                                    cwd: Some("/Users/x".into()),
                                }],
                            },
                            NativeGroupRow {
                                handle: ProviderHandle::Tx(2),
                                title: Some("build".into()),
                                splits: vec![
                                    NativeSplitRow {
                                        handle: ProviderHandle::Tx(4),
                                        title: None,
                                        cwd: Some("/private/tmp".into()),
                                    },
                                    NativeSplitRow {
                                        handle: ProviderHandle::Tx(5),
                                        title: Some("title with | pipe".into()),
                                        cwd: None,
                                    },
                                ],
                            },
                        ],
                    },
                    NativeSpaceRow {
                        native_token: "$3".into(),
                        native_name: "beta space".into(),
                        multi_window: false,
                        groups: vec![NativeGroupRow {
                            handle: ProviderHandle::Tx(5),
                            title: Some("edit".into()),
                            splits: vec![NativeSplitRow {
                                handle: ProviderHandle::Tx(9),
                                // Title embedding the separator stays whole:
                                // the title is the remainder field.
                                title: Some("weird\u{1f}title".into()),
                                cwd: Some("/home".into()),
                            }],
                        }],
                    },
                ],
            }
        );
    }

    #[test]
    fn inventory_unepoched_server_reports_none_and_never_writes() {
        let runner = ScriptedRunner::new(vec![
            ok("\n"),
            ok(&fixture("list-sessions.txt")),
            ok(&fixture("list-windows.txt")),
            ok(&fixture("list-panes.txt")),
            ok("\n"),
        ]);
        let outcome = provider(&runner).inventory(&scope(None));
        let InventoryOutcome::Complete(inv) = outcome else {
            panic!("expected complete, got {outcome:?}");
        };
        assert_eq!(inv.server_epoch, None);
        // `ls` never sets the option: every issued command is read-only.
        for call in runner.calls.borrow().iter() {
            assert_ne!(
                call[3], "set-option",
                "inventory must never write: {call:?}"
            );
        }
    }

    #[test]
    fn inventory_empty_server_is_complete_and_empty() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok(""), ok(""), ok(""), epoch_ok()]);
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Complete(inv) => assert!(inv.rows.is_empty()),
            other => panic!("expected complete, got {other:?}"),
        }
    }

    #[test]
    fn inventory_no_server_is_server_stopped_for_both_stderr_variants() {
        for stderr in [
            "error connecting to /private/tmp/tmux-501/dmux-x (No such file or directory)",
            "no server running on /private/tmp/tmux-501/dmux-x",
        ] {
            let runner = ScriptedRunner::new(vec![fail(1, stderr)]);
            match provider(&runner).inventory(&scope(None)) {
                InventoryOutcome::ServerStopped { .. } => {}
                other => panic!("expected server_stopped for {stderr:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn inventory_missing_binary_is_command_missing() {
        let runner = ScriptedRunner::new(vec![Err(RunError::MissingBinary {
            detail: "tmux: No such file or directory".into(),
        })]);
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::CommandMissing { .. } => {}
            other => panic!("expected command_missing, got {other:?}"),
        }
    }

    #[test]
    fn inventory_timeout_is_typed() {
        let runner = ScriptedRunner::new(vec![Err(RunError::Timeout {
            detail: "tmux exceeded 10000ms deadline".into(),
        })]);
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Timeout { .. } => {}
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[test]
    fn inventory_malformed_fixtures_are_malformed_outcomes() {
        for (sessions, windows, panes) in [
            // Session row with no separator at all.
            (
                fixture("malformed-sessions.txt"),
                String::new(),
                String::new(),
            ),
            // Pane row with too few fields.
            (
                fixture("list-sessions.txt"),
                fixture("list-windows.txt"),
                fixture("malformed-panes.txt"),
            ),
        ] {
            let runner = ScriptedRunner::new(vec![
                epoch_ok(),
                ok(&sessions),
                ok(&windows),
                ok(&panes),
                epoch_ok(),
            ]);
            match provider(&runner).inventory(&scope(None)) {
                InventoryOutcome::Malformed { .. } => {}
                other => panic!("expected malformed, got {other:?}"),
            }
        }
    }

    #[test]
    fn inventory_garbage_epoch_option_is_malformed() {
        let runner = ScriptedRunner::new(vec![ok("not-a-uuid\n")]);
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("@dmux_server_epoch"), "{detail}");
            }
            other => panic!("expected malformed, got {other:?}"),
        }
    }

    #[test]
    fn inventory_expected_epoch_mismatch_is_typed_malformed() {
        let runner = ScriptedRunner::new(vec![ok(&format!("{}\n", Uuid::nil()))]);
        match provider(&runner).inventory(&epoched_scope()) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("backend_epoch_changed"), "{detail}");
            }
            other => panic!("expected malformed, got {other:?}"),
        }
    }

    #[test]
    fn inventory_epoch_change_mid_scan_is_malformed() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(""),
            ok(""),
            ok(""),
            ok(&format!("{}\n", Uuid::nil())),
        ]);
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("during scan"), "{detail}");
            }
            other => panic!("expected malformed, got {other:?}"),
        }
    }

    // -- create -------------------------------------------------------------

    #[test]
    fn create_issues_exact_argv_and_returns_session_id_not_name() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),       // verify before mutation
            ok("$5|@7|%9\n"), // new-session -P output
            ok(""),           // allow-set-title
            ok(""),           // allow-passthrough
            epoch_ok(),       // verify after
            ok("$0\n$5\n"),   // presence listing
        ]);
        let spec = CreateSpec {
            native_token: "dotfiles".into(),
            cwd: Some("/Users/x/dotfiles".into()),
            bootstrap_argv: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()],
        };
        let binding = provider(&runner).create(&epoched_scope(), &spec).unwrap();
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&[
                    "new-session",
                    "-d",
                    "-P",
                    "-F",
                    "#{session_id}|#{window_id}|#{pane_id}",
                    "-s",
                    "dotfiles",
                    "-c",
                    "/Users/x/dotfiles",
                    "--",
                    "/bin/sh",
                    "-c",
                    "sleep 30",
                ]),
                argv(&["set-option", "-w", "-t", "@7", "allow-set-title", "on"]),
                argv(&["set-option", "-w", "-t", "@7", "allow-passthrough", "all"]),
                epoch_read_argv(),
                argv(&["list-sessions", "-F", "#{session_id}"]),
            ],
        );
        // Asymmetry: requested NAME in, immutable session ID out.
        assert_eq!(
            binding,
            NativeBinding {
                native_token: "$5".into(),
                server_epoch: ServerEpoch(EPOCH),
                root_group: ProviderHandle::Tx(7),
                root_split: ProviderHandle::Tx(9),
            }
        );
    }

    #[test]
    fn create_without_expected_epoch_is_typed_error() {
        let runner = ScriptedRunner::new(vec![]);
        let spec = CreateSpec {
            native_token: "x".into(),
            cwd: None,
            bootstrap_argv: vec!["/bin/true".into()],
        };
        match provider(&runner).create(&scope(None), &spec) {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("expected_epoch"), "{detail}");
            }
            other => panic!("expected wrong_instance, got {other:?}"),
        }
        assert!(runner.calls.borrow().is_empty(), "no command may run");
    }

    #[test]
    fn create_without_bootstrap_argv_is_rejected() {
        let runner = ScriptedRunner::new(vec![]);
        let spec = CreateSpec {
            native_token: "x".into(),
            cwd: None,
            bootstrap_argv: vec![],
        };
        match provider(&runner).create(&epoched_scope(), &spec) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.contains("bootstrap"), "{detail}");
            }
            other => panic!("expected native_failure, got {other:?}"),
        }
    }

    #[test]
    fn create_epoch_mismatch_before_mutation_is_epoch_changed() {
        let runner = ScriptedRunner::new(vec![ok(&format!("{}\n", Uuid::nil()))]);
        let spec = CreateSpec {
            native_token: "x".into(),
            cwd: None,
            bootstrap_argv: vec!["/bin/true".into()],
        };
        match provider(&runner).create(&epoched_scope(), &spec) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, ServerEpoch(EPOCH));
                assert_eq!(observed, Some(ServerEpoch(Uuid::nil())));
            }
            other => panic!("expected epoch_changed, got {other:?}"),
        }
        assert_eq!(
            runner.calls.borrow().len(),
            1,
            "must stop before new-session"
        );
    }

    // -- children -----------------------------------------------------------

    #[test]
    fn group_new_targets_exact_session_and_stamps_new_window() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok("$5|@11|%12\n"), ok(""), ok("")]);
        let spec = CreateSpec {
            native_token: "build".into(),
            cwd: Some("/tmp".into()),
            bootstrap_argv: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()],
        };
        let handle = provider(&runner)
            .group_new(&epoched_scope(), &binding(), &spec)
            .unwrap();
        assert_eq!(handle, ProviderHandle::Tx(11));
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&[
                    "new-window",
                    "-P",
                    "-F",
                    "#{session_id}|#{window_id}|#{pane_id}",
                    "-t",
                    "$5",
                    "-n",
                    "build",
                    "-c",
                    "/tmp",
                    "--",
                    "/bin/sh",
                    "-c",
                    "sleep 30",
                ]),
                argv(&["set-option", "-w", "-t", "@11", "allow-set-title", "on"]),
                argv(&["set-option", "-w", "-t", "@11", "allow-passthrough", "all"]),
            ],
        );
    }

    #[test]
    fn split_new_targets_exact_pane() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok("$5|@7|%13\n")]);
        let spec = CreateSpec {
            native_token: String::new(),
            cwd: None,
            bootstrap_argv: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()],
        };
        let handle = provider(&runner)
            .split_new(&epoched_scope(), &ProviderHandle::Tx(9), &spec.into())
            .unwrap();
        assert_eq!(handle, ProviderHandle::Tx(13));
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&[
                    "split-window",
                    "-P",
                    "-F",
                    "#{session_id}|#{window_id}|#{pane_id}",
                    "-v",
                    "-t",
                    "%9",
                    "--",
                    "/bin/sh",
                    "-c",
                    "sleep 30",
                ]),
            ],
        );
    }

    #[test]
    fn split_new_direction_and_percent_argv_is_deterministic() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok("$5|@7|%13\n")]);
        let split = SplitSpec {
            spec: CreateSpec {
                native_token: String::new(),
                cwd: None,
                bootstrap_argv: vec!["/bin/true".into()],
            },
            direction: SplitDirection::Left,
            percent: Some(30),
        };
        provider(&runner)
            .split_new(&epoched_scope(), &ProviderHandle::Tx(9), &split)
            .unwrap();
        assert_eq!(
            runner.calls.borrow()[1],
            argv(&[
                "split-window",
                "-P",
                "-F",
                "#{session_id}|#{window_id}|#{pane_id}",
                "-h",
                "-b",
                "-l",
                "30%",
                "-t",
                "%9",
                "--",
                "/bin/true",
            ]),
        );
    }

    #[test]
    fn child_mutation_rechecks_epoch_immediately_before_and_fails_typed() {
        let runner = ScriptedRunner::new(vec![ok(&format!("{}\n", Uuid::nil()))]);
        let spec = CreateSpec {
            native_token: String::new(),
            cwd: None,
            bootstrap_argv: vec!["/bin/true".into()],
        };
        match provider(&runner).split_new(&epoched_scope(), &ProviderHandle::Tx(9), &spec.into()) {
            Err(ProviderError::EpochChanged { .. }) => {}
            other => panic!("expected epoch_changed, got {other:?}"),
        }
        assert_eq!(runner.calls.borrow().len(), 1);
    }

    #[test]
    fn child_ops_without_expected_epoch_are_unaddressable() {
        let runner = ScriptedRunner::new(vec![]);
        let p = provider(&runner);
        let handle = ProviderHandle::Tx(3);
        assert!(matches!(
            p.group_activate(&scope(None), &handle),
            Err(ProviderError::WrongInstance { .. })
        ));
        assert!(matches!(
            p.split_remove(&scope(None), &handle),
            Err(ProviderError::WrongInstance { .. })
        ));
    }

    #[test]
    fn activate_and_rename_issue_exact_id_argv() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(""), // select-window
            epoch_ok(),
            ok(""), // select-pane
            epoch_ok(),
            ok(""),                       // rename-window
            ok("$5\u{1f}@7\u{1f}logs\n"), // verify listing
        ]);
        let p = provider(&runner);
        p.group_activate(&epoched_scope(), &ProviderHandle::Tx(7))
            .unwrap();
        p.split_activate(&epoched_scope(), &ProviderHandle::Tx(9))
            .unwrap();
        p.group_rename(&epoched_scope(), &ProviderHandle::Tx(7), "logs")
            .unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls[1], argv(&["select-window", "-t", "@7"]));
        assert_eq!(calls[3], argv(&["select-pane", "-t", "%9"]));
        assert_eq!(calls[5], argv(&["rename-window", "-t", "@7", "logs"]));
        assert_eq!(
            calls[6],
            argv(&[
                "list-windows",
                "-a",
                "-F",
                "#{session_id}\u{1f}#{window_id}\u{1f}#{window_name}",
            ]),
        );
    }

    #[test]
    fn exact_group_activation_dispatches_through_trait_and_verifies_active_window() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&action_windows(&[("$5", 7, false)])),
            epoch_ok(),
            ok(""),
            ok(&action_windows(&[("$5", 7, true)])),
            epoch_ok(),
        ]);
        let concrete = provider(&runner);
        let provider: &dyn Provider = &concrete;
        let result = provider
            .activate_group_exact(&epoched_scope(), &ProviderHandle::Tx(7))
            .unwrap();
        assert_eq!(
            result,
            GroupActivationResult {
                server_epoch: ServerEpoch(EPOCH),
                target: ProviderHandle::Tx(7),
            }
        );
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&["list-windows", "-a", "-F", ACTION_WINDOWS_FORMAT]),
                epoch_read_argv(),
                argv(&["select-window", "-t", "@7"]),
                argv(&["list-windows", "-a", "-F", ACTION_WINDOWS_FORMAT]),
                epoch_read_argv(),
            ]
        );
    }

    #[test]
    fn exact_direction_selects_from_origin_and_returns_exact_target_or_edge_none() {
        let pre = action_panes(&[
            ("$5", 7, 9, true, 40, 24, 0, 0, false),
            ("$5", 7, 10, false, 40, 24, 40, 0, false),
        ]);
        let post = action_panes(&[
            ("$5", 7, 9, false, 40, 24, 0, 0, false),
            ("$5", 7, 10, true, 40, 24, 40, 0, false),
        ]);
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&pre),
            epoch_ok(),
            ok(""),
            ok(""),
            ok(&post),
            epoch_ok(),
        ]);
        let result = provider(&runner)
            .select_split_direction(
                &epoched_scope(),
                &ProviderHandle::Tx(9),
                SplitDirection::Right,
            )
            .unwrap();
        assert_eq!(result.target, Some(ProviderHandle::Tx(10)));
        let calls = runner.calls.borrow();
        assert_eq!(calls[3], argv(&["select-pane", "-t", "%9"]));
        assert_eq!(calls[4], argv(&["select-pane", "-t", "%9", "-R"]));

        let edge = action_panes(&[("$5", 7, 9, true, 80, 24, 0, 0, false)]);
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&edge),
            epoch_ok(),
            ok(""),
            ok(""),
            ok(&edge),
            epoch_ok(),
        ]);
        let result = provider(&runner)
            .select_split_direction(
                &epoched_scope(),
                &ProviderHandle::Tx(9),
                SplitDirection::Left,
            )
            .unwrap();
        assert_eq!(
            result.target, None,
            "edge is a no-op, never an ordinal guess"
        );
    }

    #[test]
    fn exact_resize_and_zoom_emit_exact_argv_and_verify_postconditions() {
        let resize_pre = action_panes(&[
            ("$5", 7, 9, true, 40, 24, 0, 0, false),
            ("$5", 7, 10, false, 40, 24, 40, 0, false),
        ]);
        let resize_post = action_panes(&[
            ("$5", 7, 9, true, 43, 24, 0, 0, false),
            ("$5", 7, 10, false, 37, 24, 43, 0, false),
        ]);
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&resize_pre),
            epoch_ok(),
            ok(""),
            ok(&resize_post),
            epoch_ok(),
        ]);
        let result = provider(&runner)
            .resize_split_exact(
                &epoched_scope(),
                &ProviderHandle::Tx(9),
                SplitDirection::Right,
                3,
            )
            .unwrap();
        assert!(result.changed);
        assert_eq!(
            runner.calls.borrow()[3],
            argv(&["resize-pane", "-t", "%9", "-R", "3"])
        );

        let zoom_pre = action_panes(&[("$5", 7, 9, true, 80, 24, 0, 0, false)]);
        let zoom_post = action_panes(&[("$5", 7, 9, true, 80, 24, 0, 0, true)]);
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&zoom_pre),
            epoch_ok(),
            ok(""),
            ok(&zoom_post),
            epoch_ok(),
        ]);
        let result = provider(&runner)
            .toggle_split_zoom_exact(&epoched_scope(), &ProviderHandle::Tx(9))
            .unwrap();
        assert!(result.zoomed);
        assert_eq!(
            runner.calls.borrow()[3],
            argv(&["resize-pane", "-Z", "-t", "%9"])
        );
    }

    #[test]
    fn exact_actions_fail_typed_on_native_failure_and_bad_postcondition() {
        let listing = action_windows(&[("$5", 7, false)]);
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&listing),
            epoch_ok(),
            fail(1, "can't select window"),
        ]);
        assert!(matches!(
            provider(&runner).activate_group_exact(&epoched_scope(), &ProviderHandle::Tx(7)),
            Err(ProviderError::NativeFailure { .. })
        ));

        let zoom = action_panes(&[("$5", 7, 9, true, 80, 24, 0, 0, false)]);
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(&zoom),
            epoch_ok(),
            ok(""),
            ok(&zoom),
            epoch_ok(),
        ]);
        match provider(&runner).toggle_split_zoom_exact(&epoched_scope(), &ProviderHandle::Tx(9)) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.contains("zoom postcondition failed"), "{detail}");
            }
            other => panic!("unchanged zoom must fail postcondition, got {other:?}"),
        }
    }

    #[test]
    fn exact_actions_refuse_unpinned_ids_without_spawning() {
        let runner = ScriptedRunner::new(vec![]);
        assert!(matches!(
            provider(&runner).resize_split_exact(
                &scope(None),
                &ProviderHandle::Tx(9),
                SplitDirection::Right,
                3,
            ),
            Err(ProviderError::WrongInstance { .. })
        ));
        assert!(runner.calls.borrow().is_empty());
    }

    // -- rename/remove ------------------------------------------------------

    #[test]
    fn rename_issues_exact_argv_and_verifies_postcondition() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok(""), ok("$5\u{1f}new name\n")]);
        provider(&runner)
            .rename(&epoched_scope(), &binding(), "new name")
            .unwrap();
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&["rename-session", "-t", "$5", "new name"]),
                argv(&["list-sessions", "-F", "#{session_id}\u{1f}#{session_name}"]),
            ],
        );
    }

    #[test]
    fn rename_unverified_is_postcondition_failed() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok(""), ok("$5\u{1f}other\n")]);
        match provider(&runner).rename(&epoched_scope(), &binding(), "wanted") {
            Err(ProviderError::PostconditionFailed { .. }) => {}
            other => panic!("expected postcondition_failed, got {other:?}"),
        }
    }

    #[test]
    fn remove_kills_by_exact_id_and_verifies_absence() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(""),         // kill-session
            ok("$0\n$9\n"), // listing no longer contains $5
        ]);
        provider(&runner)
            .remove(&epoched_scope(), &binding())
            .unwrap();
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&["kill-session", "-t", "$5"]),
                argv(&["list-sessions", "-F", "#{session_id}"]),
            ],
        );
    }

    #[test]
    fn remove_benign_already_dead_plus_verified_absence_is_success() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            fail(1, "can't find session: $5"),
            ok("$0\n"),
        ]);
        provider(&runner)
            .remove(&epoched_scope(), &binding())
            .unwrap();
    }

    #[test]
    fn remove_of_last_session_stopping_the_server_is_verified_absence() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(""),
            fail(1, "no server running on /private/tmp/tmux-501/dmux-x"),
        ]);
        provider(&runner)
            .remove(&epoched_scope(), &binding())
            .unwrap();
    }

    #[test]
    fn remove_non_convergence_is_postcondition_failed_not_silent() {
        let mut script = Vec::new();
        script.push(epoch_ok());
        for _ in 0..REMOVE_ROUNDS {
            script.push(ok("")); // kill-session "succeeds"
            script.push(ok("$5\n")); // ...but the session is still there
        }
        let runner = ScriptedRunner::new(script);
        match provider(&runner).remove(&epoched_scope(), &binding()) {
            Err(ProviderError::PostconditionFailed { .. }) => {}
            other => panic!("expected postcondition_failed, got {other:?}"),
        }
    }

    #[test]
    fn group_and_split_remove_use_exact_ids_with_verified_absence() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(""),
            ok("@0\n@2\n"),
            epoch_ok(),
            fail(1, "can't find pane: %9"),
            ok("%0\n"),
        ]);
        let p = provider(&runner);
        p.group_remove(&epoched_scope(), &ProviderHandle::Tx(7))
            .unwrap();
        p.split_remove(&epoched_scope(), &ProviderHandle::Tx(9))
            .unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls[1], argv(&["kill-window", "-t", "@7"]));
        assert_eq!(
            calls[2],
            argv(&["list-windows", "-a", "-F", "#{window_id}"])
        );
        assert_eq!(calls[4], argv(&["kill-pane", "-t", "%9"]));
        assert_eq!(calls[5], argv(&["list-panes", "-a", "-F", "#{pane_id}"]));
    }

    // -- presentation -------------------------------------------------------

    #[test]
    fn prepare_presentation_returns_exact_attach_argv() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok("$5\n")]);
        let target = provider(&runner)
            .prepare_presentation(&epoched_scope(), &binding(), None)
            .unwrap();
        assert_eq!(
            target,
            PresentationTarget::Tmux {
                exact_argv: vec![
                    "tmux".into(),
                    "-L".into(),
                    NS.into(),
                    "attach".into(),
                    "-t".into(),
                    "$5".into(),
                ],
            }
        );
    }

    #[test]
    fn prepare_presentation_missing_session_is_not_found() {
        let runner = ScriptedRunner::new(vec![epoch_ok(), ok("$0\n")]);
        match provider(&runner).prepare_presentation(&epoched_scope(), &binding(), None) {
            Err(ProviderError::NotFound { native_ref }) => assert_eq!(native_ref, "$5"),
            other => panic!("expected not_found, got {other:?}"),
        }
    }

    #[test]
    fn stale_binding_epoch_is_rejected_before_any_command() {
        let runner = ScriptedRunner::new(vec![]);
        let stale = NativeBinding {
            server_epoch: ServerEpoch(Uuid::nil()),
            ..binding()
        };
        match provider(&runner).prepare_presentation(&epoched_scope(), &stale, None) {
            Err(ProviderError::EpochChanged { .. }) => {}
            other => panic!("expected epoch_changed, got {other:?}"),
        }
        assert!(runner.calls.borrow().is_empty());
    }

    // -- markers ------------------------------------------------------------

    #[test]
    fn marker_stamping_issues_exact_argv_and_verifies_readback() {
        let runner = ScriptedRunner::new(vec![
            epoch_ok(),
            ok(""),
            ok(""),
            ok(""),
            ok(""),
            // read_markers path: epoch verify + four reads
            epoch_ok(),
            ok("host-uid\n"),
            ok("registry-uid\n"),
            ok("space-uid\n"),
            ok("42\n"),
        ]);
        let markers = SpaceMarkers {
            host_uid: "host-uid".into(),
            registry_uid: "registry-uid".into(),
            space_uid: "space-uid".into(),
            space_no: "42".into(),
        };
        provider(&runner)
            .stamp_markers(&epoched_scope(), "$5", &markers)
            .unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(
            calls[1],
            argv(&["set-option", "-t", "$5", "@dmux_host_uid", "host-uid"])
        );
        assert_eq!(
            calls[2],
            argv(&[
                "set-option",
                "-t",
                "$5",
                "@dmux_registry_uid",
                "registry-uid"
            ]),
        );
        assert_eq!(
            calls[3],
            argv(&["set-option", "-t", "$5", "@dmux_space_uid", "space-uid"])
        );
        assert_eq!(
            calls[4],
            argv(&["set-option", "-t", "$5", "@dmux_space_no", "42"])
        );
        assert_eq!(
            calls[6],
            argv(&["show-options", "-t", "$5", "-qv", "@dmux_host_uid"])
        );
        assert_eq!(
            calls[9],
            argv(&["show-options", "-t", "$5", "-qv", "@dmux_space_no"])
        );
    }

    #[test]
    fn marker_readback_maps_absent_to_none() {
        let runner = ScriptedRunner::new(vec![ok("host-uid\n"), ok(""), ok(""), ok("")]);
        let readback = provider(&runner).read_markers(&scope(None), "$5").unwrap();
        assert_eq!(
            readback,
            SpaceMarkerReadback {
                host_uid: Some("host-uid".into()),
                registry_uid: None,
                space_uid: None,
                space_no: None,
            }
        );
    }

    // -- capabilities -------------------------------------------------------

    #[test]
    fn capabilities_probes_by_running_and_cleans_up() {
        let runner = ScriptedRunner::new(vec![
            ok("$8|@8|%8\n"),             // probe session create
            ok("$8\n"),                   // display-message echo
            ok(""),                       // set session option
            ok("ok\n"),                   // show session option
            ok(""),                       // set allow-passthrough
            ok("all\n"),                  // show allow-passthrough
            fail(1, "no current client"), // detach-client with zero clients
            ok(""),                       // kill probe session
        ]);
        let caps = provider(&runner).capabilities();
        assert_eq!(caps.backend, Backend::Tmux);
        assert!(
            !caps.cas_rename,
            "tmux has no CAS rename (ADR 006 is Wez-only)"
        );
        assert_eq!(
            caps.probed,
            vec![
                "exact_id_targeting",
                "session_options",
                "allow_passthrough_all",
                "detach_client",
            ],
        );
        let calls = runner.calls.borrow();
        assert_eq!(calls[0][3], "new-session");
        assert_eq!(
            calls[1],
            argv(&["display-message", "-p", "-t", "$8", "#{session_id}"])
        );
        assert_eq!(calls[6], argv(&["detach-client", "-s", "$8"]),);
        assert_eq!(calls[7], argv(&["kill-session", "-t", "$8"]));
    }

    #[test]
    fn capabilities_with_no_server_probes_nothing() {
        let runner = ScriptedRunner::new(vec![fail(
            1,
            "error connecting to /private/tmp/tmux-501/dmux-x (No such file or directory)",
        )]);
        let caps = provider(&runner).capabilities();
        assert!(caps.probed.is_empty());
        assert!(!caps.cas_rename);
    }

    // -- P5 epoch bootstrap (plan §11.2) -------------------------------------

    const PID: u32 = 45159;
    const START: &str = "1786887235";

    fn identity_read_argv() -> Vec<String> {
        argv(&[
            "list-sessions",
            "-F",
            "#{pid}\u{1f}#{start_time}\u{1f}#{socket_path}",
        ])
    }

    fn identity_ok() -> Result<RunOutput, RunError> {
        ok(&format!(
            "{PID}\u{1f}{START}\u{1f}/private/tmp/tmux-501/{NS}\n"
        ))
    }

    fn identity() -> TmuxServerIdentity {
        TmuxServerIdentity {
            pid: PID,
            start_token: START.into(),
        }
    }

    fn assert_read_only(runner: &ScriptedRunner) {
        for call in runner.calls.borrow().iter() {
            assert_ne!(
                call[3], "set-option",
                "epoch probe/verify must never write: {call:?}"
            );
        }
    }

    #[test]
    fn server_identity_issues_exact_argv_and_parses_pid_and_start_token() {
        // Two sessions → two identical server-scoped rows; first is used.
        let runner = ScriptedRunner::new(vec![ok(&format!(
            "{PID}\u{1f}{START}\u{1f}/private/tmp/tmux-501/{NS}\n\
             {PID}\u{1f}{START}\u{1f}/private/tmp/tmux-501/{NS}\n"
        ))]);
        let id = provider(&runner).server_identity(NS).unwrap();
        assert_eq!(id, identity());
        assert_eq!(*runner.calls.borrow(), vec![identity_read_argv()]);
        assert_read_only(&runner);
    }

    #[test]
    fn server_identity_no_server_is_typed_native_failure() {
        let runner = ScriptedRunner::new(vec![fail(
            1,
            "error connecting to /private/tmp/tmux-501/dmux-x (No such file or directory)",
        )]);
        match provider(&runner).server_identity(NS) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.contains("no tmux server"), "{detail}");
            }
            other => panic!("expected native_failure, got {other:?}"),
        }
    }

    #[test]
    fn server_identity_disagreeing_rows_are_malformed() {
        let runner = ScriptedRunner::new(vec![ok(&format!(
            "{PID}\u{1f}{START}\u{1f}/s\n{PID}\u{1f}9999999999\u{1f}/s\n"
        ))]);
        match provider(&runner).server_identity(NS) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.contains("disagree"), "{detail}");
            }
            other => panic!("expected native_failure, got {other:?}"),
        }
    }

    #[test]
    fn server_identity_empty_start_time_falls_back_to_socket_inode() {
        // Simulate a tmux without #{start_time}: the field expands empty and
        // the token is derived from the resolved socket's device+inode.
        let sock = std::env::temp_dir().join(format!("dmux-idtest-{}", std::process::id()));
        std::fs::write(&sock, b"").expect("create fake socket file");
        let runner =
            ScriptedRunner::new(vec![ok(&format!("{PID}\u{1f}\u{1f}{}\n", sock.display()))]);
        let id = provider(&runner).server_identity(NS).unwrap();
        let meta = std::fs::metadata(&sock).unwrap();
        let _ = std::fs::remove_file(&sock);
        use std::os::unix::fs::MetadataExt;
        assert_eq!(id.pid, PID);
        assert_eq!(id.start_token, format!("ino:{}:{}", meta.dev(), meta.ino()));
    }

    #[test]
    fn set_epoch_if_absent_sets_verifies_readback_and_is_the_only_writer() {
        let runner = ScriptedRunner::new(vec![
            ok("\n"),   // read: absent
            ok(""),     // set-option -g
            epoch_ok(), // readback equals what was written
        ]);
        let outcome = provider(&runner)
            .set_epoch_if_absent(NS, ServerEpoch(EPOCH))
            .unwrap();
        assert_eq!(outcome, EpochSetOutcome::Set);
        assert_eq!(
            *runner.calls.borrow(),
            vec![
                epoch_read_argv(),
                argv(&["set-option", "-g", "@dmux_server_epoch", &EPOCH.to_string()]),
                epoch_read_argv(),
            ],
        );
    }

    #[test]
    fn set_epoch_if_absent_present_value_is_already_set_without_writing() {
        let other = Uuid::new_v4();
        let runner = ScriptedRunner::new(vec![ok(&format!("{other}\n"))]);
        let outcome = provider(&runner)
            .set_epoch_if_absent(NS, ServerEpoch(EPOCH))
            .unwrap();
        assert_eq!(outcome, EpochSetOutcome::AlreadySet(ServerEpoch(other)));
        // Present → exactly one read-only command, no set-option.
        assert_eq!(*runner.calls.borrow(), vec![epoch_read_argv()]);
    }

    #[test]
    fn set_epoch_if_absent_external_racer_winning_readback_is_already_set() {
        let racer = Uuid::new_v4();
        let runner = ScriptedRunner::new(vec![
            ok("\n"),                  // read: absent
            ok(""),                    // our set-option succeeds...
            ok(&format!("{racer}\n")), // ...but an external write won
        ]);
        let outcome = provider(&runner)
            .set_epoch_if_absent(NS, ServerEpoch(EPOCH))
            .unwrap();
        assert_eq!(outcome, EpochSetOutcome::AlreadySet(ServerEpoch(racer)));
    }

    #[test]
    fn set_epoch_if_absent_vanishing_readback_is_postcondition_failed() {
        let runner = ScriptedRunner::new(vec![ok("\n"), ok(""), ok("\n")]);
        match provider(&runner).set_epoch_if_absent(NS, ServerEpoch(EPOCH)) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.contains("absent immediately after set"), "{detail}");
            }
            other => panic!("expected postcondition_failed, got {other:?}"),
        }
    }

    #[test]
    fn set_epoch_if_absent_malformed_existing_value_is_typed_and_never_overwritten() {
        let runner = ScriptedRunner::new(vec![ok("not-a-uuid\n")]);
        match provider(&runner).set_epoch_if_absent(NS, ServerEpoch(EPOCH)) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.contains("@dmux_server_epoch"), "{detail}");
            }
            other => panic!("expected native_failure, got {other:?}"),
        }
        assert_read_only(&runner);
    }

    #[test]
    fn verify_epoch_rechecks_identity_then_epoch_read_only() {
        let runner = ScriptedRunner::new(vec![identity_ok(), epoch_ok()]);
        provider(&runner)
            .verify_epoch(NS, ServerEpoch(EPOCH), &identity())
            .unwrap();
        assert_eq!(
            *runner.calls.borrow(),
            vec![identity_read_argv(), epoch_read_argv()],
        );
        assert_read_only(&runner);
    }

    #[test]
    fn verify_epoch_pid_mismatch_is_wrong_instance_before_epoch_read() {
        let runner = ScriptedRunner::new(vec![identity_ok()]);
        let expected = TmuxServerIdentity {
            pid: PID + 1,
            ..identity()
        };
        match provider(&runner).verify_epoch(NS, ServerEpoch(EPOCH), &expected) {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("incarnation changed"), "{detail}");
            }
            other => panic!("expected wrong_instance, got {other:?}"),
        }
        assert_eq!(runner.calls.borrow().len(), 1, "must stop at identity");
    }

    #[test]
    fn verify_epoch_start_token_mismatch_is_wrong_instance() {
        let runner = ScriptedRunner::new(vec![identity_ok()]);
        let expected = TmuxServerIdentity {
            start_token: "1".into(),
            ..identity()
        };
        match provider(&runner).verify_epoch(NS, ServerEpoch(EPOCH), &expected) {
            Err(ProviderError::WrongInstance { .. }) => {}
            other => panic!("expected wrong_instance, got {other:?}"),
        }
    }

    #[test]
    fn verify_epoch_same_identity_different_epoch_is_epoch_changed() {
        let runner = ScriptedRunner::new(vec![identity_ok(), ok(&format!("{}\n", Uuid::nil()))]);
        match provider(&runner).verify_epoch(NS, ServerEpoch(EPOCH), &identity()) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, ServerEpoch(EPOCH));
                assert_eq!(observed, Some(ServerEpoch(Uuid::nil())));
            }
            other => panic!("expected epoch_changed, got {other:?}"),
        }
    }

    #[test]
    fn identity_parser_rejects_noise() {
        for bad in [
            "",
            "abc\u{1f}123\u{1f}/s\n",
            "42\u{1f}12x3\u{1f}/s\n",
            "42\u{1f}123\n",
            "42\u{1f}123\u{1f}\n",
        ] {
            assert!(parse_identity(bad).is_err(), "{bad:?} must be rejected");
        }
        assert_eq!(
            parse_identity("42\u{1f}123\u{1f}/a\u{1f}b\n").unwrap(),
            (42, "123".into(), "/a\u{1f}b".into()),
            "socket path is the remainder field"
        );
    }

    // -- parsers ------------------------------------------------------------

    #[test]
    fn spawn_return_parses_frozen_format_and_rejects_noise() {
        assert_eq!(
            parse_spawn_return("$5|@7|%9\n").unwrap(),
            ("$5".into(), 7, 9)
        );
        for bad in [
            "",
            "$5|@7",
            "$5|@7|%9|extra",
            "5|@7|%9",
            "$5|7|%9",
            "$5|@7|9",
        ] {
            assert!(parse_spawn_return(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn wez_handles_are_rejected_by_tmux_targeting() {
        assert!(window_target(&ProviderHandle::Wz(1)).is_err());
        assert!(pane_target(&ProviderHandle::Opaque("x".into())).is_err());
        assert_eq!(window_target(&ProviderHandle::Tx(4)).unwrap(), "@4");
        assert_eq!(pane_target(&ProviderHandle::Tx(4)).unwrap(), "%4");
    }
}
