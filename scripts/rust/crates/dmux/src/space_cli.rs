//! P8a: the managed-plane Group/Split CLI (plan §7.2). Root-owned glue:
//! parse refs, resolve the Space against the production registry, build the
//! backend provider/scope, and drive the fenced operations in
//! `dmux::operations`. Backend is always inherited from the Space —
//! `--backend` is rejected by construction (the flags do not exist here).

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Subcommand;
use uuid::Uuid;

use dmux::backend::{InventoryScope, Provider, SplitDirection};
use dmux::model::Backend;
use dmux::operations::{self, GroupNewRequest, OpError, OperationEnv, SplitNewRequest};
use dmux::refs::{ChildRefShape, ParsedRef, SpaceRefShape, parse_ref};
use dmux::registry::{Registry, RegistryConfig};

#[derive(Subcommand)]
pub enum GroupCmd {
    /// List the Groups of a Space
    Ls {
        /// Space ref; defaults to the Space this pane belongs to
        space: Option<String>,

        /// Machine-readable listing
        #[arg(long)]
        json: bool,
    },

    /// Create a new Group in a Space
    New {
        space: String,

        /// Working directory (owner-validated); defaults per plan §11.3
        #[arg(long)]
        dir: Option<String>,

        /// Create without presenting it
        #[arg(long)]
        no_connect: bool,

        /// Command to run in the new Group's first Split
        #[arg(last = true)]
        command: Vec<String>,
    },

    /// Set a Group's title (presentation only)
    Rename { group: String, new_name: String },

    /// Remove Groups (never the last one — use `dmux rm` for the Space)
    Rm {
        groups: Vec<String>,

        /// Remove without asking
        #[arg(short, long)]
        yes: bool,
    },

    /// Present a Group
    Con { group: String },
}

#[derive(Subcommand)]
pub enum SplitCmd {
    /// List the Splits of a Group
    Ls {
        /// Group ref; defaults to the Group this pane belongs to
        group: Option<String>,

        /// Machine-readable listing
        #[arg(long)]
        json: bool,
    },

    /// Create a new Split in a Group
    New {
        group: String,

        /// Placement of the new Split
        #[arg(long, value_parser = ["left", "right", "up", "down"])]
        direction: Option<String>,

        /// New-pane size as a percent of the split axis (1-99)
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=99))]
        percent: Option<u8>,

        /// Working directory (owner-validated); defaults per plan §11.3
        #[arg(long)]
        dir: Option<String>,

        /// Create without presenting it
        #[arg(long)]
        no_connect: bool,

        /// Command to run in the new Split
        #[arg(last = true)]
        command: Vec<String>,
    },

    /// Remove Splits (never the last one — use `dmux group rm`)
    Rm {
        splits: Vec<String>,

        /// Remove without asking
        #[arg(short, long)]
        yes: bool,
    },

    /// Present a Split
    Con { split: String },
}

#[derive(Subcommand)]
pub enum RepairCmd {
    /// Preview and merge multi-window Wez resources to one window each
    /// (plan §10.3): deterministic pane-preserving plans, confirmed before
    /// any mutation; failures stay quarantined per target.
    Normalize {
        /// Restrict to these opaque workspace tokens (default: every
        /// detected multi-window resource)
        tokens: Vec<String>,

        /// Apply without asking
        #[arg(short, long)]
        yes: bool,

        /// Machine-readable preview/results
        #[arg(long)]
        json: bool,

        /// Test seam: directory holding registry.sqlite3.
        #[arg(long, hide = true)]
        data_dir: Option<String>,

        /// Test seam: kernel-lock directory.
        #[arg(long, hide = true)]
        lock_dir: Option<String>,

        /// Test seam: exact wez service socket.
        #[arg(long, hide = true)]
        socket: Option<String>,
    },
}

