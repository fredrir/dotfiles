//! `dmux migrate` — the one-time explicit cutover (plan §17).
//!
//! Preview is the default: without `--commit` it prints the proposed Space
//! mapping and adopts nothing. Legacy toggle history arrives as the parsed
//! state file rather than being read here, because that file belongs to the
//! binary and converts to SpaceUid only where a name is unambiguous
//! (plan §17.11).
//!
//! What this driver owns, of §17's thirteen steps: 1 (back up and print the
//! location), 6 (complete owner scans on each backend), 7 (the deterministic
//! proposed mapping), and 11 (history conversion). Steps 2/3/4 are
//! enrollment's, 8/9 are `operations::adopt_*` and `dmux repair normalize`,
//! 12 is the service install, and 13 is `--row`. Steps 5 and 10 are
//! *verified* here rather than re-implemented, and the verification is
//! reported in the plan:
//!
//! - §17.5, both remote Wez routes at once: already impossible.
//!   `remote::client::call_over_routes` walks `remote::routes::eligible` in
//!   priority order and returns on the first route that answers, so one call
//!   attaches one route; each route additionally carries its own
//!   `routes::wez_domain_name` identity, so two routes to one host are two
//!   distinct domains rather than one shared attachment. The plan prints the
//!   enabled Wez route count per peer as a `single_remote_wez_route` check.
//! - §17.10, duplicate cross-backend names: `operations` already refuses
//!   them (`require_no_cross_backend_name`), and adoption allocates a fresh
//!   SpaceUid/SpaceNo per resource, so duplicates can never share identity.
//!   The plan flags the pair with `duplicate_name` and the commit surfaces
//!   the operation's own `name_conflict` refusal, whose remedy is `--name`.
//!
//! "Migrate once" (case 45) is enforced twice over, because a stamp file is
//! only as durable as the person holding `rm`:
//!
//! 1. The recorded cutover ([`STAMP_FILE`]) makes a second `--commit` a
//!    no-op that adopts nothing. Resources that appear later are adopted
//!    with `dmux adopt`, the ordinary entry point (§10.3).
//! 2. Even with the stamp deleted, an already-adopted resource carries a
//!    current binding, so `inventory::reconcile` reports it managed and it
//!    is never a candidate again; and if it somehow were, `adopt_*` refuses
//!    a bound native token and a foreign/own marker set under the lease.
//!
//! Owned by the P11 migration agent (plan §19.3).

use std::collections::{BTreeMap, HashMap};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::adopt_cli::WezCli;
use crate::backend::scope::{self, ManagedTarget};
use crate::backend::tmux::TmuxProvider;
use crate::backend::wez::{SystemRunner, WezProvider, WezRunner};
use crate::backend::{InventoryOutcome, InventoryScope, Provider};
use crate::error::{ErrorCode, ExitStatus, TypedError};
use crate::history::{ConvertDropReason, History, LegacyEntry, convert_legacy_entries};
use crate::inventory::{self, ReconRow};
use crate::locks::{LockMode, LockScope, OrderedLocks};
use crate::model::{Backend, BackendInstanceUid, Health, HostUid, SpaceUid};
use crate::operations::{OpError, OperationEnv, adopt_tmux, adopt_wez};
use crate::output::{self, OutputFormat};
use crate::registry::{HostLifecycle, Registry, RegistryConfig};

/// The body below is real, so the binary's Wez-first arm dispatches here.
pub const IMPLEMENTED: bool = true;

/// The JSON `action` for every shape this module emits (§16.2).
const ACTION: &str = "migrate";

/// The §17.1 pre-migration backup, beside the registry it copies. The name
/// is fixed rather than timestamped for two reasons: the preview has to
/// print the exact path the commit will write, and a retried commit must not
/// overwrite the pre-migration copy with post-partial state — so an existing
/// file is reported and kept, never replaced.
pub const BACKUP_FILE: &str = "registry.pre-migrate.sqlite3";

/// The durable record that the cutover happened, beside the registry it
/// describes. Its presence turns every later `migrate` into a no-op.
pub const STAMP_FILE: &str = "migrated-v1.json";
const STAMP_VERSION: u64 = 1;

/// Stands in for the SpaceUid a preview cannot know: the identity of an
/// adopt candidate is allocated at commit. Only the keep/drop decision of
/// [`convert_legacy_entries`] is read back in that phase, never the UID.
const UNALLOCATED: SpaceUid = SpaceUid(Uuid::nil());

pub struct MigrateArgs {
    /// Apply the previewed plan; without it nothing is adopted or stamped.
    pub commit: bool,
    pub yes: bool,
    /// `key session` lines from the legacy `dmux -` state file.
    pub previous_sessions: BTreeMap<String, String>,
}

/// One rendered run. Returned rather than printed so a test can pin the
/// whole surface — envelope, plan, exit status — without capturing the
/// process's own stdout.
pub struct MigrateOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// §7.4's confirmation seam. `--yes` and `--format json` are answered before
/// this is consulted, so the only question left is the interactive one.
pub trait Consent {
    /// Show `preview` and ask. `None` means there is no terminal to ask on,
    /// which §7.4 treats exactly like a decline: change nothing, exit 5.
    fn confirm(&self, preview: &str) -> Option<bool>;
}

/// Production consent: prompt on stderr, so a `--format human` stdout stays
/// a plan the operator can pipe.
pub struct TerminalConsent;

impl Consent for TerminalConsent {
    fn confirm(&self, preview: &str) -> Option<bool> {
        if !std::io::stdin().is_terminal() {
            return None;
        }
        eprint!("{preview}\nApply this migration? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            return Some(false);
        }
        Some(answer.trim().eq_ignore_ascii_case("y"))
    }
}

/// Everything the driver touches, injected so the whole verb runs against a
/// scratch registry, a scratch `tmux -L` namespace, and a fake mux.
pub struct MigrateEnv<'a, R: WezRunner> {
    pub ops: &'a OperationEnv,
    pub wez: WezCli<R>,
    /// Where converted `dmux -` history lands (§17.11).
    pub history: History,
    pub consent: &'a dyn Consent,
}

/// [`MigrateEnv`] with the wezterm CLI already resolved into one provider.
struct Bound<'a, R: WezRunner> {
    ops: &'a OperationEnv,
    wez: WezProvider<R>,
    history: History,
    consent: &'a dyn Consent,
}

