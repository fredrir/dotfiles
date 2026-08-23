//! The scope a provider call is made under: which backend, which exact
//! endpoint, and the epoch the caller expects that endpoint to serve.
//!
//! Lives in its own module so that the adapters (`backend::wez`,
//! `backend::tmux`) are not module descendants of the type: a private field
//! here is private to them too. Report 05 measured that with the field
//! private inside `backend/mod.rs` the adapters kept full access, which is
//! exactly the hatch this boundary exists to close (ADR 012 WS-A.1).
//!
//! This module deliberately runs no adapter code: the liveness probe a
//! resolution needs is a trait ([`IncarnationProbe`]), with an OS-only
//! implementation here ([`OsIncarnationProbe`]) and a server-asking one
//! supplied by callers that already hold an adapter (`ls_cli`).

use std::fmt;
use std::path::PathBuf;

use crate::model::{Backend, BackendInstanceUid, ServerEpoch};
use crate::registry::{
    BackendServerRecord, HolderLiveness, Registry, Result as RegistryResult, probe_pid,
};

/// Scope for one provider call: the backend, the exact endpoint, and — for a
/// managed instance — the epoch the registry has published for it.
///
/// The epoch is private. There are exactly two ways to build a scope, and
/// they are not interchangeable:
///
/// * [`InventoryScope::managed`] — the endpoint came from a registry instance
///   and the caller holds that instance's *published* epoch. The provider
///   verifies the live server against it, refuses with
///   `backend_epoch_changed` on mismatch, and discards native IDs from a
///   server that did not match.
/// * [`InventoryScope::unmanaged_endpoint`] — nothing in the registry vouches
///   for the endpoint: a first-contact tmux namespace reached only after
///   `backend_instance_for_backend` returned `None`, or the hidden `--socket`
///   test seam. A scan under it trusts whatever answers, so no mutation may
///   run under it and no durable registry row may be minted from what it
///   observes.
///
/// A registry instance whose published epoch is `NULL` is **not** unmanaged;
/// it is a managed instance nobody has verified yet (ADR 012 §4, review
/// report 05). Turning that `None` into `unmanaged_endpoint` is the defect
/// class this boundary exists to close, which is why every
/// `unmanaged_endpoint` call site in `src/` is held to an explicit allowlist
/// by the audit test in `tests/`, and why there is no constructor that takes
/// an `Option<ServerEpoch>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryScope {
    pub backend: Backend,
    /// Exact socket path (Wez service socket / tmux `-L` namespace socket).
    pub endpoint: String,
    expected_epoch: Option<ServerEpoch>,
}

impl InventoryScope {
    /// A scope for a registry-managed instance whose published epoch the
    /// caller holds. The adapters verify the live server against `epoch`
    /// before trusting anything it says.
    pub fn managed(backend: Backend, endpoint: impl Into<String>, epoch: ServerEpoch) -> Self {
        Self {
            backend,
            endpoint: endpoint.into(),
            expected_epoch: Some(epoch),
        }
    }

    /// A scope for an endpoint the registry does not vouch for. Discovery
    /// only: reads under it are unverified and mutations refuse. Every call
    /// site is named in the audit allowlist; add one only with a reason the
    /// allowlist can quote.
    pub fn unmanaged_endpoint(backend: Backend, endpoint: impl Into<String>) -> Self {
        Self {
            backend,
            endpoint: endpoint.into(),
            expected_epoch: None,
        }
    }

    /// The epoch a managed scope pins, or `None` for an unmanaged endpoint.
    pub fn expected_epoch(&self) -> Option<ServerEpoch> {
        self.expected_epoch
    }
}

// ---------------------------------------------------------------------------
// Incarnation witnesses (ADR 001/002; ADR 012 WS-B.1, plan §5.2 state F)

/// What the registry published with an incarnation: the epoch and the
/// witnesses of the process that published it (ADR 001/002; WS-A.8/A.9).
/// A published epoch is not proof of a live server — a crashed or replaced
/// server leaves its row exactly as it published it — so a reader compares
/// these against a fresh observation before trusting the epoch. Rows that
/// predate a witness carry `None` there and are compared on what they have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedIncarnation {
    pub epoch: ServerEpoch,
    pub pid: Option<i64>,
    pub start_token: Option<String>,
    pub socket_dev: Option<i64>,
    pub socket_ino: Option<i64>,
}

