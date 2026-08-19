//! `dmux rm` and `dmux rename` with the Wez-first gate on (plan §7.1, §10.2).
//!
//! Both are owner-only mutations: they resolve a stable Space identity first
//! and never create. In JSON mode a destructive verb without `--yes` emits
//! `output::confirmation_required` and changes nothing (plan §7.4).
//!
//! Owned by the P6 mutation agent (plan §19.3).

use std::io::{IsTerminal, Write};

use serde_json::{Value, json};
use uuid::Uuid;

use crate::backend::{InventoryOutcome, InventoryScope, Provider};
use crate::connect_cli::{
    ConnectAuthority, FrozenBinding, FrozenConnectTarget, HostSelector, OwnerConnectQuery,
    OwnerLocator, ProductionConnectAdapter,
};
use crate::error::{ErrorCode, ExitStatus, TypedError};
use crate::inventory::{self, ReconRow};
use crate::model::{Backend, HostUid, Lifecycle, OperationKind, SpaceNo, SpaceUid};
use crate::operations::{OpError, OperationEnv};
use crate::output::{self, OutputFormat};
use crate::refs::{HostToken, SpaceRefShape, parse_ref};
use crate::registry::{HostLifecycle, Registry, RegistryConfig, SpaceRow};
use crate::remote::client::{
    AgentInvocation, DEFAULT_DEADLINE, PeerExpectation, SshInvoker, call_over_routes,
    request_envelope,
};
use crate::remote::protocol::{self, RenamePayload, RenameResult, RmPayload, RmResult, SpacesInfo};

/// True: `remove`/`rename` below resolve, confirm, and mutate for real. The
/// binary consults this so a machine with the canary flag already exported
/// keeps legacy behaviour until the verb lands; it lives here, in the verb's
/// own module, so nobody has to reopen main.rs to land a verb.
pub const IMPLEMENTED: bool = true;

pub struct RmArgs {
    /// `-H/--host`: alias, label, or HostUid; `None` is the local authority.
    pub host: Option<String>,
    pub targets: Vec<String>,
    /// `--name`: the exact-name escape an adopted legacy name needs when it
    /// is shaped like a ref or a subcommand, e.g. `3` or `b1` (plan §7.4).
    pub name: Option<String>,
    /// One-release compatibility escape for the old listing indices; bare
    /// digits are permanent local SpaceNo values instead (plan §17.13).
    pub rows: Vec<u64>,
    pub all: bool,
    pub backend: Option<Backend>,
    /// Legacy `-w`: one tmux window, not a Space.
    pub window: Option<String>,
    pub yes: bool,
}

pub struct RenameArgs {
    pub host: Option<String>,
    /// `dmux rename (SPACE_REF | --name OLD) NEW`: with a selector flag the
    /// grammar has one positional, so clap fills `old` with the new name and
    /// leaves `new_name` empty. Nothing else can tell the two spellings apart.
    pub old: Option<String>,
    pub new_name: Option<String>,
    pub name: Option<String>,
    pub row: Option<u64>,
    pub backend: Option<Backend>,
    pub allow_name_collision: bool,
}

pub fn remove(format: Option<OutputFormat>, args: RmArgs) -> ExitStatus {
    let json = format == Some(OutputFormat::Json);
    match run_remove(json, args) {
        Ok(status) => status,
        Err(error) => report_failure(json, "rm", &[error]),
    }
}

pub fn rename(format: Option<OutputFormat>, args: RenameArgs) -> ExitStatus {
    let json = format == Some(OutputFormat::Json);
    match run_rename(json, args) {
        Ok(status) => status,
        Err(error) => report_failure(json, "rename", &[error]),
    }
}

// ---------------------------------------------------------------------------
// rm

fn run_remove(json: bool, args: RmArgs) -> Result<ExitStatus, TypedError> {
    if args.window.is_some() {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "-w/--window removes one native window, which is a Split: use `dmux split rm`",
        ));
    }
    if !args.all && args.targets.is_empty() && args.rows.is_empty() && args.name.is_none() {
        return Err(TypedError::new(ErrorCode::Usage, "rm needs a target"));
    }
    // §7.4: `--name` exists to escape ref parsing, so it cannot ride along
    // with a spelling that would be parsed or enumerated instead. clap
    // refuses the mix at the command line; a library caller gets the same
    // answer here rather than a silently merged batch.
    if args.name.is_some() && (!args.targets.is_empty() || !args.rows.is_empty() || args.all) {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "--name already selects the Space: drop the ref, --row and --all",
        ));
    }

    // §7.4: a JSON destructive verb never prompts, and a run with no
    // terminal cannot be asked at all. Both answer before any lookup, so a
    // refusal has scanned no backend, opened no route, and — case 41's
    // "changes nothing" — not even created the authority database.
    if !args.yes
        && let Some(status) = refuse_unpromptable(json, "rm", &rm_subject(&args))?
    {
        return Ok(status);
    }

    let mut adapter = ProductionConnectAdapter::production()?;
    let local = adapter.local_host_uid()?;
    let explicit = args
        .host
        .as_deref()
        .map(|spelling| adapter.resolve_host(&host_selector(spelling)))
        .transpose()?;

    // Case 42 wants "preflights all", so a target that fails to parse or
    // names an unknown host joins the report instead of ending it; every
    // failure carries the exact word the caller typed, which is the only
    // thing they can act on in a batch.
    let mut selectors = Vec::new();
    let mut errors = Vec::new();
    for spelling in &args.targets {
        match build_selector(&mut adapter, spelling, explicit, local) {
            Ok(selector) => selectors.push(selector),
            Err(mut error) => {
                error.target.get_or_insert_with(|| spelling.clone());
                errors.push(error);
            }
        }
    }
    let owner = reconcile_owner(explicit, None, local)?;
    // The name is taken literally on exactly this owner: no `parse_ref`, no
    // prefix or fuzzy fallback, no second host consulted. That is the only
    // removal spelling a ref-shaped legacy name has (plan §7.4, §17.10).
    //
    // The spelling keeps the flag, exactly as `--row` does, because it is
    // what every failure and the confirmation subject echo back: `--name 3`
    // and the ref `3` reach different Spaces, so a bare `3` would give two
    // different removals one byte-identical document.
    if let Some(name) = &args.name {
        selectors.push(Selector {
            spelling: format!("--name {name}"),
            owner,
            locator: OwnerLocator::Name(name.clone()),
        });
    }
    for row in &args.rows {
        match resolve_row(owner, *row) {
            Ok(selector) => selectors.push(selector),
            Err(error) => errors.push(error),
        }
    }
    if args.all {
        for (space_no, spelling) in all_spaces(owner, args.backend)? {
            selectors.push(Selector {
                spelling,
                owner,
                locator: OwnerLocator::Number(space_no),
            });
        }
    }
    // "Cross-host bulk removal is forbidden" (plan §7.4): one invocation
    // names one authority, so a mistyped alias cannot sweep the wrong owner.
    require_single_owner(&selectors)?;
    if !errors.is_empty() {
        return Ok(report_failure(json, "rm", &errors));
    }
    if selectors.is_empty() {
        return Ok(finish(json, "rm", Vec::new(), Vec::new()));
    }

    // Case 42: every target is frozen before any of them is mutated, so a
    // ref that does not resolve leaves the whole batch untouched.
    let mut reach = BackendReach::default();
    let mut frozen = Vec::new();
    for selector in &selectors {
        match preflight_removal(&mut adapter, &mut reach, selector, local, args.backend) {
            Ok(removal) => frozen.push(removal),
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        return Ok(report_failure(json, "rm", &errors));
    }
    // Two spellings of one Space are one removal. Killing it twice reports
    // the second attempt as a failure the caller never caused; §16.3 code 0
    // covers the repeat as a documented idempotent no-op.
    let mut seen: Vec<SpaceUid> = Vec::new();
    frozen.retain(|removal| {
        let first = !seen.contains(&removal.target.space_uid);
        if first {
            seen.push(removal.target.space_uid);
        }
        first
    });

    if !args.yes && !confirm_removals(&frozen)? {
        eprintln!("dmux: rm declined; nothing changed");
        return Ok(ExitStatus::ConfirmationRequired);
    }

    let mut results = Vec::new();
    for removal in &frozen {
        match remove_one(local, removal) {
            Ok(()) => results.push(target_json(&removal.target, "removed", true)),
            Err(mut error) => {
                error.target = Some(removal.spelling.clone());
                errors.push(error);
            }
        }
    }
    Ok(finish(json, "rm", results, errors))
}