pub fn repair(cmd: RepairCmd) -> Result<ExitCode, String> {
    match cmd {
        RepairCmd::Normalize {
            tokens,
            yes,
            json,
            data_dir,
            lock_dir,
            socket,
        } => {
            let env = match (data_dir, lock_dir) {
                (Some(data), Some(lock)) => OperationEnv {
                    db_path: std::path::PathBuf::from(data).join("registry.sqlite3"),
                    lock_dir: std::path::PathBuf::from(lock),
                },
                _ => OperationEnv::production().map_err(|e| e.to_string())?,
            };
            let socket = match socket {
                Some(socket) => socket,
                None => {
                    dmux::runtime::read_wez_descriptor()
                        .map_err(|e| e.to_string())?
                        .ok_or("managed mux descriptor absent (service not running)")?
                        .socket
                }
            };
            let (bin, config) = production_wez_paths();
            let provider = dmux::backend::wez::WezProvider::new(&bin, config);
            let scope = InventoryScope {
                backend: Backend::Wez,
                endpoint: socket,
                expected_epoch: None,
            };

            let mut targets = match operations::repair_scan_wez(&env, &provider, &scope) {
                Ok(targets) => targets,
                Err(e) => return fail(e),
            };
            if !tokens.is_empty() {
                for wanted in &tokens {
                    if !targets.iter().any(|t| t.native_token == *wanted) {
                        eprintln!("dmux: {wanted:?} is not a multi-window wez resource");
                        return Ok(ExitCode::from(3));
                    }
                }
                targets.retain(|t| tokens.contains(&t.native_token));
            }
            if targets.is_empty() {
                if json {
                    println!("{{\"targets\":[]}}");
                } else {
                    println!("nothing to normalize");
                }
                return Ok(ExitCode::SUCCESS);
            }

            // §7.4/§16.2: JSON destructive commands never prompt and emit
            // exactly ONE document — the preview travels inside it.
            if json && !yes {
                println!(
                    "{}",
                    serde_json::json!({ "confirmation_required": true, "targets": targets })
                );
                return Ok(ExitCode::from(5));
            }
            if !json {
                // Preview before any mutation (plan §10.3).
                for t in &targets {
                    println!(
                        "{}\t{}\t{} pane move{} into window {}",
                        t.native_token,
                        t.managed
                            .map(|u| u.0.to_string())
                            .unwrap_or_else(|| "unmanaged".into()),
                        t.plan.moves.len(),
                        if t.plan.moves.len() == 1 { "" } else { "s" },
                        t.plan.target_window,
                    );
                }
                match confirm(&format!("Normalize {} resource(s)?", targets.len()), yes) {
                    Ok(_) => {}
                    Err(code) => return Ok(code),
                }
            }

            let results = operations::repair_normalize_batch(&env, &provider, &scope, &targets);
            let all_ok = results.iter().all(|r| r.ok);
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "targets": targets, "results": results })
                );
            } else {
                for r in &results {
                    println!(
                        "{}\t{}",
                        r.native_token,
                        if r.ok {
                            "normalized"
                        } else {
                            r.outcome.as_str()
                        }
                    );
                }
            }
            Ok(if all_ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(7)
            })
        }
    }
}

#[derive(Subcommand)]
pub enum HostCmd {
    /// List enrolled hosts and their routes
    Ls {
        /// Machine-readable listing
        #[arg(long)]
        json: bool,
    },

    /// Set a host's friendly label
    Label { host: String, new_label: String },

    /// Disable a host's routes and tombstone its refs (plan §12.2).
    /// Cannot target the local host; re-enrollment reactivates it.
    Forget {
        host: String,

        /// Forget without asking
        #[arg(short, long)]
        yes: bool,
    },
}

fn typed_fail(err: dmux::error::TypedError) -> Result<ExitCode, String> {
    eprintln!("dmux: {}", err.message);
    Ok(ExitCode::from(err.code.exit_status().code()))
}

