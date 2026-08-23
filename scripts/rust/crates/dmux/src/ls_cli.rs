//! `dmux ls` with the Wez-first gate on (plan §7.1, §16.1).
//!
//! The binary's legacy `list` module still answers with the gate off, so this
//! is the only caller of `inventory::reconcile` and the `output` renderers.
//! Deprecated flags keep their exact legacy payload on stdout and put their
//! migration hint on stderr, because scripts compare stdout byte for byte.
//!
//! Owned by the P4 resolver/output agent (plan §19.3).

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::time::Duration;

use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;
use uuid::Uuid;

use crate::backend::scope::{
    self, IncarnationProbe, ManagedTarget, ObservedIncarnation, OsIncarnationProbe,
    PublishedIncarnation,
};
use crate::backend::tmux::TmuxProvider;
use crate::backend::wez::WezProvider;
use crate::backend::{InventoryOutcome, InventoryScope, Provider};
use crate::error::{ErrorCode, ExitStatus, TypedError};
use crate::inventory::{self, ManagedRow, ReconRow};
use crate::locks::{LockMode, LockScope, OrderedLocks};
use crate::model::{Backend, BackendInstanceUid, HostUid, Observation, SpaceNo, SpaceUid};
use crate::operations::{OperationEnv, SpaceHierarchy};
use crate::output::{self, OutputFormat, OwnerContext};
use crate::registry::{
    HolderLiveness, HostLifecycle, Lease, LeaseScope, Registry, RegistryConfig, SpaceRow, probe_pid,
};
use crate::remote::client::{
    AgentInvocation, PeerExpectation, SshInvoker, call_over_routes, request_envelope,
};
use crate::remote::protocol::{self, SpacesInfo};

/// True: the body below is real, so the binary routes `ls` here whenever the
/// canary flag is exported. It stayed false while the body was `todo!()` so a
/// machine with the flag already set kept the legacy listing instead of
/// panicking in the user's shell.
pub const IMPLEMENTED: bool = true;

/// The JSON `action` name for every shape this module emits (§16.2).
const ACTION: &str = "list";

/// Per-host bound for the `--all-hosts` fan-out. Each peer call is capped
/// here, so the whole listing finishes in one peer's worth of waiting rather
/// than the sum of them (§16.1: concurrent, bounded timeouts).
const HOST_DEADLINE: Duration = Duration::from_secs(20);

/// The tmux socket a bare `tmux` uses. Probed only when the registry holds
/// no tmux instance at all, so first contact on an unmigrated machine still
/// lists the sessions the user already has, as unmanaged rows (§16.1, case
/// 27) — legacy `ls` discovered them the same way. Wez has no counterpart:
/// ADR 006 forbids socket discovery, so an unregistered mux server has no
/// address this process may guess.
pub const DEFAULT_TMUX_NAMESPACE: &str = "default";

/// The four listing scopes, for the `ls` subcommand's clap `long_about`
/// (plan case 24: distinct *documented* scopes).
pub const SCOPES_HELP: &str = "\
List Spaces on one host, or on every enrolled host.

Scopes:
  dmux ls              Spaces on ONE host (--host, default this machine),
                       one line per Space, no children.
  dmux ls --tree       the same host set, each Space's live Groups and
                       Splits indented beneath it.
  dmux ls --all-hosts  every enrolled host, queried concurrently with
                       bounded timeouts; unavailable hosts are reported.
  dmux host ls         hosts and their routes only, never Spaces.

--all-hosts controls host breadth, --tree controls hierarchy depth, and the
two are independent. --all-hosts conflicts with --host.";

/// The parsed `ls` surface. The binary owns the flag spelling and the gate;
/// everything below the gate is decided here. The default is the plain
/// local listing bare `dmux` falls back to on a pipe.
#[derive(Default)]
pub struct LsArgs {
    /// `-H/--host`: alias, label, or HostUid; `None` is the local authority.
    pub host: Option<String>,
    /// Declared as conflicting with `host`, but clap enforces that only when
    /// both follow the subcommand: `dmux --host h ls --all-hosts` arrives
    /// here with both set and has to be refused here.
    pub all_hosts: bool,
    pub backend: Option<Backend>,
    pub tree: bool,
    /// Deprecated `--json`: the bare legacy row array, never the envelope.
    pub json: bool,
    /// Deprecated `--tmux` / `--wez`, both replaced by `--backend`.
    pub only_tmux: bool,
    pub only_wez: bool,
    /// Hidden legacy: one name per line, for the shell wrappers.
    pub names: bool,
}

// ---------------------------------------------------------------------------
// The source seam

/// One host the listing covers, with everything the renderers need to name
/// it. `route` is where the rows came from: `local` for this authority, the
/// winning transport for a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LsHost {
    pub host_uid: HostUid,
    pub alias: String,
    pub label: Option<String>,
    pub route: String,
    /// True for the authority this process runs on.
    pub local: bool,
}

impl LsHost {
    /// This row in the host table's shape for `resolve::resolve_enrolled_host`.
    /// An `LsHost` exists only for an enrolled row (see `Authority::hosts`),
    /// so the lifecycle is `Enrolled` by construction; the enrollment and
    /// tombstone timestamps play no part in resolution and are left empty.
    fn enrolled_row(&self) -> crate::registry::HostRow {
        crate::registry::HostRow {
            host_uid: self.host_uid,
            alias: Some(self.alias.clone()),
            label: self.label.clone(),
            lifecycle: HostLifecycle::Enrolled,
            enrolled_at: String::new(),
            tombstoned_at: None,
        }
    }

    pub fn owner(&self) -> OwnerContext {
        OwnerContext {
            host_uid: self.host_uid.0.to_string(),
            alias: self.alias.clone(),
            label: self.label.clone(),
            route: self.route.clone(),
        }
    }

    /// Alias plus label when they differ, for a diagnostic line.
    fn describe(&self) -> String {
        match &self.label {
            Some(label) => format!("{} ({label})", self.alias),
            None => self.alias.clone(),
        }
    }
}