fn remove_one(local: HostUid, removal: &FrozenRemoval) -> Result<(), TypedError> {
    let target = &removal.target;
    if target.owner == local {
        let env = production_env()?;
        let (provider, scope) = local_backend(target);
        // A `deleting` row is a crashed remove, not a second one to open.
        // Resuming its own journal entry is the only way any CLI verb can
        // finish one; without it the Space, its name and its number stay
        // unusable forever (plan §10.2 remove step 3).
        return match removal.resume {
            Some((request_uid, operation_uid)) => crate::operations::resume_remove_space(
                &env,
                provider.as_ref(),
                &scope,
                target.backend,
                target.space_uid,
                request_uid,
                operation_uid,
            ),
            None => crate::operations::remove_space(
                &env,
                provider.as_ref(),
                &scope,
                target.backend,
                target.space_uid,
                Uuid::new_v4(),
            ),
        }
        .map_err(typed_operation);
    }
    let payload = serde_json::to_value(RmPayload {
        space_uid: target.space_uid,
    })
    .expect("RmPayload serializes");
    let result: RmResult = owner_call(target, protocol::methods::RM, payload)?;
    if result.space_uid != target.space_uid || !result.removed {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "owner rm answered for a different Space or reported no removal",
        ));
    }
    Ok(())
}

/// What the confirmation prompt and the exit-5 JSON document call this
/// removal, taken from the spelling the user typed — never from a lookup.
fn rm_subject(args: &RmArgs) -> String {
    if args.all {
        return match &args.host {
            Some(host) => format!("--all on {host}"),
            None => "--all on this host".to_string(),
        };
    }
    // `--name 3` and the ref `3` name different Spaces, so the flag is part
    // of the spelling everywhere: what the operator confirms here is the
    // same string the failure documents carry as `target`.
    let mut parts: Vec<String> = args.targets.clone();
    parts.extend(args.name.iter().map(|name| format!("--name {name}")));
    parts.extend(args.rows.iter().map(|row| format!("--row {row}")));
    parts.join(", ")
}

// ---------------------------------------------------------------------------
// rename

fn run_rename(json: bool, args: RenameArgs) -> Result<ExitStatus, TypedError> {
    // With `--name`/`--row` the grammar has one positional, so clap parked
    // the NEW name in `old`; a second positional then means both spellings
    // were mixed.
    let selector_flag = args.name.is_some() || args.row.is_some();
    let (selector, new_name) = match (selector_flag, args.old, args.new_name) {
        (true, Some(new_name), None) => (None, new_name),
        (true, _, Some(_)) => {
            return Err(TypedError::new(
                ErrorCode::Usage,
                "--name/--row already select the Space: `dmux rename --name OLD NEW`",
            ));
        }
        (true, None, None) => {
            return Err(TypedError::new(ErrorCode::Usage, "rename needs a new name"));
        }
        (false, Some(old), Some(new_name)) => (Some(old), new_name),
        (false, _, _) => {
            return Err(TypedError::new(
                ErrorCode::Usage,
                "rename takes an existing Space ref and a new name",
            ));
        }
    };
    crate::refs::validate_new_name(&new_name).map_err(|error| {
        TypedError::new(
            ErrorCode::InvalidName,
            format!("invalid new name {new_name:?}: {error:?}"),
        )
    })?;

    let mut adapter = ProductionConnectAdapter::production()?;
    let local = adapter.local_host_uid()?;
    let explicit = args
        .host
        .as_deref()
        .map(|spelling| adapter.resolve_host(&host_selector(spelling)))
        .transpose()?;

    let selector = match (selector, args.name, args.row) {
        (Some(spelling), _, _) => {
            let (embedded, locator) = parse_target(&spelling)?;
            let embedded = embedded
                .map(|token| adapter.resolve_host(&HostSelector::from(&token)))
                .transpose()?;
            Selector {
                owner: reconcile_owner(explicit, embedded, local)?,
                spelling,
                locator,
            }
        }
        (None, Some(name), _) => Selector {
            spelling: name.clone(),
            owner: reconcile_owner(explicit, None, local)?,
            locator: OwnerLocator::Name(name),
        },
        (None, None, Some(row)) => {
            let owner = reconcile_owner(explicit, None, local)?;
            resolve_row(owner, row)?
        }
        (None, None, None) => unreachable!("selector_flag implies --name or --row"),
    };

    // The frozen `rename` payload carries no collision waiver, so the flag
    // cannot be honoured across a route; refuse before resolving anything
    // rather than pretend it was applied.
    if args.allow_name_collision && selector.owner != local {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "--allow-name-collision is owner-local; run it on the owning host",
        ));
    }

    let target = preflight(&mut adapter, &selector, args.backend)?;
    if target.owner == local {
        // §7.4: a managed rename refuses a cross-backend name collision
        // unless the caller acknowledged it. `operations::rename_space`
        // fences only its own backend instance, so the opposite backend is
        // checked here, under the same read the resolver just proved.
        if !args.allow_name_collision {
            reject_opposite_backend_name(&target, &new_name)?;
        }
        let env = production_env()?;
        let (provider, scope) = local_backend(&target);
        crate::operations::rename_space(
            &env,
            provider.as_ref(),
            &scope,
            target.backend,
            target.space_uid,
            &new_name,
            Uuid::new_v4(),
        )
        .map_err(typed_operation)?;
    } else {
        let payload = serde_json::to_value(RenamePayload {
            space_uid: target.space_uid,
            new_name: new_name.clone(),
        })
        .expect("RenamePayload serializes");
        let result: RenameResult = owner_call(&target, protocol::methods::RENAME, payload)?;
        if result.space_uid != target.space_uid || result.name != new_name {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "owner rename answered for a different Space or name",
            ));
        }
    }

    let mut renamed = target_json(&target, "renamed", true);
    renamed["name"] = json!(new_name);
    Ok(finish(json, "rename", vec![renamed], Vec::new()))
}