impl PublishedIncarnation {
    /// The published incarnation a registry row carries, or `None` when the
    /// row publishes no epoch (states C/D, or retired).
    pub fn from_record(record: &BackendServerRecord) -> Option<Self> {
        Some(PublishedIncarnation {
            epoch: record.server_epoch?,
            pid: record.server_pid,
            start_token: record.server_start_token.clone(),
            socket_dev: record.socket_dev,
            socket_ino: record.socket_ino,
        })
    }
}

impl fmt::Display for PublishedIncarnation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "epoch {} pid {} start {} socket dev/ino {}/{}",
            self.epoch.0,
            opt(self.pid),
            opt(self.start_token.as_deref()),
            opt(self.socket_dev),
            opt(self.socket_ino)
        )
    }
}

fn opt<T: fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "none".to_string(), |value| value.to_string())
}

/// What a fresh probe of the host found for a published incarnation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedIncarnation {
    /// `kill(pid, 0)` says no such process: the published process exited.
    ProcessDead { pid: i64 },
    /// A process holds the published pid, but the endpoint answers with no
    /// server at all: the pid was reused by something that is not the
    /// published server.
    NoServer { pid: i64, detail: String },
    /// The witnesses of the process and socket the probe could read. `None`
    /// is a witness the probe *cannot* observe (the OS probe does not know a
    /// tmux server's self-reported start token), never one it observed as
    /// absent — an absent witness is a probe failure, not an observation.
    Process {
        pid: i64,
        start_token: Option<String>,
        socket_dev: Option<i64>,
        socket_ino: Option<i64>,
    },
}

impl fmt::Display for ObservedIncarnation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObservedIncarnation::ProcessDead { pid } => {
                write!(f, "process {pid} is dead (no such process)")
            }
            ObservedIncarnation::NoServer { pid, detail } => {
                write!(f, "process {pid} is alive but no server answers: {detail}")
            }
            ObservedIncarnation::Process {
                pid,
                start_token,
                socket_dev,
                socket_ino,
            } => write!(
                f,
                "pid {pid} alive, start {}, socket dev/ino {}/{}",
                start_token
                    .as_deref()
                    .map_or_else(|| "unobserved".to_string(), str::to_string),
                socket_dev.map_or_else(|| "unobserved".to_string(), |v| v.to_string()),
                socket_ino.map_or_else(|| "unobserved".to_string(), |v| v.to_string()),
            ),
        }
    }
}

/// The liveness probe a resolution runs against a published incarnation.
/// The resolver has already proven the published pid alive with
/// `kill(pid, 0)` before calling this; the probe's job is to read the
/// remaining witnesses of that process and its socket. `Err` means the
/// probe could not run at all (binary missing, timeout): nothing was
/// observed, so nothing is proven stale, and the pinned scan downstream
/// reports the failure as itself.
pub trait IncarnationProbe {
    fn observe(
        &self,
        backend: Backend,
        endpoint: &str,
        published: &PublishedIncarnation,
    ) -> Result<ObservedIncarnation, String>;
}

/// The probe every resolution runs by default: operating-system witnesses
/// only, no adapter command. For a Wez instance that is the complete check —
/// the descriptor's `start_token` is the OS process start witness
/// (`runtime::process_start_token`) and the endpoint is the socket path,
/// so pid, start token and socket dev/ino are all comparable from here. For
/// a tmux instance the socket is `stat`ed at the `-L` namespace's socket
/// path; the start token is the server's own `#{start_time}` self-report,
/// which only the server can answer, so it is left unobserved here and
/// compared by the probe `ls` supplies (`ls_cli::LiveIncarnationProbe`) and
/// by `operations::verify_published_incarnation` before any mutation.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsIncarnationProbe;

