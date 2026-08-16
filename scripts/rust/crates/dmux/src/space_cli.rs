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

/// Owner-side Wez CLI selection for managed-mux operations. Provisioning
/// owns these paths; revisited at P9/P11 when the GUI bridge lands.
pub fn production_wez_paths() -> (String, String) {
    let bin = ["/opt/homebrew/bin/wezterm", "/usr/bin/wezterm"]
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
        .unwrap_or_else(|| "wezterm".to_string());
    let config = format!(
        "{}/dotfiles/shared/wezterm/mux/dmux-mux.lua",
        std::env::var("HOME").unwrap_or_default()
    );
    (bin, config)
}

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