/// The opposite backend's live exact-name holder, if any (plan §7.4).
fn reject_opposite_backend_name(
    target: &FrozenConnectTarget,
    new_name: &str,
) -> Result<(), TypedError> {
    let env = production_env()?;
    let registry = open_registry(&env)?;
    for space in registry.spaces().map_err(typed_registry)? {
        if space.space_uid == target.space_uid
            || !space.lifecycle.occupies_name()
            || space.logical_name != new_name
        {
            continue;
        }
        let info = registry
            .backend_instance_info(space.backend_instance)
            .map_err(typed_registry)?;
        if info.backend != target.backend {
            return Err(TypedError::new(
                ErrorCode::NameConflict,
                format!(
                    "{new_name:?} is held by the {} Space {}; \
                     pass --allow-name-collision to keep both",
                    info.backend,
                    space.space_no.get()
                ),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Target selection

/// One user-named target after grammar parsing and host reconciliation,
/// still carrying what the caller typed for messages and JSON `target`
/// fields — the flag included, so `--name 3` and the ref `3` never produce
/// the same document for the two different Spaces they select.
struct Selector {
    spelling: String,
    owner: HostUid,
    locator: OwnerLocator,
}

/// Bare digits are a permanent local SpaceNo, never a row index (plan
/// §17.13); the whole §6.2 grammar comes from `refs::parse_ref`.
fn parse_target(spelling: &str) -> Result<(Option<HostToken>, OwnerLocator), TypedError> {
    let parsed = parse_ref(spelling).map_err(|error| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("invalid Space ref {spelling:?}: {error:?}"),
        )
    })?;
    if parsed.child.is_some() {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            format!("{spelling:?} names a Group/Split: use `dmux group rm` or `dmux split rm`"),
        ));
    }
    Ok(match parsed.space {
        SpaceRefShape::Canonical { host, space } => {
            (Some(HostToken::Uid(host)), OwnerLocator::Uid(space))
        }
        SpaceRefShape::Numbered { host, no } => (host, OwnerLocator::Number(no)),
        SpaceRefShape::Named { host, name } => (host, OwnerLocator::Name(name)),
    })
}

fn host_selector(spelling: &str) -> HostSelector {
    match Uuid::parse_str(spelling) {
        Ok(uid) if spelling == uid.to_string() => HostSelector::Uid(HostUid(uid)),
        _ => HostSelector::AliasOrLabel(spelling.to_string()),
    }
}

fn reconcile_owner(
    explicit: Option<HostUid>,
    embedded: Option<HostUid>,
    local: HostUid,
) -> Result<HostUid, TypedError> {
    if let (Some(explicit), Some(embedded)) = (explicit, embedded)
        && explicit != embedded
    {
        return Err(TypedError::new(
            ErrorCode::Usage,
            format!(
                "--host owner {} contradicts reference owner {}",
                explicit.0, embedded.0
            ),
        ));
    }
    Ok(embedded.or(explicit).unwrap_or(local))
}

fn require_single_owner(selectors: &[Selector]) -> Result<(), TypedError> {
    let mut owners: Vec<HostUid> = Vec::new();
    for selector in selectors {
        if !owners.contains(&selector.owner) {
            owners.push(selector.owner);
        }
    }
    match owners.as_slice() {
        [] | [_] => Ok(()),
        [first, second, ..] => Err(TypedError::new(
            ErrorCode::Usage,
            format!(
                "cross-host bulk removal is forbidden: owners {} and {} in one rm",
                first.0, second.0
            ),
        )),
    }
}

fn preflight(
    adapter: &mut ProductionConnectAdapter,
    selector: &Selector,
    backend: Option<Backend>,
) -> Result<FrozenConnectTarget, TypedError> {
    adapter
        .resolve_live(&OwnerConnectQuery {
            owner: selector.owner,
            locator: selector.locator.clone(),
            backend_filter: backend,
            child: None,
        })
        .map_err(|mut error| {
            error.target = Some(selector.spelling.clone());
            error
        })
}

/// One typed target from one spelling. Split out of the batch loop so a
/// parse or host-resolution failure can be collected beside the preflight
/// failures instead of ending the run at the first one.
fn build_selector(
    adapter: &mut ProductionConnectAdapter,
    spelling: &str,
    explicit: Option<HostUid>,
    local: HostUid,
) -> Result<Selector, TypedError> {
    let (embedded, locator) = parse_target(spelling)?;
    let embedded = embedded
        .map(|token| adapter.resolve_host(&HostSelector::from(&token)))
        .transpose()?;
    Ok(Selector {
        spelling: spelling.to_string(),
        owner: reconcile_owner(explicit, embedded, local)?,
        locator,
    })
}

/// One preflighted removal: the frozen target, the spelling that named it,
/// and — when the durable row is already `deleting` — the exact journalled
/// remove to finish rather than a second one to open.
struct FrozenRemoval {
    spelling: String,
    target: FrozenConnectTarget,
    resume: Option<(Uuid, Uuid)>,
}

/// Freeze one removal target. A remote owner still resolves through the
/// connect authority, because the peer re-resolves everything itself. A
/// local one resolves the DURABLE record instead: the connect resolver
/// refuses whatever it cannot present, which makes an externally killed
/// session or a half-deleted row permanently unremovable — and `rm` is the
/// only verb that can tombstone either (plan §14). Nothing is waived by
/// this: the fenced remove re-reads the instance under its own locks,
/// verifies the epoch, and proves native absence before any tombstone.
fn preflight_removal(
    adapter: &mut ProductionConnectAdapter,
    reach: &mut BackendReach,
    selector: &Selector,
    local: HostUid,
    backend: Option<Backend>,
) -> Result<FrozenRemoval, TypedError> {
    if selector.owner != local {
        return Ok(FrozenRemoval {
            spelling: selector.spelling.clone(),
            target: preflight(adapter, selector, backend)?,
            resume: None,
        });
    }
    let attribute = |mut error: TypedError| {
        error.target = Some(selector.spelling.clone());
        error
    };
    let removal = resolve_local_removal(selector, backend).map_err(attribute)?;
    // §14: `--yes` waives confirmation only, never the reachability check.
    // Proving the server answers here also keeps a stopped backend from
    // opening a `deleting` journal it could never close.
    reach.require(&removal.target).map_err(attribute)?;
    Ok(removal)
}

/// One live probe per backend per invocation: a `--all` sweep must not
/// re-probe the same server once per target.
#[derive(Default)]
struct BackendReach {
    verdicts: Vec<(Backend, Result<(), TypedError>)>,
}

impl BackendReach {
    fn require(&mut self, target: &FrozenConnectTarget) -> Result<(), TypedError> {
        if let Some((_, verdict)) = self
            .verdicts
            .iter()
            .find(|(backend, _)| *backend == target.backend)
        {
            return verdict.clone();
        }
        let (provider, scope) = local_backend(target);
        let verdict = classify_reach(target.backend, provider.inventory(&scope));
        self.verdicts.push((target.backend, verdict.clone()));
        verdict
    }
}

/// A remove needs a server that answers under the recorded epoch. Each
/// refusal here states what is actually unavailable — the server, or the
/// scan — instead of claiming the Space could not be proven, which is what
/// a resolver built for `con` says about a Space that is merely gone.
fn classify_reach(backend: Backend, outcome: InventoryOutcome) -> Result<(), TypedError> {
    if let InventoryOutcome::Complete(_) = outcome {
        return Ok(());
    }
    if let Some(detail) = inventory::epoch_changed_detail(&outcome) {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!("the managed {backend} server was replaced: {detail}"),
        ));
    }
    if let InventoryOutcome::ServerStopped { .. } = outcome {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!("the managed {backend} server is stopped; start it, then remove"),
        ));
    }
    Err(TypedError::new(
        ErrorCode::ProviderUnavailable,
        format!(
            "the {backend} inventory is {}: a remove cannot prove native absence",
            outcome_word(&outcome)
        ),
    ))
}