/// One host's answer.
#[derive(Debug, Clone, Default)]
pub struct HostListing {
    pub rows: Vec<ReconRow>,
    /// False for a peer: `protocol::methods::SPACES` returns durable Space
    /// rows plus per-backend scan summaries and nothing per row, so the
    /// Group/Split columns are unknown rather than zero, and the peer's
    /// unmanaged natives are not reported at all.
    pub counts: bool,
    /// The transport the rows actually arrived over, when the source learns
    /// it only by answering.
    pub route: Option<String>,
    /// Operator-visible remarks that are not failures; they go to stderr so
    /// the JSON document keeps stdout to itself (§16.2).
    pub notes: Vec<String>,
    /// Backends whose scan established nothing. Every row on such a backend
    /// was downgraded to an unverified observation, which is a partial
    /// result: typed `errors[]` and exit 7, not a clean listing (§16.2).
    pub errors: Vec<ScanFailure>,
}

/// One backend's scan that established nothing, tagged with the backend it
/// was about so a `--backend` filter drops the half the caller did not ask
/// for rather than failing the listing over it.
#[derive(Debug, Clone)]
pub struct ScanFailure {
    pub backend: Backend,
    pub error: TypedError,
}

/// Where rows come from. Production is [`Authority`]; the fan-out, filters,
/// and every rendering decision sit above this line and are exercised
/// against test doubles.
pub trait LsSource: Sync {
    /// Enrolled hosts in enrollment order (§16.1 sorts by it).
    fn hosts(&self) -> Result<Vec<LsHost>, TypedError>;

    /// One host's rows. Called once per selected host, concurrently.
    fn listing(&self, host: &LsHost) -> Result<HostListing, TypedError>;

    /// `--tree` children of one managed row. Owner-local only: a peer's
    /// hierarchy is a per-Space round trip the frozen `SPACES` contract does
    /// not carry, and a missing hierarchy is not an empty one (§11.2).
    fn hierarchy(&self, host: &LsHost, row: &ManagedRow) -> Option<SpaceHierarchy>;

    /// The revision stamped on the JSON document (§16.2).
    fn authority_revision(&self) -> u64;
}

// ---------------------------------------------------------------------------
// Entry point

pub fn run(format: Option<OutputFormat>, args: LsArgs) -> ExitStatus {
    let source = match Authority::production() {
        Ok(source) => source,
        Err(error) => return emit(refuse(format, error)),
    };
    emit(render(&source, format, &args))
}

/// Everything one `ls` produced. Split from the printing so the whole
/// surface — envelope, table, tree, exit status — is assertable in-process.
pub struct LsOutput {
    pub stdout: String,
    pub stderr: Vec<String>,
    pub status: ExitStatus,
}

fn emit(output: LsOutput) -> ExitStatus {
    for line in &output.stderr {
        eprintln!("dmux: {line}");
    }
    print!("{}", output.stdout);
    output.status
}

pub fn render(source: &dyn LsSource, format: Option<OutputFormat>, args: &LsArgs) -> LsOutput {
    let mut stderr = deprecation_hints(args);
    let refuse = |error, stderr| refuse_with(source.authority_revision(), format, error, stderr);
    let backend = match backend_filter(args) {
        Ok(backend) => backend,
        Err(error) => return refuse(error, stderr),
    };
    if args.all_hosts && args.host.is_some() {
        return refuse(
            TypedError::new(
                ErrorCode::Usage,
                "--all-hosts lists every enrolled host and cannot be narrowed by --host",
            ),
            stderr,
        );
    }
    // clap refuses `--names --json`; the global spelling is the same
    // collision, and answering it with bare names would put a non-document
    // on stdout under `--format json` (§16.2).
    if args.names && format == Some(OutputFormat::Json) {
        return refuse(
            TypedError::new(
                ErrorCode::Usage,
                "--names cannot be combined with --format json; the document already carries \
                 each name in result[].name",
            ),
            stderr,
        );
    }
    let hosts = match source.hosts().and_then(|hosts| select(hosts, args)) {
        Ok(hosts) => hosts,
        Err(error) => return refuse(error, stderr),
    };

    // Independent per host, so one unreachable peer costs its own deadline
    // and not the listing's. `inventory::scan_both` is hard-wired to the two
    // backends of a single owner and is not this fan-out.
    let answers: Vec<Result<HostListing, TypedError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = hosts
            .iter()
            .map(|host| scope.spawn(move || source.listing(host)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    Err(TypedError::new(
                        ErrorCode::OperationFailed,
                        "the listing thread for this host panicked",
                    ))
                })
            })
            .collect()
    });

    let mut listings: Vec<(LsHost, HostListing)> = Vec::new();
    let mut errors: Vec<TypedError> = Vec::new();
    for (mut host, answer) in hosts.into_iter().zip(answers) {
        match answer {
            Ok(mut listing) => {
                if let Some(route) = listing.route.take() {
                    host.route = route;
                }
                if !listing.counts && !listing.rows.is_empty() {
                    stderr.push(format!(
                        "{}: the owner reports no per-Space Group/Split counts over `spaces`; \
                         those columns read '-'",
                        host.describe()
                    ));
                }
                stderr.extend(
                    listing
                        .notes
                        .iter()
                        .map(|note| format!("{}: {note}", host.describe())),
                );
                // A backend that established nothing is reported on both
                // channels: the operator sees the sentence, a JSON consumer
                // sees the typed error that makes this listing partial.
                for failure in listing.errors.drain(..) {
                    if backend.is_some_and(|selected| selected != failure.backend) {
                        continue;
                    }
                    let mut error = failure.error;
                    stderr.push(format!("{}: {}", host.describe(), error.message));
                    error
                        .target
                        .get_or_insert_with(|| host.host_uid.0.to_string());
                    errors.push(error);
                }
                if let Some(backend) = backend {
                    listing.rows.retain(|row| row_backend(row) == backend);
                }
                listings.push((host, listing));
            }
            Err(mut error) => {
                error.target = Some(host.host_uid.0.to_string());
                stderr.push(format!("{}: {}", host.describe(), error.message));
                errors.push(error);
            }
        }
    }
    if args.tree
        && listings
            .iter()
            .any(|(_, listing)| !listing.counts && !listing.rows.is_empty())
    {
        stderr.push("--tree expands owner-local Spaces only".to_string());
    }

    let status = output::document_exit(errors.is_empty(), !listings.is_empty(), &errors);
    let stdout = if args.names {
        names(&listings)
    } else if format == Some(OutputFormat::Json) {
        let result = Value::Array(rows_json(source, &listings, args.tree));
        format!(
            "{}\n",
            output::document(
                ACTION,
                errors.is_empty(),
                result,
                &errors,
                source.authority_revision(),
            )
        )
    } else if args.json {
        // Deprecated: the bare legacy array, never the envelope (ADR 011 D2).
        format!("{}\n", legacy_json(&listings))
    } else {
        human(source, &listings, args.tree)
    };
    LsOutput {
        stdout,
        stderr,
        status,
    }
}