impl IncarnationProbe for OsIncarnationProbe {
    fn observe(
        &self,
        backend: Backend,
        endpoint: &str,
        published: &PublishedIncarnation,
    ) -> Result<ObservedIncarnation, String> {
        let pid = published
            .pid
            .ok_or_else(|| "the registry publishes no pid to observe".to_string())?;
        let start_token = match backend {
            Backend::Wez => {
                let pid = u32::try_from(pid)
                    .map_err(|_| format!("published pid {pid} is not a process id"))?;
                Some(
                    crate::runtime::process_start_token(pid)
                        .map_err(|e| format!("process {pid} start identity: {e}"))?,
                )
            }
            Backend::Tmux => None,
        };
        let socket = match backend {
            Backend::Wez => PathBuf::from(endpoint),
            Backend::Tmux => tmux_socket_path(endpoint),
        };
        let (socket_dev, socket_ino) = stat_socket(&socket)?;
        Ok(ObservedIncarnation::Process {
            pid,
            start_token,
            socket_dev: Some(socket_dev),
            socket_ino: Some(socket_ino),
        })
    }
}

/// Where tmux binds a `-L` namespace's socket, by tmux's own rule
/// (`make_label`): `$TMUX_TMPDIR` when set and non-empty, else `/tmp`, then
/// `tmux-<uid>/<label>`. The adapter asks the server for `#{socket_path}`;
/// this resolver has no server to ask and reads the same environment the
/// adapter's `tmux -L` would.
pub fn tmux_socket_path(namespace: &str) -> PathBuf {
    let base = std::env::var_os("TMUX_TMPDIR")
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    base.join(format!("tmux-{}", unsafe { libc::getuid() }))
        .join(namespace)
}

fn stat_socket(path: &PathBuf) -> Result<(i64, i64), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    Ok((
        i64::try_from(meta.dev()).map_err(|_| "socket dev exceeds i64".to_string())?,
        i64::try_from(meta.ino()).map_err(|_| "socket ino exceeds i64".to_string())?,
    ))
}

/// The pure comparison every reader and writer applies between what the
/// registry published and what the host shows (the one `tmux_bootstrap` and
/// `operations::verify_published_incarnation` make; ADR 012 WS-A.9). A
/// witness the registry did not publish, or the probe did not observe, is
/// not compared; every witness both sides carry must agree. `Err` names the
/// witnesses that disagree.
pub fn compare_incarnation(
    published: &PublishedIncarnation,
    observed: &ObservedIncarnation,
) -> Result<(), Vec<&'static str>> {
    let mut disagree = Vec::new();
    match observed {
        ObservedIncarnation::ProcessDead { .. } => return Err(vec!["process"]),
        ObservedIncarnation::NoServer { .. } => return Err(vec!["server"]),
        ObservedIncarnation::Process {
            pid,
            start_token,
            socket_dev,
            socket_ino,
        } => {
            if published.pid.is_some_and(|recorded| recorded != *pid) {
                disagree.push("pid");
            }
            if let (Some(recorded), Some(live)) = (&published.start_token, start_token)
                && recorded != live
            {
                disagree.push("start_token");
            }
            if let (Some(recorded), Some(live)) = (published.socket_dev, *socket_dev)
                && recorded != live
            {
                disagree.push("socket_dev");
            }
            if let (Some(recorded), Some(live)) = (published.socket_ino, *socket_ino)
                && recorded != live
            {
                disagree.push("socket_ino");
            }
        }
    }
    if disagree.is_empty() {
        Ok(())
    } else {
        Err(disagree)
    }
}

/// The verdict of one liveness check of a published incarnation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// Every witness both sides carry agrees (instance state E as far as
    /// the host can tell; the pinned scan still verifies the epoch).
    Live(ObservedIncarnation),
    /// The host refutes the row: the process is dead, no server answers, or
    /// a witness disagrees (instance state F).
    Stale(ObservedIncarnation),
    /// The probe could not run, so nothing is proven either way.
    Unobservable(String),
}

/// Liveness of a published incarnation: `kill(pid, 0)` first (a dead pid is
/// conclusive without any adapter), then the probe's witnesses compared with
/// [`compare_incarnation`]. A row that publishes no pid has nothing to check
/// and is `Unobservable`.
pub fn liveness(
    backend: Backend,
    endpoint: &str,
    published: &PublishedIncarnation,
    probe: &dyn IncarnationProbe,
) -> Liveness {
    let Some(pid) = published.pid else {
        return Liveness::Unobservable("the registry publishes no pid".to_string());
    };
    let alive = i32::try_from(pid).is_ok_and(|pid| probe_pid(pid) == HolderLiveness::Alive);
    if !alive {
        return Liveness::Stale(ObservedIncarnation::ProcessDead { pid });
    }
    match probe.observe(backend, endpoint, published) {
        Err(detail) => Liveness::Unobservable(detail),
        Ok(observed) => match compare_incarnation(published, &observed) {
            Ok(()) => Liveness::Live(observed),
            Err(_) => Liveness::Stale(observed),
        },
    }
}