/// The one durable Space a local selector names, and the journal state that
/// decides whether removing it starts or finishes a delete.
fn resolve_local_removal(
    selector: &Selector,
    backend: Option<Backend>,
) -> Result<FrozenRemoval, TypedError> {
    let env = production_env()?;
    let registry = open_registry(&env)?;
    let mut live: Vec<(SpaceRow, Backend)> = Vec::new();
    let mut terminal = false;
    let mut other_backend = false;
    for space in registry.spaces().map_err(typed_registry)? {
        if !locator_matches(&selector.locator, &space) {
            continue;
        }
        if space.lifecycle.is_terminal() {
            terminal = true;
            continue;
        }
        let info = registry
            .backend_instance_info(space.backend_instance)
            .map_err(typed_registry)?;
        // "A backend constraint contradicting a stable ID is an error,
        // never reinterpretation" (plan §7.4).
        if backend.is_some_and(|wanted| wanted != info.backend) {
            other_backend = true;
            continue;
        }
        live.push((space, info.backend));
    }
    let spelling = &selector.spelling;
    let (space, backend) = match live.len() {
        1 => live.remove(0),
        0 if other_backend => {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                format!("--backend contradicts the durable Space {spelling} names"),
            ));
        }
        0 if terminal => {
            return Err(TypedError::new(
                ErrorCode::SpaceDeleted,
                format!("{spelling} is already removed; its ref stays deleted forever"),
            ));
        }
        0 => {
            return Err(TypedError::new(
                ErrorCode::NotFound,
                format!("no Space on this owner matches {spelling}"),
            ));
        }
        _ => {
            return Err(TypedError::new(
                ErrorCode::AmbiguousTarget,
                format!("{spelling} matches more than one live Space; use its stable ref"),
            ));
        }
    };
    let resume = match space.lifecycle {
        Lifecycle::Active => None,
        Lifecycle::Deleting => Some(resume_journal(&registry, space.space_uid)?),
        Lifecycle::Reserved => {
            return Err(TypedError::new(
                ErrorCode::OperationInProgress,
                format!("an unfinished create owns {spelling}"),
            ));
        }
        Lifecycle::Conflict => {
            return Err(TypedError::new(
                ErrorCode::RepairRequired,
                format!("{spelling} is in conflict; repair it before removing it"),
            ));
        }
        Lifecycle::Deleted | Lifecycle::Aborted => unreachable!("terminal rows were skipped"),
    };
    Ok(FrozenRemoval {
        spelling: spelling.clone(),
        target: frozen_local_target(&registry, &space, backend, selector.owner)?,
        resume,
    })
}

fn locator_matches(locator: &OwnerLocator, space: &SpaceRow) -> bool {
    match locator {
        OwnerLocator::Uid(uid) => space.space_uid == *uid,
        OwnerLocator::Number(no) => space.space_no == *no,
        OwnerLocator::Name(name) => space.logical_name == *name,
    }
}

/// The exact unfinished remove behind a `deleting` row. `resume_remove_space`
/// deliberately refuses to turn an arbitrary `deleting` row into authority
/// to kill, so the journal — not the caller — supplies the request UID it
/// checks; a row with no unfinished remove is a repair, not a retry.
fn resume_journal(registry: &Registry, space_uid: SpaceUid) -> Result<(Uuid, Uuid), TypedError> {
    match registry
        .unfinished_operation(space_uid)
        .map_err(typed_registry)?
    {
        Some(op) if op.kind == OperationKind::Remove => Ok((op.request_uid, op.operation_uid)),
        Some(_) => Err(TypedError::new(
            ErrorCode::OperationInProgress,
            "another unfinished operation owns this Space",
        )),
        None => Err(TypedError::new(
            ErrorCode::RepairRequired,
            "this Space is marked deleting with no unfinished remove to resume",
        )),
    }
}