// ---------------------------------------------------------------------------
// Argument rules

fn deprecation_hints(args: &LsArgs) -> Vec<String> {
    let mut hints = Vec::new();
    if args.json {
        hints.push("--json is deprecated; use --format json".to_string());
    }
    if args.only_wez {
        hints.push("--wez is deprecated; use --backend wez".to_string());
    }
    if args.only_tmux {
        hints.push("--tmux is deprecated; use --backend tmux".to_string());
    }
    hints
}

/// `--backend` and the two deprecated filters name the same thing, so a
/// contradiction is a usage error rather than a silent winner. Both legacy
/// flags together meant "no filter" and still do.
fn backend_filter(args: &LsArgs) -> Result<Option<Backend>, TypedError> {
    let deprecated = match (args.only_wez, args.only_tmux) {
        (true, false) => Some(Backend::Wez),
        (false, true) => Some(Backend::Tmux),
        _ => None,
    };
    match (args.backend, deprecated) {
        (Some(backend), Some(legacy)) if backend != legacy => Err(TypedError::new(
            ErrorCode::Usage,
            format!("--backend {backend} contradicts the deprecated --{legacy} filter"),
        )),
        (Some(backend), _) => Ok(Some(backend)),
        (None, legacy) => Ok(legacy),
    }
}

fn select(hosts: Vec<LsHost>, args: &LsArgs) -> Result<Vec<LsHost>, TypedError> {
    if args.all_hosts {
        return Ok(hosts);
    }
    let Some(spelling) = args.host.as_deref() else {
        return hosts
            .into_iter()
            .find(|host| host.local)
            .map(|host| vec![host])
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    "the registry names no local authority",
                )
            });
    };
    // The one enrolled-host rule (`resolve::resolve_enrolled_host`, ADR 012
    // WS-D.3) decides `--host`: `LsHost` rows are enrolled by construction
    // (`Authority::hosts` keeps only `Enrolled` rows), so they are adapted
    // to the resolver's row shape rather than matched by a third copy of
    // the alias/label/HostUid rule.
    let rows: Vec<crate::registry::HostRow> = hosts.iter().map(LsHost::enrolled_row).collect();
    let host_uid =
        crate::resolve::resolve_enrolled_host(&rows, &crate::remote::hosts::host_token(spelling))?;
    let matches: Vec<LsHost> = hosts
        .into_iter()
        .filter(|host| host.host_uid == host_uid)
        .collect();
    match matches.len() {
        1 => Ok(matches),
        0 => Err(TypedError::new(
            ErrorCode::NotFound,
            format!("no enrolled host matches {spelling:?}"),
        )),
        _ => Err(TypedError::new(
            ErrorCode::AmbiguousTarget,
            format!("{spelling:?} matches more than one enrolled host"),
        )),
    }
}

fn row_backend(row: &ReconRow) -> Backend {
    match row {
        ReconRow::Managed(managed) => managed.backend,
        ReconRow::Unmanaged(unmanaged) => unmanaged.backend,
    }
}

/// A refusal raised before a registry could be opened at all, so `0` is the
/// only revision this process can honestly claim (case 15 reads a lower
/// revision as staleness, which is exactly what this is).
fn refuse(format: Option<OutputFormat>, error: TypedError) -> LsOutput {
    refuse_with(0, format, error, Vec::new())
}

fn refuse_with(
    authority_revision: u64,
    format: Option<OutputFormat>,
    error: TypedError,
    mut stderr: Vec<String>,
) -> LsOutput {
    let status = error.code.exit_status();
    let stdout = match format {
        Some(OutputFormat::Json) => format!(
            "{}\n",
            output::document(
                ACTION,
                false,
                Value::Null,
                std::slice::from_ref(&error),
                authority_revision,
            )
        ),
        _ => String::new(),
    };
    stderr.push(error.message);
    LsOutput {
        stdout,
        stderr,
        status,
    }
}

// ---------------------------------------------------------------------------
// Rendering

/// One name per line for the shell wrappers — including for a name holding
/// a newline, which raw would read as two Spaces neither of which exists.
fn names(listings: &[(LsHost, HostListing)]) -> String {
    let mut out = String::new();
    for (_, listing) in listings {
        for row in &listing.rows {
            out.push_str(&output::one_line(row_name(row)));
            out.push('\n');
        }
    }
    out
}

fn row_name(row: &ReconRow) -> &str {
    match row {
        ReconRow::Managed(managed) => &managed.space.logical_name,
        ReconRow::Unmanaged(unmanaged) => &unmanaged.native_name,
    }
}

/// ADR 011 D2: the deprecated `--json` keeps emitting today's bare payload,
/// so the field names, their order, and their types are the legacy
/// `list::Row`'s — pinned by
/// `the_deprecated_json_payload_is_the_legacy_row_shape`. That module lives
/// in the binary, not this library, so the shape is mirrored rather than
/// reused.
///
/// Two values the Wez-first pipeline cannot produce: `created` (no provider
/// reports a native creation time — legacy already prints `null` for every
/// wez row) and `attached` (client attachment is deferred, ADR 011 D4).
#[derive(serde::Serialize)]
struct LegacyRow<'a> {
    index: usize,
    name: &'a str,
    kind: &'static str,
    host: &'a str,
    created: Option<i64>,
    windows: u32,
    attached: bool,
}