pub fn run(format: Option<OutputFormat>, args: MigrateArgs) -> ExitStatus {
    let ops = match OperationEnv::production() {
        Ok(env) => env,
        Err(e) => return emit(refuse(format, 0, operation_failed(e.to_string()))),
    };
    let Some(state_dir) = History::default_dir() else {
        return emit(refuse(
            format,
            0,
            operation_failed("no state directory resolvable for `dmux -` history".to_string()),
        ));
    };
    let (bin, config) = crate::runtime::production_wez_paths();
    let env = MigrateEnv {
        ops: &ops,
        wez: WezCli {
            bin,
            config,
            runner: SystemRunner,
        },
        history: History::new(state_dir),
        consent: &TerminalConsent,
    };
    emit(migrate_in(env, format, args))
}

fn emit(output: MigrateOutput) -> ExitStatus {
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    output.status
}

// ---------------------------------------------------------------------------
// The proposed mapping (§17.7)

/// What the cutover will do with one resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Batch-adopt through the normal adoption lease (§17.8).
    Adopt,
    /// Left unmanaged on purpose (§17.9); the reason is a stable token.
    Quarantine(&'static str),
    /// Already carries a current binding: migrated, and never again.
    AlreadyManaged,
}

impl Disposition {
    fn token(&self) -> &'static str {
        match self {
            Disposition::Adopt => "adopt",
            Disposition::Quarantine(_) => "quarantine",
            Disposition::AlreadyManaged => "managed",
        }
    }
}

/// One row of the deterministic proposed Space mapping.
#[derive(Debug, Clone)]
pub struct Mapping {
    /// 1-based position in the printed plan. This is the "current row index"
    /// §17.7 says is NOT preserved: `space_no` is allocated from the
    /// registry's permanent counter and generally differs.
    pub row: usize,
    pub backend: Backend,
    pub name: String,
    pub native_token: Option<String>,
    pub native_ref: Option<String>,
    pub groups: u32,
    pub splits: u32,
    pub disposition: Disposition,
    /// The permanent SpaceNo: proposed for `Adopt`, actual for the rest.
    pub space_no: Option<u64>,
    pub space_uid: Option<SpaceUid>,
    /// Another row carries the same logical name on the other backend
    /// (§17.10). Presentation only — `operations` owns the refusal.
    pub duplicate_name: bool,
    pub remedy: Option<String>,
}

impl Mapping {
    fn json(&self) -> Value {
        json!({
            "row": self.row,
            "disposition": self.disposition.token(),
            "reason": match &self.disposition {
                Disposition::Quarantine(reason) => Value::from(*reason),
                _ => Value::Null,
            },
            "backend": self.backend.as_str(),
            "name": self.name,
            "native_ref": self.native_ref,
            "native_token": self.native_token,
            "groups": self.groups,
            "splits": self.splits,
            "space_no": self.space_no,
            "space_uid": self.space_uid.map(|uid| uid.0.to_string()),
            "duplicate_name": self.duplicate_name,
            "remedy": self.remedy,
        })
    }
}

/// One legacy `dmux -` entry's fate (§17.11).
#[derive(Debug, Clone)]
pub struct HistoryPlanEntry {
    pub key: String,
    pub name: String,
    /// `convert` / `drop_ambiguous` / `drop_missing`.
    pub outcome: &'static str,
    pub space_no: Option<u64>,
    pub space_uid: Option<SpaceUid>,
    pub candidates: Option<u32>,
}

impl HistoryPlanEntry {
    fn json(&self) -> Value {
        json!({
            "key": self.key,
            "name": self.name,
            "outcome": self.outcome,
            "space_no": self.space_no,
            "space_uid": self.space_uid.map(|uid| uid.0.to_string()),
            "candidates": self.candidates,
        })
    }

    fn line(&self) -> String {
        match self.outcome {
            "convert" => format!(
                "history: {} {:?} -> space {}",
                self.key,
                self.name,
                self.space_no.unwrap_or_default()
            ),
            "drop_ambiguous" => format!(
                "history: dropped {} {:?}: ambiguous ({} candidates)",
                self.key,
                self.name,
                self.candidates.unwrap_or_default()
            ),
            _ => format!(
                "history: dropped {} {:?}: no Space of that name on this authority",
                self.key, self.name
            ),
        }
    }
}

/// A §17 precondition this driver verifies rather than re-implements.
#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn json(&self) -> Value {
        json!({ "check": self.name, "ok": self.ok, "detail": self.detail })
    }
}

/// The whole previewable plan.
pub struct MigratePlan {
    pub backup_path: PathBuf,
    pub backup_exists: bool,
    pub mappings: Vec<Mapping>,
    pub history: Vec<HistoryPlanEntry>,
    pub checks: Vec<Check>,
    /// Reasons the plan cannot be committed. Never empty on a refusal.
    pub blockers: Vec<TypedError>,
}

impl MigratePlan {
    fn adoptable(&self) -> impl Iterator<Item = &Mapping> {
        self.mappings
            .iter()
            .filter(|m| m.disposition == Disposition::Adopt)
    }

    fn json(&self, committed: bool) -> Value {
        json!({
            "committed": committed,
            "already_migrated": false,
            "backup": {
                "path": self.backup_path.display().to_string(),
                "exists": self.backup_exists,
            },
            "spaces": self.mappings.iter().map(Mapping::json).collect::<Vec<_>>(),
            "history": self.history.iter().map(HistoryPlanEntry::json).collect::<Vec<_>>(),
            "checks": self.checks.iter().map(Check::json).collect::<Vec<_>>(),
        })
    }