/// The owner's own endpoint/epoch for a durable row. Every value comes from
/// the registry; none is user input.
fn frozen_local_target(
    registry: &Registry,
    space: &SpaceRow,
    backend: Backend,
    owner: HostUid,
) -> Result<FrozenConnectTarget, TypedError> {
    let info = registry
        .backend_instance_info(space.backend_instance)
        .map_err(typed_registry)?;
    let endpoint = info.socket_path.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!("the managed {backend} instance has no recorded endpoint"),
        )
    })?;
    let server_epoch = registry
        .backend_server(space.backend_instance)
        .map_err(typed_registry)?
        .server_epoch
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("the managed {backend} instance has published no server epoch"),
            )
        })?;
    // A record with no current binding has nothing native left to kill;
    // the fenced remove skips the provider entirely and tombstones it.
    let native_token = registry
        .current_binding(space.space_uid)
        .map_err(typed_registry)?
        .map(|binding| binding.native_token)
        .unwrap_or_default();
    Ok(FrozenConnectTarget {
        owner,
        space_uid: space.space_uid,
        space_no: space.space_no,
        logical_name: space.logical_name.clone(),
        backend,
        backend_instance_uid: space.backend_instance,
        server_epoch,
        binding: FrozenBinding {
            native_token,
            endpoint,
        },
        child: None,
    })
}

// ---------------------------------------------------------------------------
// `--row N`: the deprecated listing index, made explicit (plan §17.13)

/// The N-th row of the Wez-first `ls` for one host, 1-based, in the exact
/// listing order (managed by permanent SpaceNo, then unmanaged). Case 44:
/// the resolved stable ref is echoed on stderr so the substitution is never
/// silent, and an unmanaged row refuses rather than inventing an identity.
fn resolve_row(owner: HostUid, row: u64) -> Result<Selector, TypedError> {
    let index = usize::try_from(row)
        .ok()
        .and_then(|row| row.checked_sub(1))
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::Usage,
                format!("--row {row} is out of range; listing rows start at 1"),
            )
        })?;
    let listing = listing_rows(owner)?;
    // The managed prefix IS the durable record, in permanent SpaceNo order,
    // and every non-terminal row appears in it whatever the scans said — so
    // an index inside it cannot move. Past it the listing is whatever the
    // live scans proved, and indexing an unproven tail is exactly the silent
    // retargeting case 44 forbids.
    let listed = match listing.rows.get(index) {
        Some(listed) if index < listing.managed || listing.incomplete.is_none() => listed,
        _ => {
            return Err(match listing.incomplete {
                Some(detail) => TypedError::new(
                    ErrorCode::ProviderUnavailable,
                    format!("--row cannot index an incomplete listing: {detail}"),
                ),
                None => TypedError::new(
                    ErrorCode::NotFound,
                    format!(
                        "--row {row} is past the {} listed row(s)",
                        listing.rows.len()
                    ),
                ),
            });
        }
    };
    let (space_no, name) = match listed {
        ListedRow::Managed { space_no, name } => (*space_no, name.clone()),
        ListedRow::Unmanaged { native_ref, name } => {
            return Err(TypedError::new(
                ErrorCode::RepairRequired,
                format!(
                    "--row {row} is the unmanaged resource {name:?}; \
                     it has no stable ref until `dmux adopt {native_ref}`"
                ),
            ));
        }
    };
    let stable = stable_ref(owner_alias(owner)?.as_deref(), owner, space_no);
    eprintln!(
        "dmux: --row {row} resolved to {stable} ({name}); \
         --row is removed after this release"
    );
    Ok(Selector {
        spelling: format!("--row {row}"),
        owner,
        locator: OwnerLocator::Number(space_no),
    })
}

enum ListedRow {
    Managed { space_no: SpaceNo, name: String },
    Unmanaged { native_ref: String, name: String },
}

/// The listing `--row` counts against, plus how much of it is proven.
struct Listing {
    rows: Vec<ListedRow>,
    /// How many leading rows carry a permanent SpaceNo.
    managed: usize,
    /// Why the unmanaged tail could not be established, when it could not.
    incomplete: Option<String>,
}

/// The listing `--row` counts against, built exactly the way `ls` builds it
/// so the two orderings cannot drift (plan §2.10, §16.1).
fn listing_rows(owner: HostUid) -> Result<Listing, TypedError> {
    let env = production_env()?;
    let registry = open_registry(&env)?;
    if registry.identity().map_err(typed_registry)?.host_uid != owner {
        // A remote owner reports durable Spaces only: `spaces` carries no
        // unmanaged rows, so its listing is the managed rows in SpaceNo
        // order and nothing else.
        let mut spaces: Vec<_> = remote_spaces(owner)?
            .spaces
            .into_iter()
            .filter(|space| !space.lifecycle.is_terminal())
            .collect();
        spaces.sort_by_key(|space| space.space_no);
        let rows = spaces
            .into_iter()
            .map(|space| {
                Ok(ListedRow::Managed {
                    space_no: space_no(space.space_no)?,
                    name: space.name,
                })
            })
            .collect::<Result<Vec<_>, TypedError>>()?;
        let managed = rows.len();
        return Ok(Listing {
            rows,
            managed,
            incomplete: None,
        });
    }

    let mut records = Vec::new();
    let mut backends = Vec::new();
    for space in registry.spaces().map_err(typed_registry)? {
        let binding = registry
            .current_binding(space.space_uid)
            .map_err(typed_registry)?;
        let backend = registry
            .backend_instance_info(space.backend_instance)
            .map_err(typed_registry)?
            .backend;
        backends.push((space.space_uid, backend));
        records.push((space, binding));
    }
    let wez = local_scope(&registry, Backend::Wez)?;
    let tmux = local_scope(&registry, Backend::Tmux)?;
    let (wez_bin, wez_config) = crate::runtime::production_wez_paths();
    let scans = inventory::scan_both(
        move || match &wez {
            Some(scope) => {
                crate::backend::wez::WezProvider::new(&wez_bin, wez_config).inventory(scope)
            }
            None => no_instance(),
        },
        move || match &tmux {
            Some(scope) => {
                crate::backend::tmux::TmuxProvider::new(scope.endpoint.clone()).inventory(scope)
            }
            None => no_instance(),
        },
    );
    let incomplete = (!scans.both_determinate()).then(|| {
        format!(
            "wez {}, tmux {}",
            outcome_word(&scans.wez),
            outcome_word(&scans.tmux)
        )
    });
    let rows = inventory::reconcile(
        &records,
        |space| {
            backends
                .iter()
                .find(|(uid, _)| *uid == space.space_uid)
                .map(|(_, backend)| *backend)
        },
        &scans,
    );
    let managed = rows
        .iter()
        .filter(|row| matches!(row, ReconRow::Managed(_)))
        .count();
    Ok(Listing {
        rows: rows
            .into_iter()
            .map(|row| match row {
                ReconRow::Managed(row) => ListedRow::Managed {
                    space_no: row.space.space_no,
                    name: row.space.logical_name,
                },
                ReconRow::Unmanaged(row) => ListedRow::Unmanaged {
                    native_ref: output::native_ref(row.backend, &row.native_token),
                    name: row.native_name,
                },
            })
            .collect(),
        managed,
        incomplete,
    })
}