fn legacy_json(listings: &[(LsHost, HostListing)]) -> String {
    let mut rows: Vec<LegacyRow> = Vec::new();
    for (host, listing) in listings {
        let owner = host.label.as_deref().unwrap_or(&host.alias);
        for row in &listing.rows {
            let (backend, windows) = match row {
                ReconRow::Managed(managed) => (managed.backend, managed.groups),
                ReconRow::Unmanaged(unmanaged) => (unmanaged.backend, unmanaged.groups),
            };
            rows.push(LegacyRow {
                index: rows.len() + 1,
                name: row_name(row),
                kind: backend.as_str(),
                host: owner,
                created: None,
                windows,
                attached: false,
            });
        }
    }
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
}

fn rows_json(source: &dyn LsSource, listings: &[(LsHost, HostListing)], tree: bool) -> Vec<Value> {
    let mut out = Vec::new();
    for (host, listing) in listings {
        let owner = host.owner();
        for row in &listing.rows {
            let mut value = match row {
                ReconRow::Managed(managed) => output::managed_row_json(managed, &owner),
                ReconRow::Unmanaged(unmanaged) => output::unmanaged_row_json(unmanaged, &owner),
            };
            if !listing.counts {
                value["groups"] = Value::Null;
                value["splits"] = Value::Null;
            }
            if tree && let ReconRow::Managed(managed) = row {
                value["tree"] = match source.hierarchy(host, managed) {
                    Some(hierarchy) => {
                        serde_json::to_value(&hierarchy.groups).unwrap_or(Value::Null)
                    }
                    None => Value::Null,
                };
            }
            out.push(value);
        }
    }
    out
}

fn human(source: &dyn LsSource, listings: &[(LsHost, HostListing)], tree: bool) -> String {
    // One owner whose counts are its own: the shared renderers say it best,
    // and every other shape has to align columns across hosts they cannot see.
    if let [(host, listing)] = listings
        && listing.counts
    {
        let owner = host.owner();
        if !tree {
            return output::render_ls(&listing.rows, &owner);
        }
        let hierarchies: Vec<(SpaceUid, SpaceHierarchy)> = listing
            .rows
            .iter()
            .filter_map(|row| match row {
                ReconRow::Managed(managed) => source
                    .hierarchy(host, managed)
                    .map(|hierarchy| (managed.space.space_uid, hierarchy)),
                ReconRow::Unmanaged(_) => None,
            })
            .collect();
        return output::render_tree(&listing.rows, &owner, |managed| {
            hierarchies
                .iter()
                .find(|(uid, _)| *uid == managed.space.space_uid)
                .map(|(_, hierarchy)| hierarchy)
        });
    }
    table(source, listings, tree)
}

/// The §16.1 table across several owners at once, which the single-owner
/// renderers in `output` cannot align, and with the unknown counts a peer's
/// rows carry. `human_table_agrees_with_the_shared_renderer` pins the two
/// against each other.
fn table(source: &dyn LsSource, listings: &[(LsHost, HostListing)], tree: bool) -> String {
    let mut lines: Vec<(Option<[String; 10]>, String)> = Vec::new();
    for (host, listing) in listings {
        let owner = host.owner();
        let name = host.label.clone().unwrap_or_else(|| host.alias.clone());
        let count = |value: u32| {
            if listing.counts {
                value.to_string()
            } else {
                "-".to_string()
            }
        };
        for row in &listing.rows {
            let cells = match row {
                ReconRow::Managed(managed) => [
                    output::compact_ref(&owner.alias, managed.space.space_no.get()),
                    output::one_line(&managed.space.logical_name),
                    managed.backend.as_str().to_string(),
                    name.clone(),
                    count(managed.groups),
                    count(managed.splits),
                    server_column(managed.observation).to_string(),
                    "unknown".to_string(),
                    owner.route.clone(),
                    state_column(managed).to_string(),
                ],
                ReconRow::Unmanaged(unmanaged) => [
                    "-".to_string(),
                    output::one_line(&unmanaged.native_name),
                    unmanaged.backend.as_str().to_string(),
                    name.clone(),
                    count(unmanaged.groups),
                    count(unmanaged.splits),
                    "running".to_string(),
                    "unknown".to_string(),
                    owner.route.clone(),
                    if unmanaged.unepoched {
                        "unmanaged:unepoched".to_string()
                    } else {
                        "unmanaged".to_string()
                    },
                ],
            };
            lines.push((Some(cells), String::new()));
            if !tree {
                continue;
            }
            let ReconRow::Managed(managed) = row else {
                continue;
            };
            let Some(hierarchy) = source.hierarchy(host, managed) else {
                continue;
            };
            for group in &hierarchy.groups {
                lines.push((
                    None,
                    output::child_line(2, &group.group_ref, group.title.as_deref()),
                ));
                for split in &group.splits {
                    lines.push((
                        None,
                        output::child_line(4, &split.split_ref, split.title.as_deref()),
                    ));
                }
            }
        }
    }
    let mut widths: Vec<usize> = output::LS_HEADERS
        .iter()
        .map(|header| header.width())
        .collect();
    for (cells, _) in &lines {
        if let Some(cells) = cells {
            for (index, cell) in cells.iter().enumerate() {
                widths[index] = widths[index].max(cell.width());
            }
        }
    }
    let mut out = String::new();
    let emit = |out: &mut String, cells: &[String]| {
        let padded: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(index, cell)| output::pad_display(cell, widths[index]))
            .collect();
        out.push_str(padded.join("  ").trim_end());
        out.push('\n');
    };
    emit(&mut out, &output::LS_HEADERS.map(String::from));
    for (cells, child) in &lines {
        match cells {
            Some(cells) => emit(&mut out, cells),
            None => out.push_str(child),
        }
    }
    out
}

fn server_column(observation: Observation) -> &'static str {
    match observation {
        Observation::Live | Observation::Absent => "running",
        Observation::Stopped => "stopped",
        Observation::Unreachable | Observation::Unmanaged => "unreachable",
        Observation::Incompatible => "incompatible",
    }
}

fn state_column(row: &ManagedRow) -> &'static str {
    use crate::model::Health;
    match row.space.health {
        Health::MultiWindow => return "multi_window",
        Health::NativeKeyCollision => return "native_key_collision",
        Health::Unstamped => return "unstamped",
        Health::Unknown => return "unknown",
        Health::Healthy => {}
    }
    match row.observation {
        Observation::Live => "live",
        Observation::Absent => "absent",
        Observation::Stopped => "stopped",
        Observation::Unreachable => "unreachable",
        Observation::Incompatible => "incompatible",
        Observation::Unmanaged => "unmanaged",
    }
}