    /// The human preview — also exactly what the interactive confirmation
    /// shows, because §14 requires the operator to see what is about to
    /// change before agreeing to it.
    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "backup: {} ({})\n",
            self.backup_path.display(),
            if self.backup_exists {
                "already present; kept as the pre-migration copy"
            } else {
                "written by --commit"
            }
        ));
        for mapping in &self.mappings {
            let target = match mapping.space_no {
                Some(no) => format!("space {no}"),
                None => "-".to_string(),
            };
            out.push_str(&format!(
                "  {:>3}  {:<10}  {:<4}  {:<24}  {}",
                mapping.row,
                mapping.disposition.token(),
                mapping.backend.as_str(),
                output::one_line(&mapping.name),
                target,
            ));
            if let Some(remedy) = &mapping.remedy {
                out.push_str(&format!("  {remedy}"));
            }
            if mapping.duplicate_name {
                out.push_str("  [duplicate name: addressable by ref only]");
            }
            out.push('\n');
        }
        for entry in &self.history {
            out.push_str(&entry.line());
            out.push('\n');
        }
        for check in &self.checks {
            out.push_str(&format!(
                "check: {} {}: {}\n",
                check.name,
                if check.ok { "ok" } else { "FAILED" },
                check.detail
            ));
        }
        for blocker in &self.blockers {
            out.push_str(&format!("blocked: {}\n", blocker.message));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Entry point

pub fn migrate_in<R: WezRunner>(
    env: MigrateEnv<'_, R>,
    format: Option<OutputFormat>,
    args: MigrateArgs,
) -> MigrateOutput {
    let MigrateEnv {
        ops,
        wez,
        history,
        consent,
    } = env;
    // One provider for both the owner scan and every adoption, so a test's
    // fake mux answers the same `list` the cutover then acts on.
    let wez = WezProvider::with_runner(wez.bin, wez.config, wez.runner);
    let env = &Bound {
        ops,
        wez,
        history,
        consent,
    };

    // The recorded cutover answers first, before any scan: a second run is a
    // documented no-op (§16.3 exit 0), not a second migration.
    match read_stamp(&stamp_path(env.ops)) {
        Ok(Some(receipt)) => return already_migrated(format, revision(env.ops), receipt),
        Ok(None) => {}
        Err(error) => return refuse(format, revision(env.ops), error),
    }

    let plan = match build_plan(env, &args) {
        Ok(plan) => plan,
        Err(error) => return refuse(format, revision(env.ops), error),
    };

    if !args.commit {
        return preview(format, revision(env.ops), &plan);
    }
    if !plan.blockers.is_empty() {
        return blocked(format, revision(env.ops), &plan);
    }
    if !args.yes {
        if format == Some(OutputFormat::Json) {
            // §7.4: JSON destructive commands never prompt. One document,
            // nothing mutated, exit 5 — and the plan travels inside it, so
            // `--yes` is not agreement to something unseen.
            let (mut document, status) =
                output::confirmation_required(ACTION, "migrate --commit", revision(env.ops));
            document["result"] = plan.json(false);
            return MigrateOutput {
                status,
                stdout: format!("{document}\n"),
                stderr: String::new(),
            };
        }
        match env.consent.confirm(&plan.render()) {
            Some(true) => {}
            Some(false) => {
                return refuse(
                    format,
                    revision(env.ops),
                    TypedError::new(
                        ErrorCode::ConfirmationDeclined,
                        "migration declined; nothing was adopted",
                    ),
                );
            }
            None => {
                return refuse(
                    format,
                    revision(env.ops),
                    TypedError::new(
                        ErrorCode::ConfirmationRequired,
                        "migrate --commit needs confirmation (re-run with --yes)",
                    ),
                );
            }
        }
    }

    commit(env, format, plan)
}

// ---------------------------------------------------------------------------
// Planning (§17.6, §17.7, §17.9, §17.11)

fn build_plan<R: WezRunner>(
    env: &Bound<'_, R>,
    args: &MigrateArgs,
) -> Result<MigratePlan, TypedError> {
    let registry = open(env.ops)?;
    let mut blockers = Vec::new();
    let mut checks = Vec::new();

    // Durable records plus their current bindings, exactly as `ls` joins
    // them: a bound native is managed and is never a migration candidate.
    let spaces = registry.spaces().map_err(reg)?;
    let mut backends: HashMap<BackendInstanceUid, Backend> = HashMap::new();
    let mut records = Vec::with_capacity(spaces.len());
    for space in spaces {
        if let std::collections::hash_map::Entry::Vacant(slot) =
            backends.entry(space.backend_instance)
        {
            slot.insert(
                registry
                    .backend_instance_info(space.backend_instance)
                    .map_err(reg)?
                    .backend,
            );
        }
        let binding = registry.current_binding(space.space_uid).map_err(reg)?;
        records.push((space, binding));
    }

    // Unlike `ls`, migration never falls back to a well-known socket: §17
    // steps 2–4 import the backend definitions before step 6 scans them, and
    // guessing `tmux -L default` on a machine that never enrolled would
    // batch-adopt whatever that server happens to hold. `resolve_managed`
    // gives exactly that — a scope only for a registered, addressable,
    // *published* instance, and a distinct arm for every reason there is none.
    let wez_target = scope::resolve_managed(&registry, Backend::Wez).map_err(reg)?;
    let tmux_target = scope::resolve_managed(&registry, Backend::Tmux).map_err(reg)?;
    let scans = scan_backends(env, &wez_target, &tmux_target)?;

    // §17.6: a migration reads a *complete* owner scan or it reads nothing.
    // An indeterminate scan is not an empty backend (§2.10) — committing on
    // one would quarantine live resources that simply were not seen.
    let mut scanned = Vec::new();
    for (backend, target) in [(Backend::Wez, &wez_target), (Backend::Tmux, &tmux_target)] {
        match target {
            ManagedTarget::Managed { .. } => {
                scanned.push(backend.as_str());
                if let Some(detail) = indeterminate_detail(scans.get(backend)) {
                    blockers.push(TypedError::new(
                        scan_error_code(scans.get(backend)),
                        format!(
                            "{backend} owner scan is indeterminate, so the migration cannot tell \
                             an absent resource from an unseen one: {detail}"
                        ),
                    ));
                }
            }
            // A registered, addressable instance that has published no server
            // epoch is refused, not scanned. An unpinned scan of its endpoint
            // would trust whatever server answered, and a `Complete` answer
            // from a stranger would make every one of its workspaces look
            // adoptable — which is exactly how the review drove `migrate`
            // against a foreign mux and got `adopted:2` (finding #3). Blocking
            // it here refuses the cutover in preview (a typed failure document,
            // never "nothing to migrate") and in --commit, before the backup,
            // any adoption, or the stamp.
            ManagedTarget::Unpublished(instance) => {
                blockers.push(TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    ManagedTarget::unpublished_detail(backend, *instance),
                ));
            }
            // A published epoch the host refutes (state F) blocks the same
            // way: a scan pinned to it would be refused, and a cutover that
            // read "nothing to migrate" off a stale row would stamp itself
            // done against a server it never saw.
            ManagedTarget::StaleIncarnation {
                instance,
                published,
                observed,
            } => {
                blockers.push(TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    ManagedTarget::stale_incarnation_detail(
                        backend, *instance, published, observed,
                    ),
                ));
            }
            // Nothing registered, or registered with no endpoint: a machine
            // that never enrolled this backend is a normal state, not a
            // failed cutover, so there is nothing to migrate for it.
            ManagedTarget::Unaddressable(_) | ManagedTarget::Unregistered => {}
        }
    }
    checks.push(Check {
        name: "complete_owner_scans",
        ok: blockers.is_empty(),
        detail: if scanned.is_empty() {
            "no backend instance is registered on this authority".to_string()
        } else {
            format!("registered backends scanned: {}", scanned.join(", "))
        },
    });
    checks.push(remote_wez_route_check(&registry)?);

    let rows = inventory::reconcile(
        &records,
        |space| backends.get(&space.backend_instance).copied(),
        &scans,
    );

    // Deterministic order: backend, then logical name, then native token.
    // Nothing here reads a provider's listing order, so two runs against one
    // authority print the same plan and propose the same numbers.
    let mut sorted: Vec<&ReconRow> = rows.iter().collect();
    sorted.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));

    let mut names: BTreeMap<String, u32> = BTreeMap::new();
    for row in &sorted {
        *names.entry(row_name(row)).or_default() += 1;
    }

    let mut next_space_no = next_space_no(&registry)?;
    let mut mappings = Vec::with_capacity(sorted.len());
    for (index, row) in sorted.iter().enumerate() {
        let name = row_name(row);
        let duplicate_name = names.get(&name).copied().unwrap_or(0) > 1;
        let row_no = index + 1;
        let mapping = match row {
            ReconRow::Managed(managed) => {
                // §17.9: the migration cannot commit a managed multi-window
                // Space. One that already exists blocks the cutover with the
                // repair that fixes it, rather than being quietly adopted.
                if managed.space.health == Health::MultiWindow || managed.multi_window {
                    blockers.push(TypedError::new(
                        ErrorCode::RepairRequired,
                        format!(
                            "Space {} ({:?}) spans multiple windows; run `dmux repair normalize \
                             {}` before migrating",
                            managed.space.space_no,
                            managed.space.logical_name,
                            managed.space.space_no
                        ),
                    ));
                }
                Mapping {
                    row: row_no,
                    backend: managed.backend,
                    name,
                    native_token: managed.native_token.clone(),
                    native_ref: managed
                        .native_token
                        .as_ref()
                        .map(|token| output::native_ref(managed.backend, token)),
                    groups: managed.groups,
                    splits: managed.splits,
                    disposition: Disposition::AlreadyManaged,
                    space_no: Some(managed.space.space_no.get()),
                    space_uid: Some(managed.space.space_uid),
                    duplicate_name,
                    remedy: None,
                }
            }
            ReconRow::Unmanaged(native) => {
                let native_ref = output::native_ref(native.backend, &native.native_token);
                let (disposition, remedy) = if native.multi_window {
                    // §17.9's second half: normalize it or leave it
                    // unmanaged. Migration takes the safe branch by itself
                    // and names the one that makes it adoptable.
                    (
                        Disposition::Quarantine("multi_window"),
                        Some(format!("run `dmux repair normalize {native_ref}` first")),
                    )
                } else if native.unepoched {
                    (
                        Disposition::Quarantine("unepoched"),
                        Some(
                            "the server carries no managed epoch; restart it under the dmux \
                             service before adopting"
                                .to_string(),
                        ),
                    )
                } else {
                    (Disposition::Adopt, None)
                };
                let space_no = (disposition == Disposition::Adopt).then(|| {
                    let no = next_space_no;
                    next_space_no += 1;
                    no
                });
                Mapping {
                    row: row_no,
                    backend: native.backend,
                    name,
                    native_token: Some(native.native_token.clone()),
                    native_ref: Some(native_ref),
                    groups: native.groups,
                    splits: native.splits,
                    disposition,
                    space_no,
                    space_uid: None,
                    duplicate_name,
                    remedy,
                }
            }
        };
        mappings.push(mapping);
    }

    let backup_path = backup_path(env.ops);
    Ok(MigratePlan {
        backup_exists: backup_path.exists(),
        backup_path,
        history: plan_history(&mappings, &args.previous_sessions),
        mappings,
        checks,
        blockers,
    })
}