// ---------------------------------------------------------------------------
// The resolver

/// What the registry says about one backend's managed instance, resolved
/// before anything is probed. Promoted from `ls_cli`'s `ScanTarget`
/// (ADR 012 WS-A.4, review report 05): this is the one place where "a
/// managed instance's epoch is NULL" is first observable, so it is the one
/// place that decides. The enum carries no `Option<ServerEpoch>` — in the
/// `Unpublished` arm there is no epoch value in scope to hand to
/// [`InventoryScope::managed`], so a caller that wants to proceed anyway has
/// to write the other branch out loud, in a function that resolved a
/// registry instance three lines earlier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedTarget {
    /// A registered, addressable instance with a published epoch whose
    /// process the host still shows (ADR 012 WS-B.1): probe exactly that
    /// endpoint, pinned to exactly that epoch.
    Managed {
        instance: BackendInstanceUid,
        scope: InventoryScope,
    },
    /// Registered and addressable, but the server incarnation was never
    /// published (`server_epoch` is NULL). The row exists before the mux
    /// coordinates (`dmux-mux-start.sh` registers first and publishes later)
    /// and stays this way if coordination never completes. Nothing about the
    /// live server can be verified: reads are indeterminate, mutations
    /// refuse, and no durable row may be minted from what a scan would say.
    Unpublished(BackendInstanceUid),
    /// Registered, addressable and published — and the host refutes the
    /// publication: the published process is dead, no server answers on the
    /// endpoint, or the start token or socket dev/ino disagree with a fresh
    /// observation (plan §5.2 instance state F, review finding #1). There is
    /// no scope: the published epoch is a pin nothing live would answer to.
    /// Mutations refuse; `ls` renders the instance's Spaces `unreachable`
    /// with a `stale_incarnation` detail; the operator's clear is `dmux
    /// repair retire-incarnation` once the service is confirmed down.
    StaleIncarnation {
        instance: BackendInstanceUid,
        published: PublishedIncarnation,
        observed: ObservedIncarnation,
    },
    /// Registered without a recorded endpoint: the registry claims an
    /// instance this process cannot address.
    Unaddressable(BackendInstanceUid),
    /// No instance is registered for this backend. Whether to discover
    /// natives on a first-contact endpoint is the caller's decision, made
    /// with [`InventoryScope::unmanaged_endpoint`] and named in the audit
    /// allowlist.
    Unregistered,
}

impl ManagedTarget {
    /// The instance the registry knows about, whichever state it is in.
    pub fn instance(&self) -> Option<BackendInstanceUid> {
        match self {
            ManagedTarget::Managed { instance, .. }
            | ManagedTarget::Unpublished(instance)
            | ManagedTarget::StaleIncarnation { instance, .. }
            | ManagedTarget::Unaddressable(instance) => Some(*instance),
            ManagedTarget::Unregistered => None,
        }
    }

    /// The pinned scope, only for a managed instance.
    pub fn scope(&self) -> Option<&InventoryScope> {
        match self {
            ManagedTarget::Managed { scope, .. } => Some(scope),
            _ => None,
        }
    }

    /// The refusal every verb reports for an unpublished instance. One text,
    /// so the nine readers and writers that used to launder this case say the
    /// same thing; the code that goes with it is `BackendEpochChanged`, the
    /// mapping `ls` already made (an unpublished epoch is an epoch fault, not
    /// an unreachable endpoint).
    pub fn unpublished_detail(backend: Backend, instance: BackendInstanceUid) -> String {
        format!(
            "managed {backend} backend instance {} has published no server epoch, so nothing \
             about its live server can be verified",
            instance.0
        )
    }