// ---------------------------------------------------------------------------
// The production source

/// This machine's registry for the local rows, the owner agent's `spaces`
/// method for every enrolled peer.
pub struct Authority {
    env: OperationEnv,
    wez_bin: String,
    wez_config: String,
    /// Probed only when no tmux instance is registered; see
    /// [`DEFAULT_TMUX_NAMESPACE`].
    tmux_namespace: String,
    invoker: SshInvoker,
}

/// What the registry says about one backend before anything is probed.
enum ScanTarget {
    /// A managed instance with a recorded endpoint and a published server
    /// epoch: probe exactly it, pinned to exactly that epoch. The scope is
    /// the resolver's (`backend::scope::resolve_managed`), so it is managed
    /// by construction — the case with no epoch is [`ScanTarget::Unpublished`].
    Managed(BackendInstanceUid, InventoryScope),
    /// A registered instance whose server incarnation was never published
    /// (`server_epoch` is NULL): addressable, but there is nothing to verify
    /// a scan against, so nothing is probed and the backend is indeterminate.
    /// The row exists before the mux coordinates (`dmux-mux-start.sh`
    /// registers first, publishes later) and stays this way if coordination
    /// never completes.
    Unpublished(BackendInstanceUid),
    /// A registered instance whose published incarnation the host refutes
    /// (plan §5.2 state F): nothing is probed, and every Space on it is
    /// `unreachable` with the `stale_incarnation` detail carried here.
    Stale(String),
    /// No instance registered, but the backend has a well-known endpoint
    /// natives can be discovered on. Not fenced — there is no managed
    /// instance to fence — and it can only ever yield unmanaged rows.
    Unregistered(InventoryScope),
    /// A registered instance with no recorded endpoint: the registry claims
    /// a managed instance this process cannot address.
    Unaddressable,
    /// Nothing registered and nothing discoverable, so nothing is probed.
    Nothing,
}

impl ScanTarget {
    /// The instance whose shared fence the listing consults: a probed
    /// target's, so the scan runs under it; an unpublished one's, so a
    /// coordinator mid-flight (the exclusive holder between registering and
    /// publishing — instance state D) is told from an idle instance nobody
    /// has bootstrapped (state C). `Unpublished` is still never probed
    /// (ADR 012 WS-B.2, review finding #19).
    fn instance(&self) -> Option<BackendInstanceUid> {
        match self {
            ScanTarget::Managed(instance, _) | ScanTarget::Unpublished(instance) => Some(*instance),
            _ => None,
        }
    }
}

impl Authority {
    pub fn production() -> Result<Authority, TypedError> {
        let env = OperationEnv::production()
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
        let (wez_bin, wez_config) = crate::runtime::production_wez_paths();
        Ok(Authority::with_wez(env, wez_bin, wez_config))
    }

    /// Explicit registry and `wezterm` paths, so the whole owner-local path —
    /// recorded endpoint, socket probe, sentinel handshake, reconciliation —
    /// runs against a scratch registry without the production mux service.
    pub fn with_wez(
        env: OperationEnv,
        wez_bin: impl Into<String>,
        wez_config: impl Into<String>,
    ) -> Authority {
        Authority {
            env,
            wez_bin: wez_bin.into(),
            wez_config: wez_config.into(),
            tmux_namespace: DEFAULT_TMUX_NAMESPACE.to_string(),
            invoker: SshInvoker::default(),
        }
    }

    /// The `-L` namespace first contact discovers native tmux sessions on.
    /// Tests point it at a scratch server so no suite run can reach the
    /// developer's own sessions.
    pub fn with_tmux_namespace(mut self, namespace: impl Into<String>) -> Authority {
        self.tmux_namespace = namespace.into();
        self
    }