/// Sort key for the printed plan. Managed and unmanaged rows interleave by
/// backend and name, so the mapping reads as one table rather than two.
fn sort_key(row: &ReconRow) -> (&'static str, String, String) {
    match row {
        ReconRow::Managed(m) => (
            m.backend.as_str(),
            m.space.logical_name.clone(),
            m.native_token.clone().unwrap_or_default(),
        ),
        ReconRow::Unmanaged(u) => (
            u.backend.as_str(),
            u.native_name.clone(),
            u.native_token.clone(),
        ),
    }
}

fn row_name(row: &ReconRow) -> String {
    match row {
        ReconRow::Managed(m) => m.space.logical_name.clone(),
        ReconRow::Unmanaged(u) => u.native_name.clone(),
    }
}

/// The next SpaceNo the registry will hand out. Read from the authority's
/// own monotonic counter, not from `max(space_no) + 1`: a removed Space's
/// number is never reused, so the maximum of the live rows would propose a
/// number that has already been spent.
fn next_space_no(registry: &Registry) -> Result<u64, TypedError> {
    registry
        .raw_connection()
        .query_row(
            "SELECT space_no_counter FROM meta WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|no| no as u64)
        .map_err(|e| operation_failed(format!("reading the SpaceNo counter: {e}")))
}

/// Both owner scans, under the same shared fences `ls` takes: a recovering
/// or mutating instance is not scanned at all, because a half-restored tree
/// read as a complete inventory would publish live Spaces as unmanaged and
/// this driver would then adopt them.
/// The instance whose shared fence a scan holds: only a Managed target is
/// probed, so only its instance is fenced. An Unpublished or Unaddressable
/// instance is registered but never scanned, so fencing it would be lock
/// traffic for a probe that does not happen.
fn probed_instance(target: &ManagedTarget) -> Option<BackendInstanceUid> {
    match target {
        ManagedTarget::Managed { instance, .. } => Some(*instance),
        _ => None,
    }
}

fn scan_backends<R: WezRunner>(
    env: &Bound<'_, R>,
    wez: &ManagedTarget,
    tmux: &ManagedTarget,
) -> Result<inventory::BackendScans, TypedError> {
    let mut locks = OrderedLocks::new(&env.ops.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| operation_failed(format!("authority scan lock: {e}")))?;
    let mut fenced = Vec::new();
    // Same-rank scopes are taken in increasing key order (§10.1).
    let mut ordered = [probed_instance(wez), probed_instance(tmux)];
    ordered.sort_by_key(|instance| instance.map(|i| LockScope::BackendInstance(i).key()));
    for instance in ordered.into_iter().flatten() {
        if locks
            .try_acquire(LockScope::BackendInstance(instance), LockMode::Shared)
            .map_err(|e| operation_failed(format!("backend scan lock: {e}")))?
        {
            fenced.push(instance);
        }
    }

    let probe =
        |target: &ManagedTarget, run: &dyn Fn(&InventoryScope) -> InventoryOutcome| match target {
            ManagedTarget::Managed { instance, scope } if fenced.contains(instance) => run(scope),
            ManagedTarget::Managed { .. } => InventoryOutcome::Unreachable {
                detail: "backend instance is recovering or mutating".into(),
            },
            // No scope, so `run` is never called and nothing is probed. This
            // detail is not read — build_plan refuses an Unpublished target
            // by its own arm — but a scan result is still required here.
            ManagedTarget::Unpublished(_) => InventoryOutcome::Unreachable {
                detail: "backend instance has published no server epoch".into(),
            },
            // Same: no scope, nothing probed; build_plan blocks on its own arm.
            ManagedTarget::StaleIncarnation { .. } => InventoryOutcome::Unreachable {
                detail: "backend instance publishes a stale incarnation (stale_incarnation)".into(),
            },
            ManagedTarget::Unaddressable(_) => InventoryOutcome::Unreachable {
                detail: "the registered backend instance has no recorded endpoint".into(),
            },
            // Nothing registered, so nothing is probed and nothing is
            // established: reconcile must not read this as an empty backend.
            ManagedTarget::Unregistered => InventoryOutcome::Unreachable {
                detail: "no backend instance is registered".into(),
            },
        };
    // Sequential on purpose: `inventory::scan_both` needs both closures to
    // be `Send`, which would force the injected wezterm runner to be `Sync`
    // for no benefit — a one-time cutover has nothing to gain from overlapping
    // two local probes, and both still run under the same held fences.
    let scans = inventory::BackendScans {
        wez: probe(wez, &|scope| env.wez.inventory(scope)),
        tmux: probe(tmux, &|scope| {
            TmuxProvider::new(scope.endpoint.clone()).inventory(scope)
        }),
    };
    locks.release_all();
    Ok(scans)
}

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

fn scan_error_code(outcome: &InventoryOutcome) -> ErrorCode {
    match outcome {
        InventoryOutcome::VersionMismatch { .. } => ErrorCode::VersionMismatch,
        InventoryOutcome::ProtocolMismatch { .. } => ErrorCode::ProtocolMismatch,
        InventoryOutcome::AuthFailed { .. } | InventoryOutcome::HostKeyIdentityFailed { .. } => {
            ErrorCode::AuthFailed
        }
        _ => ErrorCode::ProviderUnavailable,
    }
}

/// §17.5, verified rather than re-implemented. Route selection is a
/// priority-ordered walk that returns on the first route that answers
/// (`remote::client::call_over_routes`), and each route has its own
/// `routes::wez_domain_name`, so two enabled Wez routes to one host are two
/// distinct domains that are never attached in the same call.
fn remote_wez_route_check(registry: &Registry) -> Result<Check, TypedError> {
    let local = registry.identity().map_err(reg)?.host_uid;
    let mut described = Vec::new();
    for host in registry.hosts().map_err(reg)? {
        if host.host_uid == local || host.lifecycle != HostLifecycle::Enrolled {
            continue;
        }
        let enabled = registry
            .routes_for(host.host_uid)
            .map_err(reg)?
            .into_iter()
            .filter(|route| route.enabled && route.wez_domain.is_some())
            .count();
        described.push(format!(
            "{}={enabled}",
            host.alias.unwrap_or_else(|| host.host_uid.0.to_string())
        ));
    }
    Ok(Check {
        name: "single_remote_wez_route",
        ok: true,
        detail: format!(
            "enabled Wez routes per peer [{}]; selection is priority-ordered first-success, \
             one domain per attach",
            described.join(" ")
        ),
    })
}

// ---------------------------------------------------------------------------
// History conversion (§17.11)

/// The legacy `key session` lines, previewed against the plan and applied
/// against the post-adoption registry. Both phases go through
/// [`convert_legacy_entries`] so the ambiguity rule lives in exactly one
/// place; only this driver decides what a hit means.
fn plan_history(
    mappings: &[Mapping],
    previous_sessions: &BTreeMap<String, String>,
) -> Vec<HistoryPlanEntry> {
    let entries: Vec<LegacyEntry> = previous_sessions
        .iter()
        .map(|(key, name)| LegacyEntry {
            key: key.clone(),
            name: name.clone(),
        })
        .collect();
    if entries.is_empty() {
        return Vec::new();
    }

    // Only names this authority will own after the cutover can convert. A
    // peer's names live in the peer's registry and are not resolvable here,
    // so those entries drop with the same `missing` warning §17.11 asks for.
    let mut index: BTreeMap<String, (u32, Option<u64>, Option<SpaceUid>)> = BTreeMap::new();
    for mapping in mappings {
        if matches!(mapping.disposition, Disposition::Quarantine(_)) {
            continue;
        }
        let slot =
            index
                .entry(mapping.name.clone())
                .or_insert((0, mapping.space_no, mapping.space_uid));
        slot.0 += 1;
    }

    let (converted, warnings) = convert_legacy_entries(&entries, |name| {
        index
            .get(name)
            .map(|(count, _, uid)| (uid.unwrap_or(UNALLOCATED), *count))
    });

    let mut plan = Vec::with_capacity(entries.len());
    for (key, space_uid) in converted {
        let name = previous_sessions.get(&key).cloned().unwrap_or_default();
        let (_, space_no, _) = index.get(&name).copied().unwrap_or((0, None, None));
        plan.push(HistoryPlanEntry {
            key,
            name,
            outcome: "convert",
            space_no,
            space_uid: (space_uid != UNALLOCATED).then_some(space_uid),
            candidates: None,
        });
    }
    for warning in warnings {
        let (outcome, candidates) = match warning.reason {
            ConvertDropReason::Ambiguous { candidates } => ("drop_ambiguous", Some(candidates)),
            ConvertDropReason::Missing => ("drop_missing", None),
        };
        plan.push(HistoryPlanEntry {
            key: warning.key,
            name: warning.name,
            outcome,
            space_no: None,
            space_uid: None,
            candidates,
        });
    }
    plan.sort_by(|a, b| a.key.cmp(&b.key));
    plan
}

/// Apply the converted entries. The legacy file's bare `host` key is the
/// toggle target and `host:current` is what dmux attached last, so replaying
/// them in that order through [`History::record_attach`] lands previous and
/// current in exactly those slots. A host spelling that no longer resolves
/// drops with a warning rather than seeding an unowned slot.
fn apply_history(
    registry: &Registry,
    history: &History,
    planned: &[HistoryPlanEntry],
) -> Vec<TypedError> {
    let mut per_host: BTreeMap<String, (HostUid, Option<SpaceUid>, Option<SpaceUid>)> =
        BTreeMap::new();
    let mut warnings = Vec::new();
    for entry in planned.iter().filter(|e| e.outcome == "convert") {
        let Some(space_uid) = entry.space_uid else {
            continue;
        };
        let (host_token, is_current) = match entry.key.split_once(':') {
            Some((host, "current")) => (host, true),
            _ => (entry.key.as_str(), false),
        };
        match crate::remote::hosts::resolve_host(registry, host_token) {
            Ok(host) => {
                let slot = per_host.entry(host.host_uid.0.to_string()).or_insert((
                    host.host_uid,
                    None,
                    None,
                ));
                if is_current {
                    slot.2 = Some(space_uid);
                } else {
                    slot.1 = Some(space_uid);
                }
            }
            Err(_) => warnings.push(TypedError::new(
                ErrorCode::NotFound,
                format!(
                    "dropped `dmux -` history for {:?}: no enrolled host answers to that legacy \
                     spelling",
                    entry.key
                ),
            )),
        }
    }
    for (_, (host, previous, current)) in per_host {
        for space in [previous, current].into_iter().flatten() {
            if let Err(error) = history.record_attach(host, space) {
                warnings.push(TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("recording converted history for {}: {error}", host.0),
                ));
            }
        }
    }
    warnings
}