    /// The refusal for an instance with no recorded endpoint.
    pub fn unaddressable_detail(backend: Backend, instance: BackendInstanceUid) -> String {
        format!(
            "managed {backend} backend instance {} has no recorded endpoint",
            instance.0
        )
    }

    /// Whether `detail` is one of this type's two epoch-fault texts — an
    /// unpublished or a stale incarnation — as a peer sends it over the
    /// `spaces` wire in a `ScanSummary` whose outcome tag is only
    /// `unreachable`. Recognised here, beside the texts, so the wire mapping
    /// (`ls_cli::peer_scan_error_code`) and the texts cannot drift apart.
    pub fn is_epoch_fault_detail(detail: &str) -> bool {
        detail.contains("has published no server epoch") || detail.contains("stale_incarnation")
    }

    /// The refusal every verb reports for a stale incarnation (state F), and
    /// the detail `ls` carries on the instance's `unreachable` rows. The
    /// code is `BackendEpochChanged`: the published epoch is the fault. The
    /// remedy follows plan §5.2 as amended — a managed restart republishes
    /// but is safe only while the service holds no user panes; otherwise the
    /// explicit clear once the service is confirmed down.
    pub fn stale_incarnation_detail(
        backend: Backend,
        instance: BackendInstanceUid,
        published: &PublishedIncarnation,
        observed: &ObservedIncarnation,
    ) -> String {
        format!(
            "managed {backend} backend instance {} publishes a stale incarnation \
             (stale_incarnation, instance state F): the registry records {published}, but \
             the host shows {observed}; nothing about the live server is verified by that \
             row. `dmux doctor` names the state; if the managed service is down, `dmux repair \
             retire-incarnation --backend {backend} --epoch {}` clears the row; a managed \
             restart republishes it but is safe only while the service holds no user panes",
            instance.0, published.epoch.0
        )
    }
}

/// Resolve one backend's managed instance from the registry. This is the
/// only sanctioned way to turn a registry instance into a scope; every
/// `InventoryScope::managed` built from registry rows goes through here or
/// through [`resolve_managed_instance`]. The liveness check runs with the
/// OS probe ([`OsIncarnationProbe`]); a caller holding an adapter passes a
/// server-asking probe through [`resolve_managed_with`].
pub fn resolve_managed(registry: &Registry, backend: Backend) -> RegistryResult<ManagedTarget> {
    resolve_managed_with(registry, backend, &OsIncarnationProbe)
}

/// [`resolve_managed`] with the liveness probe chosen by the caller.
pub fn resolve_managed_with(
    registry: &Registry,
    backend: Backend,
    probe: &dyn IncarnationProbe,
) -> RegistryResult<ManagedTarget> {
    let Some(instance) = registry.backend_instance_for_backend(backend)? else {
        return Ok(ManagedTarget::Unregistered);
    };
    resolve_managed_instance_with(registry, instance, probe)
}

/// The same resolution for a caller that already holds the instance — a
/// Space row's `backend_instance`, typically. Never `Unregistered`: the
/// instance is a foreign key the registry vouches for.
pub fn resolve_managed_instance(
    registry: &Registry,
    instance: BackendInstanceUid,
) -> RegistryResult<ManagedTarget> {
    resolve_managed_instance_with(registry, instance, &OsIncarnationProbe)
}