/// Every Space on exactly one host, for `rm --all` (plan §7.4). Durable
/// registry membership decides this, not a live scan: an unreachable
/// backend must not silently shrink the sweep. Every non-terminal row is a
/// Space `ls` still shows, so every one is swept — dropping the `reserved`,
/// `deleting` and `conflict` rows made `rm --all` report an unqualified
/// success over a host it had not emptied. One that cannot be removed now
/// fails as a named target and the run reports partial.
fn all_spaces(
    owner: HostUid,
    backend: Option<Backend>,
) -> Result<Vec<(SpaceNo, String)>, TypedError> {
    let env = production_env()?;
    let registry = open_registry(&env)?;
    if registry.identity().map_err(typed_registry)?.host_uid != owner {
        let mut rows = Vec::new();
        for space in remote_spaces(owner)?.spaces {
            if space.lifecycle.is_terminal() || backend.is_some_and(|b| b != space.backend) {
                continue;
            }
            rows.push((space_no(space.space_no)?, space.name));
        }
        rows.sort_by_key(|(no, _)| *no);
        return Ok(rows);
    }
    let mut rows = Vec::new();
    for space in registry.spaces().map_err(typed_registry)? {
        if space.lifecycle.is_terminal() {
            continue;
        }
        let info = registry
            .backend_instance_info(space.backend_instance)
            .map_err(typed_registry)?;
        if backend.is_some_and(|b| b != info.backend) {
            continue;
        }
        rows.push((space.space_no, space.logical_name));
    }
    rows.sort_by_key(|(no, _)| *no);
    Ok(rows)
}

fn space_no(raw: u64) -> Result<SpaceNo, TypedError> {
    std::num::NonZeroU64::new(raw)
        .map(SpaceNo)
        .ok_or_else(|| TypedError::new(ErrorCode::ProtocolMismatch, "owner reported SpaceNo 0"))
}

fn outcome_word(outcome: &InventoryOutcome) -> &'static str {
    match outcome {
        InventoryOutcome::Complete(_) => "complete",
        InventoryOutcome::ServerStopped { .. } => "stopped",
        InventoryOutcome::AuthFailed { .. } => "auth_failed",
        InventoryOutcome::HostKeyIdentityFailed { .. } => "host_key_identity_failed",
        InventoryOutcome::CommandMissing { .. } => "command_missing",
        InventoryOutcome::VersionMismatch { .. } => "version_mismatch",
        InventoryOutcome::ProtocolMismatch { .. } => "protocol_mismatch",
        InventoryOutcome::Malformed { .. } => "malformed",
        InventoryOutcome::Timeout { .. } => "timeout",
        InventoryOutcome::PermissionFailure { .. } => "permission_failure",
        InventoryOutcome::Unreachable { .. } => "unreachable",
    }
}

// ---------------------------------------------------------------------------
// Owner plumbing

fn production_env() -> Result<OperationEnv, TypedError> {
    OperationEnv::production()
        .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
}

fn open_registry(env: &OperationEnv) -> Result<Registry, TypedError> {
    Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))
}

fn typed_registry(error: impl std::fmt::Display) -> TypedError {
    TypedError::new(ErrorCode::OperationFailed, error.to_string())
}

/// The same OpError partition `new_cli` uses. It is duplicated rather than
/// shared because that copy is private to a module this agent does not own.
fn typed_operation(error: OpError) -> TypedError {
    let code = match &error {
        OpError::NameConflict(_) => ErrorCode::NameConflict,
        OpError::Indeterminate(_) => ErrorCode::ProviderUnavailable,
        OpError::NotFound(_) => ErrorCode::NotFound,
        OpError::Refused(_) => ErrorCode::OperationInProgress,
        OpError::StaleRef(_) => ErrorCode::BackendEpochChanged,
        OpError::Registry(detail) if detail.contains("registry busy") => ErrorCode::RegistryBusy,
        // A concurrent rm that won the Space lock has already tombstoned it.
        // "Somebody else removed it" is the idempotent not-found answer
        // (§16.3 code 3), never an internal failure (code 1).
        OpError::Registry(detail)
            if detail.starts_with("live space ") && detail.contains("not found") =>
        {
            ErrorCode::SpaceDeleted
        }
        OpError::Bootstrap(_) | OpError::Lock(_) | OpError::Provider(_) | OpError::Registry(_) => {
            ErrorCode::OperationFailed
        }
    };
    TypedError::new(code, error.to_string())
}

/// The provider/scope pair for a frozen local target. Both values come from
/// the owner's own scan through `FrozenConnectTarget`; none is user input.
fn local_backend(target: &FrozenConnectTarget) -> (Box<dyn Provider>, InventoryScope) {
    let provider: Box<dyn Provider> = match target.backend {
        Backend::Tmux => Box::new(crate::backend::tmux::TmuxProvider::new(
            target.binding.endpoint.clone(),
        )),
        Backend::Wez => {
            let (bin, config) = crate::runtime::production_wez_paths();
            Box::new(crate::backend::wez::WezProvider::new(&bin, config))
        }
    };
    (
        provider,
        InventoryScope {
            backend: target.backend,
            endpoint: target.binding.endpoint.clone(),
            expected_epoch: Some(target.server_epoch),
        },
    )
}

/// The exact endpoint one managed backend instance publishes, or `None`
/// when this owner has never registered that backend.
fn local_scope(
    registry: &Registry,
    backend: Backend,
) -> Result<Option<InventoryScope>, TypedError> {
    let Some(instance) = registry
        .backend_instance_for_backend(backend)
        .map_err(typed_registry)?
    else {
        return Ok(None);
    };
    let info = registry
        .backend_instance_info(instance)
        .map_err(typed_registry)?;
    let Some(endpoint) = info.socket_path else {
        return Ok(None);
    };
    let expected_epoch = registry
        .backend_server(instance)
        .map_err(typed_registry)?
        .server_epoch;
    Ok(Some(InventoryScope {
        backend,
        endpoint,
        expected_epoch,
    }))
}

/// An unregistered backend is not proof of an empty one, so it stays
/// indeterminate and `--row` refuses rather than renumbering (plan §2.10).
fn no_instance() -> InventoryOutcome {
    InventoryOutcome::Unreachable {
        detail: "no managed backend instance is registered".into(),
    }
}

fn owner_alias(owner: HostUid) -> Result<Option<String>, TypedError> {
    let env = production_env()?;
    Ok(open_registry(&env)?
        .hosts()
        .map_err(typed_registry)?
        .into_iter()
        .find(|row| row.host_uid == owner && row.lifecycle == HostLifecycle::Enrolled)
        .and_then(|row| row.alias))
}