// ---------------------------------------------------------------------------
// Commit (§17.1, §17.8)

fn commit<R: WezRunner>(
    env: &Bound<'_, R>,
    format: Option<OutputFormat>,
    mut plan: MigratePlan,
) -> MigrateOutput {
    let mut errors = Vec::new();

    // §17.1 first, before anything is adopted: the backup is only a backup
    // if it predates the mutation it exists to undo.
    let backup_note = match back_up(env.ops, &plan.backup_path) {
        Ok(note) => note,
        Err(error) => return refuse(format, revision(env.ops), error),
    };

    // §17.8: batch-adopt through the normal adoption lease. Each row is one
    // fenced operation; a refusal is that row's, not the batch's, so one
    // duplicate name cannot strand the rest of the cutover.
    let mut adopted = 0usize;
    for mapping in plan.mappings.iter_mut() {
        if mapping.disposition != Disposition::Adopt {
            continue;
        }
        let (Some(token), Some(reference)) = (&mapping.native_token, &mapping.native_ref) else {
            continue;
        };
        let scope = match scan_scope(env, mapping.backend) {
            Ok(scope) => scope,
            Err(error) => {
                errors.push(with_target(error, reference));
                continue;
            }
        };
        let request_uid = Uuid::new_v4();
        let outcome = match mapping.backend {
            Backend::Tmux => adopt_tmux(
                env.ops,
                &TmuxProvider::new(scope.endpoint.clone()),
                &scope,
                token,
                None,
                request_uid,
            ),
            Backend::Wez => adopt_wez(env.ops, &env.wez, &scope, token, None, request_uid),
        };
        match outcome {
            Ok(space) => {
                adopted += 1;
                mapping.space_uid = Some(space.space_uid);
                mapping.space_no = Some(space.space_no.get());
                mapping.name = space.name;
                mapping.native_token = Some(space.native_token);
            }
            Err(error) => {
                mapping.disposition = Disposition::Quarantine("adoption_refused");
                mapping.space_no = None;
                errors.push(with_target(typed(&error), reference));
            }
        }
    }

    // §17.11: fill in the SpaceUids the preview could not know. The
    // keep/drop decision itself is NOT revisited here. It was made against
    // the candidate set — what the legacy name could have meant when it was
    // written — and the registry's own live-name uniqueness means at most
    // one of a duplicate pair can survive the cutover. Re-judging against
    // the survivor would silently convert a name the preview correctly
    // refused to guess at.
    for entry in plan.history.iter_mut().filter(|e| e.outcome == "convert") {
        match adopted_identity(&plan.mappings, &entry.name) {
            Some((space_no, space_uid)) => {
                entry.space_no = Some(space_no);
                entry.space_uid = Some(space_uid);
            }
            // Its row was refused and is quarantined: there is nothing left
            // for the toggle to point at.
            None => {
                entry.outcome = "drop_missing";
                entry.space_no = None;
                entry.space_uid = None;
            }
        }
    }
    match open(env.ops) {
        Ok(registry) => errors.extend(apply_history(&registry, &env.history, &plan.history)),
        Err(error) => errors.push(error),
    }

    // The cutover is recorded only when every planned adoption resolved.
    // A failed row keeps `migrate` available for the retry, and the rows
    // that did land are already bound, so the retry cannot re-adopt them.
    let stamped = errors.is_empty();
    if stamped && let Err(error) = write_stamp(&stamp_path(env.ops), &plan, adopted) {
        errors.push(error);
    }

    let mut result = plan.json(true);
    result["adopted"] = json!(adopted);
    result["backup"]["created"] = json!(backup_note.created);
    result["recorded"] = json!(stamped && errors.is_empty());
    render(
        format,
        revision(env.ops),
        ACTION,
        true,
        result,
        errors,
        &commit_lines(&plan, adopted, &backup_note),
    )
}