pub fn host(cmd: HostCmd) -> Result<ExitCode, String> {
    let env = OperationEnv::production().map_err(|e| e.to_string())?;
    match cmd {
        HostCmd::Ls { json } => {
            let listings = match dmux::remote::hosts::list(&env) {
                Ok(listings) => listings,
                Err(err) => return typed_fail(err),
            };
            if json {
                let doc: Vec<_> = listings
                    .iter()
                    .map(|l| {
                        serde_json::json!({
                            "host_uid": l.host.host_uid.0.to_string(),
                            "alias": l.host.alias,
                            "label": l.host.label,
                            "lifecycle": l.host.lifecycle.as_str(),
                            "enrolled_at": l.host.enrolled_at,
                            "routes": l.routes.iter().map(|r| serde_json::json!({
                                "route_id": r.route_id,
                                "transport": r.transport.as_str(),
                                "endpoint": r.endpoint,
                                "username": r.username,
                                "wez_domain": r.wez_domain,
                                "network_class": r.network_class.as_str(),
                                "priority": r.priority,
                                "required_capability": r.required_capability,
                                "trust_fingerprint": r.trust_fingerprint,
                                "enabled": r.enabled,
                                "last_outcome": r.last_outcome,
                                "last_outcome_at": r.last_outcome_at,
                            })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string(&doc).map_err(|e| e.to_string())?
                );
            } else {
                for l in &listings {
                    println!(
                        "{}\t{}\t{}\t{}",
                        l.host.alias.as_deref().unwrap_or("-"),
                        l.host.label.as_deref().unwrap_or("-"),
                        l.host.lifecycle.as_str(),
                        l.host.host_uid.0,
                    );
                    for r in &l.routes {
                        println!(
                            "  {}\t{}\t{}\tprio {}\t{}\tdomain={}\tuser={}\t{}",
                            r.transport.as_str(),
                            r.endpoint,
                            r.network_class.as_str(),
                            r.priority,
                            if r.enabled { "enabled" } else { "disabled" },
                            r.wez_domain.as_deref().unwrap_or("-"),
                            r.username.as_deref().unwrap_or("-"),
                            r.last_outcome.as_deref().unwrap_or("-"),
                        );
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        HostCmd::Label { host, new_label } => {
            match dmux::remote::hosts::label(&env, &host, &new_label) {
                Ok(_) => Ok(ExitCode::SUCCESS),
                Err(err) => typed_fail(err),
            }
        }
        HostCmd::Forget { host, yes } => {
            match confirm(&format!("Forget host {host:?} (disables its routes)?"), yes) {
                Ok(_) => {}
                Err(code) => return Ok(code),
            }
            match dmux::remote::hosts::forget(&env, &host, true) {
                Ok(row) => {
                    println!(
                        "forgot {} ({})",
                        row.alias.as_deref().unwrap_or("?"),
                        row.host_uid.0
                    );
                    Ok(ExitCode::SUCCESS)
                }
                Err(err) => typed_fail(err),
            }
        }
    }
}

#[derive(Subcommand)]
pub enum ContextCmd {
    /// Acknowledge this pane's marker for an adopted Space (plan §10.3):
    /// derives the current epoch-qualified refs from the pane environment,
    /// records the stamp, and reports how many panes are still pending.
    Stamp { space: String },
}

pub fn context(cmd: ContextCmd) -> Result<ExitCode, String> {
    match cmd {
        ContextCmd::Stamp { space } => {
            let (target, _) = resolve(&space)?;
            let pane = std::env::var("TMUX_PANE")
                .or_else(|_| std::env::var("WEZTERM_PANE"))
                .map_err(|_| "neither TMUX_PANE nor WEZTERM_PANE is set")?;
            let outcome = match operations::context_stamp(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
                &pane,
            ) {
                Ok(outcome) => outcome,
                Err(e) => return fail(e),
            };
            println!(
                "{}",
                serde_json::to_string(&outcome).map_err(|e| e.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// A resolved local Space: registry row plus the provider/scope to reach
/// its backend instance.
struct Target {
    env: OperationEnv,
    provider: Box<dyn Provider>,
    scope: InventoryScope,
    space_uid: dmux::model::SpaceUid,
    logical_name: String,
}

fn op_exit(err: &OpError) -> ExitCode {
    ExitCode::from(match err {
        OpError::NotFound(_) => 3,
        OpError::NameConflict(_) | OpError::Refused(_) | OpError::StaleRef(_) => 4,
        OpError::Indeterminate(_) => 6,
        _ => 1,
    })
}

fn fail(err: OpError) -> Result<ExitCode, String> {
    eprintln!("dmux: {err}");
    Ok(op_exit(&err))
}

/// Re-export kept for the binary's other callers; the resolution itself
/// lives in the lib so the remote owner agent can build a Wez provider.
pub use dmux::runtime::production_wez_paths;

/// The pane-bootstrap helper is installed beside dmux.
fn helper_bin() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sibling = exe.with_file_name("pane-bootstrap");
    if sibling.exists() {
        return Ok(sibling.display().to_string());
    }
    Ok("pane-bootstrap".to_string())
}

/// Resolve a local Space ref (name, number, or canonical URI) and build its
/// provider/scope. Remote-host tokens arrive with P8b.
fn resolve(space_ref: &str) -> Result<(Target, Option<ChildRefShape>), String> {
    let env = OperationEnv::production().map_err(|e| e.to_string())?;
    let parsed: ParsedRef =
        parse_ref(space_ref).map_err(|e| format!("invalid ref {space_ref:?}: {e:?}"))?;
    let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| e.to_string())?;
    let identity = registry.identity().map_err(|e| e.to_string())?;

    let rows = registry.spaces().map_err(|e| e.to_string())?;
    let row = match &parsed.space {
        SpaceRefShape::Canonical { host, space } => {
            if *host != identity.host_uid {
                return Err("refs for other hosts arrive with P8b".into());
            }
            rows.iter()
                .find(|r| r.space_uid == *space)
                .ok_or_else(|| format!("no Space {}", space.0))?
        }
        SpaceRefShape::Numbered { host, no } => {
            if host.is_some() {
                return Err("refs for other hosts arrive with P8b".into());
            }
            rows.iter()
                .find(|r| r.space_no == *no && r.lifecycle.occupies_name())
                .ok_or_else(|| format!("no Space number {no}"))?
        }
        SpaceRefShape::Named { host, name } => {
            if host.is_some() {
                return Err("refs for other hosts arrive with P8b".into());
            }
            let matches: Vec<_> = rows
                .iter()
                .filter(|r| r.logical_name == *name && r.lifecycle.occupies_name())
                .collect();
            match matches.as_slice() {
                [one] => *one,
                [] => return Err(format!("no Space named {name:?}")),
                _ => {
                    return Err(format!(
                        "name {name:?} is ambiguous across backends; use the Space number"
                    ));
                }
            }
        }
    };
    if row.lifecycle != dmux::model::Lifecycle::Active {
        return Err(format!(
            "Space {:?} is {:?}, not active",
            row.logical_name, row.lifecycle
        ));
    }

    let info = registry
        .backend_instance_info(row.backend_instance)
        .map_err(|e| e.to_string())?;
    let (provider, scope): (Box<dyn Provider>, InventoryScope) = match info.backend {
        Backend::Tmux => {
            let namespace = info
                .socket_path
                .ok_or("tmux instance has no namespace recorded")?;
            (
                Box::new(dmux::backend::tmux::TmuxProvider::new(namespace.clone())),
                InventoryScope {
                    backend: Backend::Tmux,
                    endpoint: namespace,
                    expected_epoch: None,
                },
            )
        }
        Backend::Wez => {
            let socket = match info.socket_path {
                Some(socket) => socket,
                None => {
                    dmux::runtime::read_wez_descriptor()
                        .map_err(|e| e.to_string())?
                        .ok_or("managed mux descriptor absent (service not running)")?
                        .socket
                }
            };
            let (bin, config) = production_wez_paths();
            (
                Box::new(dmux::backend::wez::WezProvider::new(&bin, config)),
                InventoryScope {
                    backend: Backend::Wez,
                    endpoint: socket,
                    expected_epoch: None,
                },
            )
        }
    };

    Ok((
        Target {
            env,
            provider,
            scope,
            space_uid: row.space_uid,
            logical_name: row.logical_name.clone(),
        },
        parsed.child,
    ))
}

/// The Space this pane belongs to, from its bootstrap marker environment.
fn ambient_space_ref() -> Result<String, String> {
    match (
        std::env::var("DMUX_HOST_UID"),
        std::env::var("DMUX_SPACE_UID"),
    ) {
        (Ok(host), Ok(space)) => Ok(format!("dmux://{host}/spaces/{space}")),
        _ => Err("no Space ref given and this pane carries no dmux markers".into()),
    }
}

/// §11.3: `--dir` validated on the owner host; otherwise the invoking
/// Split's cwd when run inside the target Space.
fn requested_cwd(
    dir: Option<String>,
    target_space: dmux::model::SpaceUid,
) -> Result<Option<String>, String> {
    if let Some(dir) = dir {
        let canonical = std::fs::canonicalize(&dir).map_err(|e| format!("--dir {dir:?}: {e}"))?;
        if !canonical.is_dir() {
            return Err(format!("--dir {dir:?} is not a directory"));
        }
        return Ok(Some(canonical.display().to_string()));
    }
    let inside = std::env::var("DMUX_SPACE_UID")
        .ok()
        .and_then(|v| v.parse::<Uuid>().ok())
        .is_some_and(|uid| uid == target_space.0);
    if inside && let Ok(cwd) = std::env::current_dir() {
        return Ok(Some(cwd.display().to_string()));
    }
    Ok(None)
}

fn parse_direction(direction: Option<&str>) -> SplitDirection {
    match direction {
        Some("left") => SplitDirection::Left,
        Some("right") => SplitDirection::Right,
        Some("up") => SplitDirection::Up,
        _ => SplitDirection::Down,
    }
}

fn require_child(
    child: Option<ChildRefShape>,
    kind: dmux::model::ChildKind,
    what: &str,
) -> Result<ChildRefShape, String> {
    let child =
        child.ok_or_else(|| format!("{what} requires a child ref (see `dmux group ls`)"))?;
    if child.kind != kind {
        return Err(format!(
            "{what} requires a {kind:?} ref, got {:?}",
            child.kind
        ));
    }
    Ok(child)
}

/// Confirmation per §7.4: without --yes, prompt on a TTY; decline and
/// non-TTY both change nothing and exit 5.
fn confirm(prompt: &str, yes: bool) -> Result<bool, ExitCode> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        eprintln!("dmux: confirmation required (re-run with --yes)");
        return Err(ExitCode::from(5));
    }
    eprint!("{prompt} [y/N] ");
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() || !line.trim().eq_ignore_ascii_case("y") {
        return Err(ExitCode::from(5));
    }
    Ok(true)
}

/// Presentation after a mutation: tmux activates in place; Wez has no
/// trusted GUI bridge before P9, which is the §7.4 partial (exit 7).
fn present(
    target: &Target,
    handle: &dmux::model::ProviderHandle,
    kind: dmux::model::ChildKind,
    no_connect: bool,
) -> ExitCode {
    if no_connect {
        return ExitCode::SUCCESS;
    }
    match target.scope.backend {
        Backend::Tmux => {
            let result = match kind {
                dmux::model::ChildKind::Group => {
                    target.provider.group_activate(&target.scope, handle)
                }
                dmux::model::ChildKind::Split => {
                    target.provider.split_activate(&target.scope, handle)
                }
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("dmux: created, but activation failed: {e:?}");
                    ExitCode::from(7)
                }
            }
        }
        Backend::Wez => {
            eprintln!("dmux: created, not presented (no trusted GUI bridge before P9)");
            ExitCode::from(7)
        }
    }
}

fn parse_child_of(
    target_ref: &str,
) -> Result<(Target, ChildRefShape, dmux::model::ChildKind), String> {
    let (target, child) = resolve(target_ref)?;
    let child = child.ok_or_else(|| format!("{target_ref:?} has no child component"))?;
    Ok((target, child.clone(), child.kind))
}

pub fn group(cmd: GroupCmd) -> Result<ExitCode, String> {
    match cmd {
        GroupCmd::Ls { space, json } => {
            let space_ref = match space {
                Some(space) => space,
                None => ambient_space_ref()?,
            };
            let (target, _) = resolve(&space_ref)?;
            let tree = match operations::hierarchy(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
            ) {
                Ok(tree) => tree,
                Err(e) => return fail(e),
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&tree).map_err(|e| e.to_string())?
                );
            } else {
                for group in &tree.groups {
                    println!(
                        "{}\t{}\t{} split{}",
                        group.group_ref,
                        group.title.as_deref().unwrap_or("-"),
                        group.splits.len(),
                        if group.splits.len() == 1 { "" } else { "s" },
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        GroupCmd::New {
            space,
            dir,
            no_connect,
            command,
        } => {
            let (target, _) = resolve(&space)?;
            let cwd = requested_cwd(dir, target.space_uid)?;
            let created = match operations::group_new(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                &GroupNewRequest {
                    request_uid: Uuid::new_v4(),
                    space_uid: target.space_uid,
                    cwd,
                    program: command,
                    helper_bin: helper_bin()?,
                },
            ) {
                Ok(created) => created,
                Err(e) => return fail(e),
            };
            println!("{}/{}", target.logical_name, created.group_ref);
            let shape = parse_ref(&format!("x/{}", created.group_ref))
                .ok()
                .and_then(|p| p.child)
                .ok_or("internal: unparseable created ref")?;
            Ok(present(
                &target,
                &shape.handle,
                dmux::model::ChildKind::Group,
                no_connect,
            ))
        }
        GroupCmd::Rename { group, new_name } => {
            let (target, child, _) = parse_child_of(&group)?;
            match operations::group_rename(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
                &child,
                &new_name,
                Uuid::new_v4(),
            ) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => fail(e),
            }
        }
        GroupCmd::Rm { groups, yes } => {
            if groups.is_empty() {
                return Err("no group refs given".into());
            }
            match confirm(&format!("Remove {} group(s)?", groups.len()), yes) {
                Ok(_) => {}
                Err(code) => return Ok(code),
            }
            for group_ref in &groups {
                let (target, child, _) = parse_child_of(group_ref)?;
                if let Err(e) = operations::group_remove(
                    &target.env,
                    target.provider.as_ref(),
                    &target.scope,
                    target.space_uid,
                    &child,
                    Uuid::new_v4(),
                ) {
                    return fail(e);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        GroupCmd::Con { group } => {
            let (target, child, _) = parse_child_of(&group)?;
            let child = match require_child(Some(child), dmux::model::ChildKind::Group, "group con")
            {
                Ok(child) => child,
                Err(e) => return Err(e),
            };
            Ok(present(
                &target,
                &child.handle,
                dmux::model::ChildKind::Group,
                false,
            ))
        }
    }
}

pub fn split(cmd: SplitCmd) -> Result<ExitCode, String> {
    match cmd {
        SplitCmd::Ls { group, json } => {
            let group_ref = match group {
                Some(group) => group,
                None => {
                    let space = ambient_space_ref()?;
                    let suffix = std::env::var("DMUX_GROUP_REF")
                        .map_err(|_| "no Group ref given and no DMUX_GROUP_REF marker")?;
                    format!("{space}/{suffix}")
                }
            };
            let (target, child, _) = parse_child_of(&group_ref)?;
            let tree = match operations::hierarchy(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
            ) {
                Ok(tree) => tree,
                Err(e) => return fail(e),
            };
            let group_ref = dmux::refs::child_suffix(&child);
            let Some(listed) = tree.groups.iter().find(|g| g.group_ref == group_ref) else {
                eprintln!("dmux: group {group_ref} not in the live tree");
                return Ok(ExitCode::from(3));
            };
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&listed).map_err(|e| e.to_string())?
                );
            } else {
                for split in &listed.splits {
                    println!(
                        "{}\t{}\t{}",
                        split.split_ref,
                        split.title.as_deref().unwrap_or("-"),
                        split.cwd.as_deref().unwrap_or("-"),
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        SplitCmd::New {
            group,
            direction,
            percent,
            dir,
            no_connect,
            command,
        } => {
            let (target, child, _) = parse_child_of(&group)?;
            let child = require_child(Some(child), dmux::model::ChildKind::Group, "split new")?;
            let cwd = requested_cwd(dir, target.space_uid)?;
            let created = match operations::split_new(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                &SplitNewRequest {
                    request_uid: Uuid::new_v4(),
                    space_uid: target.space_uid,
                    group: child,
                    direction: parse_direction(direction.as_deref()),
                    percent,
                    cwd,
                    program: command,
                    helper_bin: helper_bin()?,
                },
            ) {
                Ok(created) => created,
                Err(e) => return fail(e),
            };
            println!("{}/{}", target.logical_name, created.split_ref);
            let shape = parse_ref(&format!("x/{}", created.split_ref))
                .ok()
                .and_then(|p| p.child)
                .ok_or("internal: unparseable created ref")?;
            Ok(present(
                &target,
                &shape.handle,
                dmux::model::ChildKind::Split,
                no_connect,
            ))
        }
        SplitCmd::Rm { splits, yes } => {
            if splits.is_empty() {
                return Err("no split refs given".into());
            }
            match confirm(&format!("Remove {} split(s)?", splits.len()), yes) {
                Ok(_) => {}
                Err(code) => return Ok(code),
            }
            for split_ref in &splits {
                let (target, child, _) = parse_child_of(split_ref)?;
                let child = require_child(Some(child), dmux::model::ChildKind::Split, "split rm")?;
                if let Err(e) = operations::split_remove(
                    &target.env,
                    target.provider.as_ref(),
                    &target.scope,
                    target.space_uid,
                    &child,
                    Uuid::new_v4(),
                ) {
                    return fail(e);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        SplitCmd::Con { split } => {
            let (target, child, _) = parse_child_of(&split)?;
            let child = require_child(Some(child), dmux::model::ChildKind::Split, "split con")?;
            Ok(present(
                &target,
                &child.handle,
                dmux::model::ChildKind::Split,
                false,
            ))
        }
    }
}