/// The compact ref when the owner's alias is known, else the portable
/// owner-qualified number (plan §6.2). A bare number means the local
/// authority, so it is never printed for an owner we cannot name.
fn stable_ref(alias: Option<&str>, owner: HostUid, space_no: SpaceNo) -> String {
    match alias {
        Some(alias) => output::compact_ref(alias, space_no.get()),
        None => format!("{}:{}", owner.0, space_no.get()),
    }
}

fn remote_spaces(owner: HostUid) -> Result<SpacesInfo, TypedError> {
    call_owner(owner, protocol::methods::SPACES, json!({}), None)
}

fn owner_call<T: for<'de> serde::Deserialize<'de>>(
    target: &FrozenConnectTarget,
    method: &str,
    payload: Value,
) -> Result<T, TypedError> {
    call_owner(
        target.owner,
        method,
        payload,
        Some((target.backend_instance_uid, target.server_epoch)),
    )
}

/// One fenced owner mutation over the verified routes. This mirrors
/// `new_cli`'s `remote_call`; it is copied rather than shared because that
/// one is private to a module this agent does not own.
fn call_owner<T: for<'de> serde::Deserialize<'de>>(
    owner: HostUid,
    method: &str,
    payload: Value,
    claimed: Option<(crate::model::BackendInstanceUid, crate::model::ServerEpoch)>,
) -> Result<T, TypedError> {
    let env = production_env()?;
    let mut registry = open_registry(&env)?;
    let identity = registry.identity().map_err(typed_registry)?;
    let head = registry.authority_head().map_err(typed_registry)?;
    let request_uid = Uuid::new_v4();
    let mut request = request_envelope(&identity, &head, method, request_uid, payload);
    if let Some((instance, epoch)) = claimed {
        request.backend_instance_uid = Some(instance);
        request.server_epoch = Some(epoch);
    }
    let mut invocation = AgentInvocation::new(method);
    invocation.remote_bin = "dmux".to_string();
    let outcome = call_over_routes(
        &mut registry,
        &PeerExpectation {
            host_uid: owner,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        &SshInvoker::default(),
        &invocation,
        DEFAULT_DEADLINE,
    )?;
    if outcome.envelope.method != method || outcome.envelope.request_uid != request_uid {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "owner response changed method/request UID",
        ));
    }
    // The owner's replay shortcuts answer without instance/epoch qualifiers
    // (`Reply::plain`), so a present qualifier must match and an absent one
    // is the documented idempotent answer.
    if let Some((instance, epoch)) = claimed
        && (outcome
            .envelope
            .backend_instance_uid
            .is_some_and(|got| got != instance)
            || outcome
                .envelope
                .server_epoch
                .is_some_and(|got| got != epoch))
    {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            "owner response changed the claimed backend instance/epoch",
        ));
    }
    serde_json::from_value(outcome.envelope.payload.clone().ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProtocolMismatch,
            "successful owner response omitted payload",
        )
    })?)
    .map_err(|error| {
        TypedError::new(
            ErrorCode::ProtocolMismatch,
            format!("owner {method} payload: {error}"),
        )
    })
}

// ---------------------------------------------------------------------------
// Confirmation and output

/// §7.4's destructive-verb rule for the two shapes that cannot be asked:
/// JSON emits exactly one `confirmation_required` document and exits 5, and
/// a human without a terminal says so on stderr and exits 5. Both answer
/// before any lookup. `Ok(None)` means a terminal is present, so the caller
/// resolves its targets first and prompts for them by name.
fn refuse_unpromptable(
    json: bool,
    action: &str,
    subject: &str,
) -> Result<Option<ExitStatus>, TypedError> {
    if json {
        let (document, status) = output::confirmation_required(action, subject, revision());
        println!("{document}");
        return Ok(Some(status));
    }
    if !std::io::stdin().is_terminal() {
        eprintln!("dmux: {action} {subject} needs confirmation (re-run with --yes)");
        return Ok(Some(ExitStatus::ConfirmationRequired));
    }
    Ok(None)
}

/// §14's destructive preview: the prompt names what is about to be
/// destroyed. `--row 3` and `--all` say nothing about the Spaces they
/// resolved to, and case 44 is not satisfied by an echo the operator only
/// reads after committing.
fn confirm_removals(frozen: &[FrozenRemoval]) -> Result<bool, TypedError> {
    let alias = frozen
        .first()
        .map(|removal| owner_alias(removal.target.owner))
        .transpose()?
        .flatten();
    for removal in frozen {
        eprintln!(
            "{}",
            removal_preview(
                &removal.target,
                alias.as_deref(),
                child_counts(&removal.target)
            )
        );
    }
    eprint!("Remove {} Space(s)? [y/N] ", frozen.len());
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return Ok(false);
    }
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

/// One preview line, in §14's order: stable ref, name, backend, owner,
/// Group count, Split count.
fn removal_preview(
    target: &FrozenConnectTarget,
    alias: Option<&str>,
    counts: (String, String),
) -> String {
    format!(
        "dmux: remove {}\t{}\t{}\towner={}\tgroups={}\tsplits={}",
        stable_ref(alias, target.owner, target.space_no),
        output::one_line(&target.logical_name),
        target.backend,
        target.owner.0,
        counts.0,
        counts.1,
    )
}

/// Best effort. §14 wants the child counts in the preview, but a Space whose
/// backend cannot be read is still removable, so an unavailable hierarchy
/// prints `?` rather than blocking the prompt.
fn child_counts(target: &FrozenConnectTarget) -> (String, String) {
    let unknown = || ("?".to_string(), "?".to_string());
    let Ok(env) = production_env() else {
        return unknown();
    };
    let (provider, scope) = local_backend(target);
    match crate::operations::hierarchy(&env, provider.as_ref(), &scope, target.space_uid) {
        Ok(tree) => (
            tree.groups.len().to_string(),
            tree.groups
                .iter()
                .map(|group| group.splits.len())
                .sum::<usize>()
                .to_string(),
        ),
        Err(_) => unknown(),
    }
}

/// Best effort: the envelope still has to carry a revision when the
/// authority cannot be opened. A refused destructive verb must not be the
/// thing that allocates this host's identity, so a missing database reports
/// revision 0 instead of being created (case 41, plan §17.2 step 2).
fn revision() -> u64 {
    production_env()
        .ok()
        .filter(|env| env.db_path.exists())
        .and_then(|env| open_registry(&env).ok())
        .and_then(|registry| registry.authority_head().ok())
        .map(|head| head.revision)
        .unwrap_or_default()
}

fn target_json(target: &FrozenConnectTarget, verb: &str, done: bool) -> Value {
    json!({
        "uri": crate::refs::canonical_uri(target.owner, target.space_uid),
        "portable_ref": format!("{}:{}", target.owner.0, target.space_no.get()),
        "space_uid": target.space_uid.0.to_string(),
        "space_no": target.space_no.get(),
        "name": target.logical_name,
        "backend": target.backend.as_str(),
        "owner": target.owner.0.to_string(),
        verb: done,
    })
}