/// The identity a migrated row ended up with, for the history pass. A
/// quarantined row has none — refused or deliberately left unmanaged, it
/// owns no Space.
fn adopted_identity(mappings: &[Mapping], name: &str) -> Option<(u64, SpaceUid)> {
    mappings.iter().find_map(|mapping| {
        if mapping.name != name || matches!(mapping.disposition, Disposition::Quarantine(_)) {
            return None;
        }
        mapping.space_no.zip(mapping.space_uid)
    })
}

struct BackupNote {
    path: PathBuf,
    created: bool,
}

/// §17.1. The registry's own WAL-safe online backup, never a file copy; an
/// existing pre-migration copy is kept, because the only run whose state is
/// worth preserving is the first one.
fn back_up(env: &OperationEnv, dest: &Path) -> Result<BackupNote, TypedError> {
    if dest.exists() {
        return Ok(BackupNote {
            path: dest.to_path_buf(),
            created: false,
        });
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| operation_failed(format!("creating {}: {e}", parent.display())))?;
    }
    open(env)?.backup_to(dest).map_err(reg)?;
    Ok(BackupNote {
        path: dest.to_path_buf(),
        created: true,
    })
}

fn scan_scope<R: WezRunner>(
    env: &Bound<'_, R>,
    backend: Backend,
) -> Result<InventoryScope, TypedError> {
    // Reached only for an adopt row, which only a Managed target produces, so
    // a build that blocked on an Unpublished instance never gets here. The
    // arms are still written out so a future caller cannot adopt onto an
    // unverified server through this seam either.
    match scope::resolve_managed(&open(env.ops)?, backend).map_err(reg)? {
        ManagedTarget::Managed { scope, .. } => Ok(scope),
        ManagedTarget::Unpublished(instance) => Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            ManagedTarget::unpublished_detail(backend, instance),
        )),
        ManagedTarget::StaleIncarnation {
            instance,
            published,
            observed,
        } => Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            ManagedTarget::stale_incarnation_detail(backend, instance, &published, &observed),
        )),
        ManagedTarget::Unaddressable(instance) => Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            ManagedTarget::unaddressable_detail(backend, instance),
        )),
        ManagedTarget::Unregistered => Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!("no addressable managed {backend} instance"),
        )),
    }
}