    fn registry(&self) -> Result<Registry, TypedError> {
        Registry::open(RegistryConfig::new(&self.env.db_path, &self.env.lock_dir))
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))
    }

    /// What to probe for one backend, from what the registry recorded: the
    /// exact endpoint plus the published epoch. Deliberately not the
    /// verified descriptor path, which hard-errors when the service is down
    /// — a listing must be able to say `stopped` (§2.7, case 25).
    ///
    /// `discoverable` is the endpoint to fall back to when nothing is
    /// registered at all; only tmux has one.
    fn scan_target(
        registry: &Registry,
        backend: Backend,
        discoverable: Option<&str>,
    ) -> Result<ScanTarget, TypedError> {
        Ok(
            match scope::resolve_managed_with(registry, backend, &LiveIncarnationProbe)
                .map_err(typed_registry)?
            {
                ManagedTarget::Managed { instance, scope } => ScanTarget::Managed(instance, scope),
                ManagedTarget::Unpublished(instance) => ScanTarget::Unpublished(instance),
                ManagedTarget::StaleIncarnation {
                    instance,
                    published,
                    observed,
                } => ScanTarget::Stale(ManagedTarget::stale_incarnation_detail(
                    backend, instance, &published, &observed,
                )),
                ManagedTarget::Unaddressable(_) => ScanTarget::Unaddressable,
                ManagedTarget::Unregistered => match discoverable {
                    // audit(unmanaged_endpoint): first-contact tmux namespace; nothing is registered for this backend
                    Some(endpoint) => ScanTarget::Unregistered(InventoryScope::unmanaged_endpoint(
                        backend,
                        endpoint.to_string(),
                    )),
                    None => ScanTarget::Nothing,
                },
            },
        )
    }

    fn local_listing(&self) -> Result<HostListing, TypedError> {
        let registry = self.registry()?;
        let spaces = registry.spaces().map_err(typed_registry)?;
        let mut records = Vec::with_capacity(spaces.len());
        let mut backends: HashMap<BackendInstanceUid, Backend> = HashMap::new();
        for space in spaces {
            if let std::collections::hash_map::Entry::Vacant(slot) =
                backends.entry(space.backend_instance)
            {
                slot.insert(
                    registry
                        .backend_instance_info(space.backend_instance)
                        .map_err(typed_registry)?
                        .backend,
                );
            }
            let binding = registry
                .current_binding(space.space_uid)
                .map_err(typed_registry)?;
            records.push((space, binding));
        }

        let wez = Authority::scan_target(&registry, Backend::Wez, None)?;
        let tmux = Authority::scan_target(&registry, Backend::Tmux, Some(&self.tmux_namespace))?;
        // A recovering or mutating instance is not scanned at all: a
        // half-restored tree read as a complete inventory would demote live
        // Spaces to `absent` and publish their natives as unmanaged.
        let mut locks = OrderedLocks::new(&self.env.lock_dir);
        locks
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .map_err(|error| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("authority scan lock: {error}"),
                )
            })?;
        let mut fenced = Vec::new();
        // Same-rank scopes are taken in increasing key order (§10.1).
        let mut ordered = [wez.instance(), tmux.instance()];
        ordered.sort_by_key(|instance| instance.map(|i| LockScope::BackendInstance(i).key()));
        for instance in ordered.into_iter().flatten() {
            if locks
                .try_acquire(LockScope::BackendInstance(instance), LockMode::Shared)
                .map_err(|error| {
                    TypedError::new(
                        ErrorCode::OperationFailed,
                        format!("backend scan lock: {error}"),
                    )
                })?
            {
                fenced.push(instance);
            }
        }

        // An unpublished instance is classified now, while the fences are
        // held and the registry is open: the two witnesses that tell a
        // first bootstrap or recovery in flight (state D) from an idle,
        // never-bootstrapped instance (state C) are the shared fence just
        // tried and the instance's recovery lease.
        let mut unpublished: HashMap<BackendInstanceUid, String> = HashMap::new();
        for (backend, target) in [(Backend::Wez, &wez), (Backend::Tmux, &tmux)] {
            if let ScanTarget::Unpublished(instance) = target {
                let lease = registry
                    .current_lease(&LeaseScope::Recovery(*instance))
                    .map_err(typed_registry)?;
                unpublished.insert(
                    *instance,
                    unpublished_state_detail(
                        backend,
                        *instance,
                        !fenced.contains(instance),
                        lease.as_ref(),
                    ),
                );
            }
        }
        let scan = |target: &ScanTarget, probe: &dyn Fn(&InventoryScope) -> InventoryOutcome| {
            match target {
                ScanTarget::Managed(instance, scope) if fenced.contains(instance) => probe(scope),
                ScanTarget::Managed(..) => InventoryOutcome::Unreachable {
                    detail: "backend instance is recovering or mutating".into(),
                },
                // A managed instance with no published epoch is refused, not
                // scanned: an unpinned scan of a managed endpoint would
                // accept whatever server answers, and a `Complete` answer
                // from the wrong one demotes every live Space to `absent`.
                ScanTarget::Unpublished(instance) => InventoryOutcome::Unreachable {
                    detail: unpublished[instance].clone(),
                },
                // Published and refuted by the host (state F): never
                // `stopped`, never probed — nothing verified has answered
                // (§8.1), so the rows are `unreachable: stale_incarnation`.
                ScanTarget::Stale(detail) => InventoryOutcome::Unreachable {
                    detail: detail.clone(),
                },
                ScanTarget::Unregistered(scope) => probe(scope),
                ScanTarget::Unaddressable => InventoryOutcome::Unreachable {
                    detail: "the registered backend instance has no recorded endpoint".into(),
                },
                // Nothing is probed, so nothing is established: reconcile
                // must still not read this as an empty backend (§2.10).
                ScanTarget::Nothing => InventoryOutcome::Unreachable {
                    detail: "no backend instance is registered".into(),
                },
            }
        };
        let scans = inventory::scan_both(
            || {
                scan(&wez, &|scope| {
                    WezProvider::new(&self.wez_bin, &self.wez_config).inventory(scope)
                })
            },
            || {
                scan(&tmux, &|scope| {
                    TmuxProvider::new(scope.endpoint.clone()).inventory(scope)
                })
            },
        );
        locks.release_all();

        let mut errors = Vec::new();
        for (backend, target) in [(Backend::Wez, &wez), (Backend::Tmux, &tmux)] {
            // Nothing was probed, so there is nothing to report: a machine
            // that runs no mux is a normal state, not a failure (`list.rs`).
            if matches!(target, ScanTarget::Nothing) {
                continue;
            }
            if let Some(detail) = indeterminate_detail(scans.get(backend)) {
                errors.push(ScanFailure {
                    backend,
                    error: TypedError::new(
                        target_error_code(target, scans.get(backend)),
                        format!("{backend} inventory is indeterminate: {detail}"),
                    ),
                });
            }
        }
        Ok(HostListing {
            rows: inventory::reconcile(
                &records,
                |space| backends.get(&space.backend_instance).copied(),
                &scans,
            ),
            counts: true,
            route: None,
            notes: Vec::new(),
            errors,
        })
    }

    fn remote_listing(&self, host: &LsHost) -> Result<HostListing, TypedError> {
        let mut registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let head = registry.authority_head().map_err(typed_registry)?;
        let request = request_envelope(
            &identity,
            &head,
            protocol::methods::SPACES,
            Uuid::new_v4(),
            json!({}),
        );
        let outcome = call_over_routes(
            &mut registry,
            &PeerExpectation {
                host_uid: host.host_uid,
                need_capability: None,
                claimed_current: false,
            },
            &request,
            &self.invoker,
            &AgentInvocation::new(protocol::methods::SPACES),
            HOST_DEADLINE,
        )?;
        let payload = outcome.envelope.payload.ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                "owner spaces response omitted its payload",
            )
        })?;
        let info: SpacesInfo = serde_json::from_value(payload).map_err(|error| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                format!("owner spaces payload: {error}"),
            )
        })?;
        let route = registry
            .routes_for(host.host_uid)
            .map_err(typed_registry)?
            .into_iter()
            .find(|row| row.route_id == outcome.route_id)
            .map(|row| row.transport.as_str().to_string());
        Ok(peer_listing(host.host_uid, info, route))
    }
}