/// [`resolve_managed_instance`] with the liveness probe chosen by the caller.
pub fn resolve_managed_instance_with(
    registry: &Registry,
    instance: BackendInstanceUid,
    probe: &dyn IncarnationProbe,
) -> RegistryResult<ManagedTarget> {
    let info = registry.backend_instance_info(instance)?;
    let Some(endpoint) = info.socket_path else {
        return Ok(ManagedTarget::Unaddressable(instance));
    };
    let record = registry.backend_server(instance)?;
    let Some(published) = PublishedIncarnation::from_record(&record) else {
        return Ok(ManagedTarget::Unpublished(instance));
    };
    // A published epoch is a pin, not liveness (review finding #1): the row
    // of a crashed or replaced server is refuted here, before any caller
    // pins a scan to it or refuses on its word.
    if let Liveness::Stale(observed) = liveness(info.backend, &endpoint, &published, probe) {
        return Ok(ManagedTarget::StaleIncarnation {
            instance,
            published,
            observed,
        });
    }
    Ok(ManagedTarget::Managed {
        instance,
        scope: InventoryScope::managed(info.backend, endpoint, published.epoch),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryConfig;
    use uuid::Uuid;

    fn scratch() -> (tempfile::TempDir, Registry) {
        let dir = tempfile::tempdir().expect("scratch dir");
        let registry = Registry::open(RegistryConfig::new(
            dir.path().join("registry.sqlite3"),
            dir.path().join("locks"),
        ))
        .expect("scratch registry");
        (dir, registry)
    }

    /// A pid nothing holds: spawn and reap a child, then use its pid.
    fn dead_pid() -> i64 {
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = i64::from(child.id());
        let _ = child.wait_with_output();
        pid
    }

    fn own_pid() -> i64 {
        i64::from(std::process::id())
    }

    #[test]
    fn an_unregistered_backend_resolves_to_unregistered() {
        let (_dir, registry) = scratch();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        assert_eq!(target, ManagedTarget::Unregistered);
        assert_eq!(target.instance(), None);
        assert!(target.scope().is_none());
    }

    #[test]
    fn an_instance_without_an_endpoint_is_unaddressable() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Wez, None, None)
            .unwrap();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        assert_eq!(target, ManagedTarget::Unaddressable(instance));
        assert_eq!(target.instance(), Some(instance));
        assert!(target.scope().is_none());
    }

    #[test]
    fn a_registered_instance_with_no_published_epoch_is_unpublished_not_a_scope() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-scratch"), None)
            .unwrap();
        let target = resolve_managed(&registry, Backend::Tmux).unwrap();
        assert_eq!(target, ManagedTarget::Unpublished(instance));
        assert_eq!(target.instance(), Some(instance));
        // The whole point: there is no scope to hand to a provider here, and
        // no epoch value in reach to build one with.
        assert!(target.scope().is_none());
        let detail = ManagedTarget::unpublished_detail(Backend::Tmux, instance);
        assert!(detail.contains("has published no server epoch"), "{detail}");
        assert!(detail.contains(&instance.0.to_string()), "{detail}");
    }

    /// A row that publishes an epoch but no process witnesses (the shape
    /// the earliest publishers wrote) has nothing to compare: it pins.
    #[test]
    fn a_published_instance_resolves_to_a_scope_pinned_to_its_epoch() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/scratch.sock"), None)
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        registry
            .publish_backend_server(instance, epoch, None, None, None, None)
            .unwrap();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        let ManagedTarget::Managed {
            instance: resolved,
            scope,
        } = &target
        else {
            panic!("expected Managed, got {target:?}");
        };
        assert_eq!(*resolved, instance);
        assert_eq!(scope.backend, Backend::Wez);
        assert_eq!(scope.endpoint, "/tmp/scratch.sock");
        assert_eq!(scope.expected_epoch(), Some(epoch));
        assert_eq!(target.scope(), Some(scope));
    }

    #[test]
    fn resolving_by_instance_agrees_with_resolving_by_backend() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-scratch"), None)
            .unwrap();
        assert_eq!(
            resolve_managed_instance(&registry, instance).unwrap(),
            resolve_managed(&registry, Backend::Tmux).unwrap()
        );
        let epoch = ServerEpoch(Uuid::new_v4());
        registry
            .publish_backend_server(instance, epoch, None, None, None, None)
            .unwrap();
        assert_eq!(
            resolve_managed_instance(&registry, instance).unwrap(),
            resolve_managed(&registry, Backend::Tmux).unwrap()
        );
    }

    /// Review finding #1 inverted: a published epoch whose process has
    /// exited is state F, for both backends, before anything is probed on
    /// the endpoint — and it is not a scope.
    #[test]
    fn a_published_epoch_against_a_dead_pid_is_a_stale_incarnation_not_a_scope() {
        let (_dir, mut registry) = scratch();
        let wez = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/scratch.sock"), None)
            .unwrap();
        let tmux = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-scratch"), None)
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        let dead = dead_pid();
        for instance in [wez, tmux] {
            registry
                .publish_backend_server(
                    instance,
                    epoch,
                    Some(dead),
                    Some("gone"),
                    Some(16777231),
                    Some(10519741),
                )
                .unwrap();
        }
        for (backend, instance) in [(Backend::Wez, wez), (Backend::Tmux, tmux)] {
            let target = resolve_managed(&registry, backend).unwrap();
            let ManagedTarget::StaleIncarnation {
                instance: named,
                published,
                observed,
            } = &target
            else {
                panic!("{backend}: expected StaleIncarnation, got {target:?}");
            };
            assert_eq!(*named, instance);
            assert_eq!(published.epoch, epoch);
            assert_eq!(published.pid, Some(dead));
            assert_eq!(*observed, ObservedIncarnation::ProcessDead { pid: dead });
            assert_eq!(target.instance(), Some(instance));
            assert!(
                target.scope().is_none(),
                "{backend}: a stale row pins nothing"
            );
            let detail =
                ManagedTarget::stale_incarnation_detail(backend, instance, published, observed);
            assert!(detail.contains("stale_incarnation"), "{detail}");
            assert!(detail.contains(&epoch.0.to_string()), "{detail}");
            assert!(detail.contains("is dead"), "{detail}");
            assert!(detail.contains("retire-incarnation"), "{detail}");
            assert!(
                detail.contains("safe only while the service holds no user panes"),
                "{detail}"
            );
        }
    }

    /// The Wez check is complete from the OS alone: this process's own pid
    /// and start token with a real socket's dev/ino is live; the same row
    /// with another start token, or with the socket replaced, is stale.
    #[test]
    fn the_os_probe_compares_a_wez_row_on_pid_start_token_and_socket_identity() {
        use std::os::unix::fs::MetadataExt;
        let (dir, mut registry) = scratch();
        let socket = dir.path().join("wez.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let meta = std::fs::metadata(&socket).unwrap();
        let (dev, ino) = (meta.dev() as i64, meta.ino() as i64);
        let instance = registry
            .register_backend_instance(Backend::Wez, Some(socket.to_str().unwrap()), None)
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        let token = crate::runtime::process_start_token(std::process::id()).unwrap();

        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(own_pid()),
                Some(&token),
                Some(dev),
                Some(ino),
            )
            .unwrap();
        assert!(
            matches!(
                resolve_managed(&registry, Backend::Wez).unwrap(),
                ManagedTarget::Managed { .. }
            ),
            "every witness agrees"
        );

        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(own_pid()),
                Some("macos:1:1"),
                Some(dev),
                Some(ino),
            )
            .unwrap();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        let ManagedTarget::StaleIncarnation { observed, .. } = &target else {
            panic!("a start token the OS does not confirm is stale: {target:?}");
        };
        assert!(
            matches!(observed, ObservedIncarnation::Process { start_token: Some(live), .. } if *live == token),
            "{observed:?}"
        );

        // The socket replaced: a fresh bind gets a fresh inode.
        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(own_pid()),
                Some(&token),
                Some(dev),
                Some(ino),
            )
            .unwrap();
        drop(_listener);
        std::fs::remove_file(&socket).unwrap();
        let _replacement = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        assert!(
            matches!(
                target,
                ManagedTarget::StaleIncarnation {
                    observed: ObservedIncarnation::Process { .. },
                    ..
                }
            ),
            "a replaced socket is stale: {target:?}"
        );

        // Socket gone entirely: unobservable, so not proven stale — the
        // pinned scan downstream says `stopped`/`unreachable` for itself.
        drop(_replacement);
        std::fs::remove_file(&socket).unwrap();
        assert!(matches!(
            resolve_managed(&registry, Backend::Wez).unwrap(),
            ManagedTarget::Managed { .. }
        ));
    }

    /// Pre-A.9 rows carry no socket witnesses and are compared on what they
    /// have: a live pid with a matching start token pins; a live pid with
    /// no witnesses at all pins too (nothing contradicts it).
    #[test]
    fn a_row_without_socket_witnesses_is_compared_on_what_it_has() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/nonexistent/wez.sock"), None)
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        let token = crate::runtime::process_start_token(std::process::id()).unwrap();
        registry
            .publish_backend_server(instance, epoch, Some(own_pid()), Some(&token), None, None)
            .unwrap();
        // The socket path does not exist: the probe cannot stat it, which is
        // unobservable, not a refutation.
        assert!(matches!(
            resolve_managed(&registry, Backend::Wez).unwrap(),
            ManagedTarget::Managed { .. }
        ));
        registry
            .publish_backend_server(instance, epoch, Some(own_pid()), None, None, None)
            .unwrap();
        assert!(matches!(
            resolve_managed(&registry, Backend::Wez).unwrap(),
            ManagedTarget::Managed { .. }
        ));
        registry
            .publish_backend_server(instance, epoch, Some(dead_pid()), None, None, None)
            .unwrap();
        assert!(matches!(
            resolve_managed(&registry, Backend::Wez).unwrap(),
            ManagedTarget::StaleIncarnation { .. }
        ));
    }

    /// The injected probe decides for an alive pid: an explicit server
    /// answer with the same witnesses is live, a different one is stale, a
    /// probe that cannot run proves nothing.
    #[test]
    fn an_injected_probe_decides_liveness_for_an_alive_pid() {
        struct Canned(Result<ObservedIncarnation, String>);
        impl IncarnationProbe for Canned {
            fn observe(
                &self,
                _: Backend,
                _: &str,
                _: &PublishedIncarnation,
            ) -> Result<ObservedIncarnation, String> {
                self.0.clone()
            }
        }
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-scratch"), None)
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(own_pid()),
                Some("1700"),
                Some(7),
                Some(9),
            )
            .unwrap();
        let published = PublishedIncarnation {
            epoch,
            pid: Some(own_pid()),
            start_token: Some("1700".into()),
            socket_dev: Some(7),
            socket_ino: Some(9),
        };
        let same = ObservedIncarnation::Process {
            pid: own_pid(),
            start_token: Some("1700".into()),
            socket_dev: Some(7),
            socket_ino: Some(9),
        };
        assert!(matches!(
            resolve_managed_with(&registry, Backend::Tmux, &Canned(Ok(same.clone()))).unwrap(),
            ManagedTarget::Managed { .. }
        ));
        assert_eq!(
            liveness(
                Backend::Tmux,
                "dmux-scratch",
                &published,
                &Canned(Ok(same.clone()))
            ),
            Liveness::Live(same)
        );

        let other_server = ObservedIncarnation::Process {
            pid: own_pid() + 1,
            start_token: Some("1800".into()),
            socket_dev: Some(7),
            socket_ino: Some(10),
        };
        assert_eq!(
            compare_incarnation(&published, &other_server),
            Err(vec!["pid", "start_token", "socket_ino"])
        );
        assert!(matches!(
            resolve_managed_with(&registry, Backend::Tmux, &Canned(Ok(other_server))).unwrap(),
            ManagedTarget::StaleIncarnation { .. }
        ));

        let no_server = ObservedIncarnation::NoServer {
            pid: own_pid(),
            detail: "no server running".into(),
        };
        assert_eq!(
            compare_incarnation(&published, &no_server),
            Err(vec!["server"])
        );
        assert!(matches!(
            resolve_managed_with(&registry, Backend::Tmux, &Canned(Ok(no_server))).unwrap(),
            ManagedTarget::StaleIncarnation { .. }
        ));

        assert!(matches!(
            resolve_managed_with(
                &registry,
                Backend::Tmux,
                &Canned(Err("tmux: not found".into()))
            )
            .unwrap(),
            ManagedTarget::Managed { .. }
        ));
        assert_eq!(
            liveness(
                Backend::Tmux,
                "dmux-scratch",
                &published,
                &Canned(Err("tmux: not found".into()))
            ),
            Liveness::Unobservable("tmux: not found".into())
        );

        // Unobserved witnesses are not compared.
        let partial = ObservedIncarnation::Process {
            pid: own_pid(),
            start_token: None,
            socket_dev: None,
            socket_ino: None,
        };
        assert_eq!(compare_incarnation(&published, &partial), Ok(()));
    }

    #[test]
    fn the_tmux_socket_path_follows_tmux_tmpdir_and_the_uid() {
        let uid = unsafe { libc::getuid() };
        // This test process's own environment; the rule, not a fixed value.
        let expected_base = std::env::var_os("TMUX_TMPDIR")
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        assert_eq!(
            tmux_socket_path("dmux-scratch"),
            expected_base
                .join(format!("tmux-{uid}"))
                .join("dmux-scratch")
        );
    }
}