// ---------------------------------------------------------------------------
// The recorded cutover

fn backup_path(env: &OperationEnv) -> PathBuf {
    beside_registry(env, BACKUP_FILE)
}

fn stamp_path(env: &OperationEnv) -> PathBuf {
    beside_registry(env, STAMP_FILE)
}

fn beside_registry(env: &OperationEnv, file: &str) -> PathBuf {
    env.db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file)
}

/// `Ok(None)` when this authority has not migrated. A stamp that exists but
/// cannot be read is not treated as absent: that would migrate a second
/// time, which case 45 forbids.
fn read_stamp(path: &Path) -> Result<Option<Value>, TypedError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(operation_failed(format!("reading {}: {e}", path.display()))),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| operation_failed(format!("{} is unreadable: {e}", path.display())))?;
    if value.get("version").and_then(Value::as_u64) != Some(STAMP_VERSION) {
        return Err(operation_failed(format!(
            "{} records an unknown migration version; inspect it before migrating again",
            path.display()
        )));
    }
    Ok(Some(value))
}

fn write_stamp(path: &Path, plan: &MigratePlan, adopted: usize) -> Result<(), TypedError> {
    let receipt = json!({
        "version": STAMP_VERSION,
        "completed_at": crate::registry::now_rfc3339(),
        "adopted": adopted,
        "backup": plan.backup_path.display().to_string(),
        "spaces": plan.mappings.iter().map(Mapping::json).collect::<Vec<_>>(),
        "history": plan.history.iter().map(HistoryPlanEntry::json).collect::<Vec<_>>(),
    });
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| operation_failed(format!("creating {}: {e}", parent.display())))?;
    // Same-directory temp file then rename: a crash leaves the old document
    // or the new one, never a half-written stamp that reads as corrupt and
    // blocks the verb forever.
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| operation_failed(format!("staging {}: {e}", path.display())))?;
    let staged = format!(
        "{}\n",
        serde_json::to_string_pretty(&receipt)
            .map_err(|e| operation_failed(format!("rendering {}: {e}", path.display())))?
    );
    temp.write_all(staged.as_bytes())
        .map_err(|e| operation_failed(format!("writing {}: {e}", path.display())))?;
    temp.persist(path)
        .map_err(|e| operation_failed(format!("publishing {}: {e}", path.display())))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — one §16.2 document on every branch, refusals included

fn preview(format: Option<OutputFormat>, revision: u64, plan: &MigratePlan) -> MigrateOutput {
    let mut human = String::from("dmux: migrate preview — nothing is adopted without --commit\n");
    human.push_str(&plan.render());
    human.push_str("run `dmux migrate --commit` to apply\n");
    render(
        format,
        revision,
        ACTION,
        true,
        plan.json(false),
        plan.blockers.clone(),
        &human,
    )
}

/// A `--commit` that cannot run. The plan still travels in `result` — the
/// operator has to see what blocked it — but it is context, not a result, so
/// the exit status is the first blocker's rather than partial.
fn blocked(format: Option<OutputFormat>, revision: u64, plan: &MigratePlan) -> MigrateOutput {
    if format == Some(OutputFormat::Json) {
        let document = output::document(ACTION, false, plan.json(false), &plan.blockers, revision);
        return MigrateOutput {
            status: output::document_exit(false, false, &plan.blockers),
            stdout: format!("{document}\n"),
            stderr: String::new(),
        };
    }
    MigrateOutput {
        status: output::document_exit(false, false, &plan.blockers),
        stdout: String::new(),
        stderr: plan
            .blockers
            .iter()
            .map(|error| format!("dmux: {}\n", error.message))
            .collect(),
    }
}