impl LsSource for Authority {
    fn hosts(&self) -> Result<Vec<LsHost>, TypedError> {
        let registry = self.registry()?;
        let local = registry.identity().map_err(typed_registry)?.host_uid;
        Ok(registry
            .hosts()
            .map_err(typed_registry)?
            .into_iter()
            .filter(|row| row.lifecycle == HostLifecycle::Enrolled)
            .filter_map(|row| {
                let is_local = row.host_uid == local;
                Some(LsHost {
                    host_uid: row.host_uid,
                    alias: row.alias?,
                    label: row.label,
                    // A peer's real transport is whichever route answered;
                    // the walk reports it back and this placeholder is
                    // replaced before any of its rows are rendered.
                    route: if is_local {
                        "local".into()
                    } else {
                        "ssh".into()
                    },
                    local: is_local,
                })
            })
            .collect())
    }

    fn listing(&self, host: &LsHost) -> Result<HostListing, TypedError> {
        if host.local {
            self.local_listing()
        } else {
            self.remote_listing(host)
        }
    }

    fn hierarchy(&self, host: &LsHost, row: &ManagedRow) -> Option<SpaceHierarchy> {
        if !host.local || row.observation != Observation::Live {
            return None;
        }
        let registry = self.registry().ok()?;
        let ScanTarget::Managed(_, managed) =
            Authority::scan_target(&registry, row.backend, None).ok()?
        else {
            return None;
        };
        let scope = managed;
        let provider: Box<dyn Provider> = match row.backend {
            Backend::Wez => Box::new(WezProvider::new(&self.wez_bin, &self.wez_config)),
            Backend::Tmux => Box::new(TmuxProvider::new(scope.endpoint.clone())),
        };
        drop(registry);
        crate::operations::hierarchy(&self.env, provider.as_ref(), &scope, row.space.space_uid).ok()
    }

    fn authority_revision(&self) -> u64 {
        self.registry()
            .and_then(|registry| registry.authority_head().map_err(typed_registry))
            .map_or(0, |head| head.revision)
    }
}

/// A peer's durable Spaces as listing rows. The owner reports one scan
/// summary per backend and no per-row join, so liveness is claimed only for
/// an active, currently bound Space on a backend whose scan completed, and
/// nothing here invents an unmanaged row the owner never sent.
pub fn peer_listing(owner: HostUid, info: SpacesInfo, route: Option<String>) -> HostListing {
    let mut notes: Vec<String> = Vec::new();
    let mut errors: Vec<ScanFailure> = Vec::new();
    let mut outcomes: HashMap<Backend, &str> = HashMap::new();
    for scan in &info.scans {
        outcomes.insert(scan.backend, scan.outcome.as_str());
        if scan.outcome != "complete" {
            let remark = match &scan.detail {
                Some(detail) => format!("{} scan {}: {detail}", scan.backend, scan.outcome),
                None => format!("{} scan {}", scan.backend, scan.outcome),
            };
            // A stopped server is a determinate answer; anything else left
            // the peer's rows unverified, which makes the listing partial.
            if scan.outcome == "server_stopped" {
                notes.push(remark);
            } else {
                errors.push(ScanFailure {
                    backend: scan.backend,
                    error: TypedError::new(
                        peer_scan_error_code(&scan.outcome, scan.detail.as_deref()),
                        remark,
                    ),
                });
            }
        }
    }
    let rows = info
        .spaces
        .into_iter()
        .filter(|space| !space.lifecycle.is_terminal())
        .filter_map(|space| {
            let space_no = NonZeroU64::new(space.space_no)?;
            let outcome = outcomes
                .get(&space.backend)
                .copied()
                .unwrap_or("unavailable");
            let observation = match outcome {
                "complete"
                    if space.lifecycle == crate::model::Lifecycle::Active
                        && space.native_token.is_some() =>
                {
                    Observation::Live
                }
                "complete" => Observation::Absent,
                "server_stopped" => Observation::Stopped,
                "version_mismatch" | "protocol_mismatch" => Observation::Incompatible,
                _ => Observation::Unreachable,
            };
            Some(ReconRow::Managed(ManagedRow {
                space: SpaceRow {
                    space_uid: space.space_uid,
                    owner,
                    space_no: SpaceNo(space_no),
                    backend_instance: space.backend_instance_uid,
                    logical_name: space.name,
                    lifecycle: space.lifecycle,
                    health: space.health,
                    created_at: String::new(),
                    updated_at: String::new(),
                    deleted_at: None,
                },
                backend: space.backend,
                observation,
                groups: 0,
                splits: 0,
                server_epoch: None,
                native_token: space.native_token,
                multi_window: false,
            }))
        })
        .collect();
    HostListing {
        rows,
        counts: false,
        route,
        notes,
        errors,
    }
}

/// The detail a refused unpublished-epoch scan reports, with the advice
/// that is safe for the state the instance is actually in (plan §5.2;
/// review report 04 rows C/D). `exclusive_held` is the non-blocking shared
/// fence having been refused — the instance's exclusive holder is the
/// coordinator between registering and publishing (`tmux_bootstrap`, the
/// Wez recovery coordinator); `recovery_lease` is the instance's held
/// `recovery:<uid>` lease, which a killed coordinator leaves behind for the
/// next one to take over. Either witness is state D: something is in
/// flight, and the only safe advice is to wait. Neither is state C: nothing
/// has ever published, and nothing `ls` does can substitute for the
/// coordination that would. Nothing here says "restart": a restart during
/// a first bootstrap destroys the coordinator it is waiting for.
pub fn unpublished_state_detail(
    backend: Backend,
    instance: BackendInstanceUid,
    exclusive_held: bool,
    recovery_lease: Option<&Lease>,
) -> String {
    let base = ManagedTarget::unpublished_detail(backend, instance);
    let mut witnesses = Vec::new();
    if exclusive_held {
        witnesses.push("the backend-instance lock is held exclusively".to_string());
    }
    if let Some(lease) = recovery_lease {
        witnesses.push(match lease.holder_pid {
            Some(pid) => format!(
                "a recovery lease is held by pid {pid} ({})",
                match probe_pid(pid) {
                    HolderLiveness::Alive => "alive",
                    HolderLiveness::Dead => "exited; the next coordinator takes it over",
                }
            ),
            None => "a recovery lease is held".to_string(),
        });
    }
    if witnesses.is_empty() {
        format!(
            "{base}; the instance has never published an incarnation (instance state C): wait \
             for the managed mux to coordinate (or run the tmux bootstrap, `dmux _tmux-bootstrap \
             --namespace <ns>`, against a running managed server); if the service is up and still \
             unpublished, `dmux doctor` names the state"
        )
    } else {
        format!(
            "{base}; a bootstrap or recovery is in flight (instance state D: {}); wait and re-run \
             `dmux ls`",
            witnesses.join(", ")
        )
    }
}