/// Case 42's tail: `ok` means "this document carries a usable result", so a
/// mutation that removed something and failed something else stays `ok` and
/// exits 7, while one where every target failed keeps `ok=false` and the
/// first error's own status. An empty sweep is a documented no-op, not a
/// resultless failure.
fn batch_ok(removed: usize, errors: &[TypedError]) -> bool {
    errors.is_empty() || removed > 0
}

fn finish(json: bool, action: &str, results: Vec<Value>, errors: Vec<TypedError>) -> ExitStatus {
    let ok = batch_ok(results.len(), &errors);
    let status = output::document_exit(ok, !results.is_empty(), &errors);
    if json {
        println!(
            "{}",
            output::document(action, ok, Value::Array(results), &errors, revision())
        );
        return status;
    }
    for entry in &results {
        println!(
            "{}\t{}\t{}",
            entry["space_no"],
            entry["name"].as_str().unwrap_or("-"),
            entry["backend"].as_str().unwrap_or("-"),
        );
    }
    for error in &errors {
        match &error.target {
            Some(target) => eprintln!("dmux: {target}: {}", error.message),
            None => eprintln!("dmux: {}", error.message),
        }
    }
    status
}

fn report_failure(json: bool, action: &str, errors: &[TypedError]) -> ExitStatus {
    if json {
        println!(
            "{}",
            output::document(action, false, Value::Null, errors, revision())
        );
    } else {
        for error in errors {
            eprintln!("dmux: {}", error.message);
        }
    }
    output::document_exit(false, false, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(byte: u128) -> HostUid {
        HostUid(Uuid::from_u128(byte))
    }

    fn selector(owner: HostUid) -> Selector {
        Selector {
            spelling: "x".into(),
            owner,
            locator: OwnerLocator::Name("x".into()),
        }
    }

    /// Plan §17.13: under the gate a bare number is a permanent SpaceNo, and
    /// the whole §6.2 grammar keeps working — nothing here is a row index.
    #[test]
    fn targets_parse_as_stable_refs_only() {
        assert!(matches!(
            parse_target("3").unwrap(),
            (None, OwnerLocator::Number(no)) if no.get() == 3
        ));
        assert!(matches!(
            parse_target("b2").unwrap(),
            (Some(HostToken::AliasOrLabel(alias)), OwnerLocator::Number(no))
                if alias == "b" && no.get() == 2
        ));
        assert!(matches!(
            parse_target("project").unwrap(),
            (None, OwnerLocator::Name(name)) if name == "project"
        ));
        let host = Uuid::from_u128(1);
        let space = Uuid::from_u128(2);
        assert!(matches!(
            parse_target(&format!("dmux://{host}/spaces/{space}")).unwrap(),
            (Some(HostToken::Uid(_)), OwnerLocator::Uid(got)) if got.0 == space
        ));
    }

    /// A Group/Split ref is a different verb's target; `rm` must not treat
    /// its Space prefix as the thing to remove (plan §7.2's cascade rule).
    #[test]
    fn a_child_ref_is_refused_rather_than_truncated() {
        let child = format!("demo/g{}.wz-1", Uuid::from_u128(9));
        let error = parse_target(&child).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRef);
        assert!(error.message.contains("dmux split rm"));
    }

    #[test]
    fn a_host_flag_contradicting_the_ref_owner_is_a_usage_error() {
        let error = reconcile_owner(Some(uid(1)), Some(uid(2)), uid(3)).unwrap_err();
        assert_eq!(error.code, ErrorCode::Usage);
        assert_eq!(reconcile_owner(None, None, uid(3)).unwrap(), uid(3));
        assert_eq!(reconcile_owner(Some(uid(1)), None, uid(3)).unwrap(), uid(1));
        assert_eq!(reconcile_owner(None, Some(uid(2)), uid(3)).unwrap(), uid(2));
    }

    /// Plan §7.4: "cross-host bulk removal is forbidden".
    #[test]
    fn one_rm_names_one_authority() {
        assert!(require_single_owner(&[]).is_ok());
        assert!(require_single_owner(&[selector(uid(1)), selector(uid(1))]).is_ok());
        let error = require_single_owner(&[selector(uid(1)), selector(uid(2))]).unwrap_err();
        assert_eq!(error.code, ErrorCode::Usage);
    }

    /// Case 42's exit arithmetic, which is otherwise only reachable with two
    /// live backends: partial is 7, total failure keeps the typed status.
    #[test]
    fn a_partial_batch_is_exit_7_and_a_total_failure_is_not() {
        let failed = vec![TypedError::new(ErrorCode::NotFound, "gone")];
        assert_eq!(
            output::document_exit(batch_ok(2, &[]), true, &[]),
            ExitStatus::Success
        );
        assert_eq!(
            output::document_exit(batch_ok(1, &failed), true, &failed),
            ExitStatus::Partial
        );
        assert_eq!(
            output::document_exit(batch_ok(0, &failed), false, &failed),
            ExitStatus::NotFound
        );
        assert!(batch_ok(0, &[]), "an empty sweep is a documented no-op");
    }

    #[test]
    fn the_confirmation_subject_is_what_the_user_typed() {
        let args = |targets: &[&str], rows: Vec<u64>, all: bool| RmArgs {
            host: None,
            targets: targets.iter().map(|t| t.to_string()).collect(),
            name: None,
            rows,
            all,
            backend: None,
            window: None,
            yes: false,
        };
        assert_eq!(rm_subject(&args(&["a", "b2"], Vec::new(), false)), "a, b2");
        assert_eq!(rm_subject(&args(&[], vec![3], false)), "--row 3");
        assert_eq!(
            rm_subject(&args(&[], Vec::new(), true)),
            "--all on this host"
        );
        // The Space named `3` is not the Space numbered 3: the subject the
        // operator confirms has to say which one was asked for.
        let named = RmArgs {
            name: Some("3".to_string()),
            ..args(&[], Vec::new(), false)
        };
        assert_eq!(rm_subject(&named), "--name 3");
    }

    /// A bare number means the local authority, so an owner whose alias is
    /// unknown gets the portable owner-qualified spelling instead.
    #[test]
    fn an_unaliased_owner_never_borrows_the_bare_number() {
        let no = SpaceNo(std::num::NonZeroU64::new(4).unwrap());
        assert_eq!(stable_ref(Some("a"), uid(1), no), "4");
        assert_eq!(stable_ref(Some("b"), uid(1), no), "b4");
        assert_eq!(
            stable_ref(None, uid(1), no),
            format!("{}:4", Uuid::from_u128(1))
        );
    }
}