fn already_migrated(
    format: Option<OutputFormat>,
    revision: u64,
    mut receipt: Value,
) -> MigrateOutput {
    let completed = receipt
        .get("completed_at")
        .and_then(Value::as_str)
        .unwrap_or("an earlier run")
        .to_string();
    receipt["already_migrated"] = json!(true);
    receipt["committed"] = json!(false);
    render(
        format,
        revision,
        ACTION,
        true,
        receipt,
        Vec::new(),
        &format!(
            "dmux: already migrated at {completed}; nothing to do\n\
             adopt anything created since with `dmux adopt NATIVE_REF`\n"
        ),
    )
}

fn refuse(format: Option<OutputFormat>, revision: u64, error: TypedError) -> MigrateOutput {
    let status = error.code.exit_status();
    if format == Some(OutputFormat::Json) {
        let errors = [error];
        return MigrateOutput {
            status,
            stdout: format!(
                "{}\n",
                output::document(ACTION, false, Value::Null, &errors, revision)
            ),
            stderr: String::new(),
        };
    }
    MigrateOutput {
        status,
        stdout: String::new(),
        stderr: format!("dmux: {}\n", error.message),
    }
}

fn render(
    format: Option<OutputFormat>,
    revision: u64,
    action: &str,
    ok: bool,
    result: Value,
    errors: Vec<TypedError>,
    human: &str,
) -> MigrateOutput {
    if format == Some(OutputFormat::Json) {
        let document = output::document(action, ok, result, &errors, revision);
        return MigrateOutput {
            status: output::document_exit(ok, true, &errors),
            stdout: format!("{document}\n"),
            stderr: String::new(),
        };
    }
    MigrateOutput {
        status: output::document_exit(ok, true, &errors),
        stdout: human.to_string(),
        stderr: errors
            .iter()
            .map(|error| format!("dmux: {}\n", error.message))
            .collect(),
    }
}

fn commit_lines(plan: &MigratePlan, adopted: usize, backup: &BackupNote) -> String {
    let quarantined = plan
        .mappings
        .iter()
        .filter(|m| matches!(m.disposition, Disposition::Quarantine(_)))
        .count();
    let managed = plan
        .mappings
        .iter()
        .filter(|m| m.disposition == Disposition::AlreadyManaged)
        .count();
    let mut out = format!(
        "dmux: migrated {adopted}, quarantined {quarantined}, already managed {managed}\n\
         backup: {} ({})\n",
        backup.path.display(),
        if backup.created {
            "written"
        } else {
            "kept from an earlier run"
        }
    );
    for mapping in plan.adoptable() {
        let Some(space_no) = mapping.space_no else {
            continue;
        };
        out.push_str(&format!(
            "adopted {space_no} {:?} ({})\n\
             unstamped: every pane that predates adoption must run `dmux context stamp {space_no}`\n",
            output::one_line(&mapping.name),
            mapping.backend,
        ));
    }
    for mapping in plan
        .mappings
        .iter()
        .filter(|m| matches!(m.disposition, Disposition::Quarantine(_)))
    {
        out.push_str(&format!(
            "quarantined {} {:?} unmanaged: {}\n",
            mapping.backend,
            output::one_line(&mapping.name),
            mapping.remedy.as_deref().unwrap_or("left as it was"),
        ));
    }
    for entry in &plan.history {
        out.push_str(&entry.line());
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Small shared helpers

fn open(env: &OperationEnv) -> Result<Registry, TypedError> {
    Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))
}

fn reg(e: crate::registry::RegistryError) -> TypedError {
    TypedError::new(e.error_code(), e.to_string())
}

fn operation_failed(message: String) -> TypedError {
    TypedError::new(ErrorCode::OperationFailed, message)
}

fn revision(env: &OperationEnv) -> u64 {
    open(env)
        .and_then(|r| r.authority_head().map_err(reg))
        .map_or(0, |head| head.revision)
}

fn with_target(mut error: TypedError, target: &str) -> TypedError {
    error.target = Some(target.to_string());
    error
}

/// One adoption's stringly error as the plan's typed codes, in the same
/// partition `adopt_cli` uses — the batch is not entitled to a weaker one.
fn typed(err: &OpError) -> TypedError {
    let (code, message) = match err {
        OpError::Provider(detail) if detail.contains("cas_capability_missing") => (
            ErrorCode::VersionMismatch,
            format!(
                "the managed WezTerm server lacks the fork CAS rename verb (ADR 006), so this \
                 workspace stays unmanaged (plan §2.7): {detail}"
            ),
        ),
        OpError::Provider(detail) if detail.contains("spans multiple windows") => (
            ErrorCode::RepairRequired,
            format!("run `dmux repair normalize` first: {detail}"),
        ),
        OpError::NameConflict(detail)
            if detail.starts_with(crate::operations::ADOPT_IDENTITY_CONFLICT)
                || detail.starts_with(crate::operations::ADOPT_MARKER_CONFLICT) =>
        {
            (ErrorCode::IdentityConflict, detail.clone())
        }
        OpError::NameConflict(detail)
            if detail.starts_with(crate::operations::ADOPT_UNRENDERABLE_NAME) =>
        {
            (ErrorCode::InvalidName, detail.clone())
        }
        OpError::NotFound(detail) => (ErrorCode::NotFound, detail.clone()),
        OpError::NameConflict(detail) => (ErrorCode::NameConflict, detail.clone()),
        OpError::Indeterminate(detail) => (ErrorCode::ProviderUnavailable, detail.clone()),
        OpError::Refused(detail) => (ErrorCode::OperationInProgress, detail.clone()),
        OpError::StaleRef(detail) => (ErrorCode::BackendEpochChanged, detail.clone()),
        OpError::Registry(detail) if detail.contains("registry busy") => {
            (ErrorCode::RegistryBusy, detail.clone())
        }
        other => (ErrorCode::OperationFailed, other.to_string()),
    };
    TypedError::new(code, message)
}