/// The code for a failed scan. Normally the outcome decides, but a target
/// that was refused before any probe knows better than the generic
/// `Unreachable` it had to report: an unpublished epoch is an epoch fault,
/// not an unreachable endpoint, and the same distinction the sibling readers
/// make (`gui_lifecycle.rs`, `gui_cli.rs`) has to survive into `errors[]`.
fn target_error_code(target: &ScanTarget, outcome: &InventoryOutcome) -> ErrorCode {
    match target {
        ScanTarget::Unpublished(_) | ScanTarget::Stale(_) => ErrorCode::BackendEpochChanged,
        _ => scan_error_code(outcome),
    }
}

/// The liveness probe the listing resolves under (ADR 012 WS-B.1). For Wez
/// the OS probe is the whole check. For tmux the server is asked — the
/// adapter's `server_incarnation` probe, the one `tmux_bootstrap` published
/// from — so the start token is compared too, and a server that answers with
/// another pid, a fresh socket inode, or "no server" on a namespace whose
/// published pid is alive (pid reuse) is stale before it is listed. The
/// resolver itself runs no adapter code; this is the caller-supplied probe
/// its contract names.
pub struct LiveIncarnationProbe;

impl IncarnationProbe for LiveIncarnationProbe {
    fn observe(
        &self,
        backend: Backend,
        endpoint: &str,
        published: &PublishedIncarnation,
    ) -> Result<ObservedIncarnation, String> {
        match backend {
            Backend::Wez => OsIncarnationProbe.observe(backend, endpoint, published),
            Backend::Tmux => {
                let pid = published
                    .pid
                    .ok_or_else(|| "the registry publishes no pid to observe".to_string())?;
                match TmuxProvider::new(endpoint).server_incarnation(endpoint) {
                    Ok(live) => Ok(ObservedIncarnation::Process {
                        pid: i64::from(live.identity.pid),
                        start_token: Some(live.identity.start_token),
                        socket_dev: i64::try_from(live.socket_dev).ok(),
                        socket_ino: i64::try_from(live.socket_ino).ok(),
                    }),
                    Err(crate::backend::ProviderError::NativeFailure { detail })
                        if detail.starts_with("no tmux server for this namespace") =>
                    {
                        Ok(ObservedIncarnation::NoServer { pid, detail })
                    }
                    Err(error) => Err(format!("tmux incarnation probe: {error:?}")),
                }
            }
        }
    }
}

/// The typed code for an indeterminate owner-local scan. Same mapping as
/// `gui_lifecycle::inventory_error`, plus the epoch-mismatch case a listing
/// can reach and a readiness probe cannot.
fn scan_error_code(outcome: &InventoryOutcome) -> ErrorCode {
    match outcome {
        _ if inventory::epoch_changed_detail(outcome).is_some() => ErrorCode::BackendEpochChanged,
        InventoryOutcome::AuthFailed { .. } => ErrorCode::AuthFailed,
        InventoryOutcome::HostKeyIdentityFailed { .. } => ErrorCode::HostIdentityChanged,
        InventoryOutcome::VersionMismatch { .. } => ErrorCode::VersionMismatch,
        InventoryOutcome::ProtocolMismatch { .. } => ErrorCode::ProtocolMismatch,
        InventoryOutcome::Malformed { .. } => ErrorCode::PostconditionFailed,
        _ => ErrorCode::ProviderUnavailable,
    }
}

/// The same mapping over the wire, where the owner sends the outcome tag
/// and its detail. The epoch fault travels in the detail, exactly as the
/// adapters report it locally (`inventory::epoch_changed_detail`'s prefix
/// on a `malformed` outcome) and as the owner's resolver reports an
/// unpublished or stale instance (`unreachable` with the shared
/// `ManagedTarget` text), so a peer's epoch fault is `BackendEpochChanged`
/// like the local path's, never the generic `provider_unavailable`
/// (ADR 012 WS-A.5 remote-agent handoff). An outcome this build does not
/// know is still a failed scan.
fn peer_scan_error_code(outcome: &str, detail: Option<&str>) -> ErrorCode {
    if detail.is_some_and(|detail| {
        detail.starts_with(inventory::EPOCH_CHANGED_PREFIX)
            || ManagedTarget::is_epoch_fault_detail(detail)
    }) {
        return ErrorCode::BackendEpochChanged;
    }
    match outcome {
        "auth_failed" => ErrorCode::AuthFailed,
        "host_key_identity_failed" => ErrorCode::HostIdentityChanged,
        "version_mismatch" => ErrorCode::VersionMismatch,
        "protocol_mismatch" => ErrorCode::ProtocolMismatch,
        "malformed" => ErrorCode::PostconditionFailed,
        _ => ErrorCode::ProviderUnavailable,
    }
}

/// The reason a scan established nothing, for the operator. A determinate
/// scan has none.
fn indeterminate_detail(outcome: &InventoryOutcome) -> Option<&str> {
    match outcome {
        InventoryOutcome::Complete(_) | InventoryOutcome::ServerStopped { .. } => None,
        InventoryOutcome::Unreachable { detail }
        | InventoryOutcome::AuthFailed { detail }
        | InventoryOutcome::HostKeyIdentityFailed { detail }
        | InventoryOutcome::CommandMissing { detail }
        | InventoryOutcome::VersionMismatch { detail }
        | InventoryOutcome::ProtocolMismatch { detail }
        | InventoryOutcome::Malformed { detail }
        | InventoryOutcome::Timeout { detail }
        | InventoryOutcome::PermissionFailure { detail } => Some(detail),
    }
}

fn typed_registry(error: crate::registry::RegistryError) -> TypedError {
    TypedError::new(error.error_code(), error.to_string())
}
