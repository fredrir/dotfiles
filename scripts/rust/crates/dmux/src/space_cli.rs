//! P8a: the managed-plane Group/Split CLI (plan §7.2). Root-owned glue:
//! parse refs, resolve the Space against the production registry, build the
//! backend provider/scope, and drive the fenced operations in
//! `dmux::operations`. Backend is always inherited from the Space —
//! `--backend` is rejected by construction (the flags do not exist here).
//!
//! Every verb here answers `--format json` with exactly one §16.2 document
//! (case 43) — refusals, not-founds, and confirmations included. That is why
//! nothing below fails with a bare `String`: a caller reading stdout must
//! never have to fall back to parsing stderr to learn what happened.

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;

use dmux::backend::{InventoryScope, Provider, SplitDirection};
use dmux::error::{ErrorCode, TypedError};
use dmux::model::{Backend, BackendInstanceUid, ServerEpoch};
use dmux::operations::{self, GroupNewRequest, OpError, OperationEnv, SplitNewRequest};
use dmux::output::{self, OutputFormat};
use dmux::refs::{ChildRefShape, ParsedRef, SpaceRefShape, parse_ref};
use dmux::registry::{Registry, RegistryConfig};

#[derive(Subcommand)]
pub enum GroupCmd {
    /// List the Groups of a Space
    Ls {
        /// Space ref; defaults to the Space this pane belongs to
        space: Option<String>,

        /// Deprecated: bare hierarchy (use --format json)
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

        /// Deprecated: bare group object (use --format json)
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

        /// Deprecated: bare preview/results (use --format json)
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

    /// Preview and resolve the journal rows a crashed holder stranded
    /// (plan §10.2/§10.3, cases 11/13/39). Each row is routed through the
    /// frozen `registry::reconcile` decision table; a row a live process
    /// still owns is listed and left alone.
    Reconcile {
        /// Restrict to these Space refs (default: every stranded row)
        spaces: Vec<String>,

        /// Apply without asking
        #[arg(short, long)]
        yes: bool,

        /// Test seam: directory holding registry.sqlite3.
        #[arg(long, hide = true)]
        data_dir: Option<String>,

        /// Test seam: kernel-lock directory.
        #[arg(long, hide = true)]
        lock_dir: Option<String>,
    },
}

pub fn repair(cmd: RepairCmd, format: Option<OutputFormat>) -> ExitCode {
    let action = repair_action(&cmd);
    match repair_cmd(cmd, format) {
        Ok(code) => code,
        Err(error) => refuse(action, format, &error, None),
    }
}

fn repair_action(cmd: &RepairCmd) -> &'static str {
    match cmd {
        RepairCmd::Normalize { .. } => "repair_normalize",
        RepairCmd::Reconcile { .. } => "repair_reconcile",
    }
}

fn repair_cmd(cmd: RepairCmd, format: Option<OutputFormat>) -> Result<ExitCode, TypedError> {
    const ACTION: &str = "repair_normalize";
    match cmd {
        RepairCmd::Reconcile {
            spaces,
            yes,
            data_dir,
            lock_dir,
        } => reconcile_cmd(spaces, yes, data_dir, lock_dir, format),
        RepairCmd::Normalize {
            tokens,
            yes,
            json,
            data_dir,
            lock_dir,
            socket,
        } => {
            let envelope = format == Some(OutputFormat::Json);
            if json {
                eprintln!("{}", crate::JSON_FLAG_HINT);
            }
            let env = match (data_dir, lock_dir) {
                (Some(data), Some(lock)) => OperationEnv {
                    db_path: std::path::PathBuf::from(data).join("registry.sqlite3"),
                    lock_dir: std::path::PathBuf::from(lock),
                },
                _ => OperationEnv::production().map_err(runtime_error)?,
            };
            // Past this point the env is known, so every refusal can stamp
            // the head of the registry the command was actually pointed at.
            let refused = |error: TypedError| refuse(ACTION, format, &error, Some(&env));
            let (socket, expected_epoch) = match socket {
                Some(socket) => (socket, None),
                None => match verified_wez_target(&env, None) {
                    Ok((socket, epoch)) => (socket, Some(epoch)),
                    Err(error) => return Ok(refused(error)),
                },
            };
            let (bin, config) = production_wez_paths();
            let provider = dmux::backend::wez::WezProvider::new(&bin, config);
            let scope = match expected_epoch {
                Some(epoch) => InventoryScope::managed(Backend::Wez, socket, epoch),
                None => InventoryScope::unmanaged_endpoint(Backend::Wez, socket),
            };

            let mut targets = match operations::repair_scan_wez(&env, &provider, &scope) {
                Ok(targets) => targets,
                Err(e) => return Ok(refused(typed_op(&e))),
            };
            if !tokens.is_empty() {
                for wanted in &tokens {
                    if !targets.iter().any(|t| t.native_token == *wanted) {
                        let mut error = TypedError::new(
                            ErrorCode::NotFound,
                            format!("{wanted:?} is not a multi-window wez resource"),
                        );
                        error.target = Some(wanted.clone());
                        return Ok(refused(error));
                    }
                }
                targets.retain(|t| tokens.contains(&t.native_token));
            }
            if targets.is_empty() {
                if envelope {
                    emit_document(ACTION, json!({ "targets": [] }), authority_revision(&env));
                } else if json {
                    println!("{{\"targets\":[]}}");
                } else {
                    println!("nothing to normalize");
                }
                return Ok(ExitCode::SUCCESS);
            }

            // §7.4/§16.2: JSON destructive commands never prompt and emit
            // exactly ONE document — the preview travels inside it, which is
            // why this confirmation carries a result the ADR 008 example
            // leaves null.
            if envelope && !yes {
                let (mut document, exit) = output::confirmation_required(
                    ACTION,
                    &target_list(&targets),
                    authority_revision(&env),
                );
                document["result"] = json!({ "targets": targets });
                println!("{document}");
                return Ok(ExitCode::from(exit.code()));
            }
            // The deprecated flag keeps its own pre-§16.2 refusal payload for
            // the same release its listing payloads keep theirs.
            if json && !yes {
                println!(
                    "{}",
                    json!({ "confirmation_required": true, "targets": targets })
                );
                return Ok(ExitCode::from(5));
            }
            if !envelope && !json {
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
                if let Err(code) = confirm(
                    ACTION,
                    format,
                    &target_list(&targets),
                    &format!("Normalize {} resource(s)?", targets.len()),
                    yes,
                    Some(&env),
                ) {
                    return Ok(code);
                }
            }

            let results = operations::repair_normalize_batch(&env, &provider, &scope, &targets);
            let all_ok = results.iter().all(|r| r.ok);
            if envelope {
                // A quarantined target is a typed error beside a real result,
                // which is what makes the mixed batch the §16.3 partial (7).
                let errors: Vec<TypedError> = results
                    .iter()
                    .filter(|r| !r.ok)
                    .map(|r| {
                        let mut error =
                            TypedError::new(ErrorCode::OperationFailed, r.outcome.clone());
                        error.target = Some(r.native_token.clone());
                        error
                    })
                    .collect();
                println!(
                    "{}",
                    output::document(
                        ACTION,
                        all_ok,
                        json!({ "targets": targets, "results": results }),
                        &errors,
                        authority_revision(&env),
                    )
                );
                return Ok(ExitCode::from(
                    output::document_exit(all_ok, true, &errors).code(),
                ));
            }
            if json {
                println!("{}", json!({ "targets": targets, "results": results }));
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

/// `dmux repair reconcile`: the operator verb for the journal rows a crashed
/// holder stranded. Before this existed a process killed between
/// `reserve_space_kind` and `abort_create` burned its logical name forever —
/// `rm` said `operation_in_progress`, `rename` said `repair_required`, and a
/// replayed `adopt` said `name_conflict`, with no verb able to reap the row
/// (plan cases 11, 13, 39).
///
/// Preview first, always; then one §16.2 document on every branch — nothing
/// to do, refused, declined, applied, or partially applied.
fn reconcile_cmd(
    spaces: Vec<String>,
    yes: bool,
    data_dir: Option<String>,
    lock_dir: Option<String>,
    format: Option<OutputFormat>,
) -> Result<ExitCode, TypedError> {
    const ACTION: &str = "repair_reconcile";
    let envelope = format == Some(OutputFormat::Json);
    let env = match (data_dir, lock_dir) {
        (Some(data), Some(lock)) => OperationEnv {
            db_path: std::path::PathBuf::from(data).join("registry.sqlite3"),
            lock_dir: std::path::PathBuf::from(lock),
        },
        _ => OperationEnv::production().map_err(runtime_error)?,
    };
    let refused = |error: TypedError| refuse(ACTION, format, &error, Some(&env));

    let mut targets = match operations::reconcile_scan(&env) {
        Ok(targets) => targets,
        Err(e) => return Ok(refused(typed_op(&e))),
    };
    if !spaces.is_empty()
        && let Err(error) = reconcile_filter(&mut targets, &spaces)
    {
        return Ok(refused(error));
    }
    if targets.is_empty() {
        if envelope {
            emit_document(
                ACTION,
                json!({ "targets": [], "results": [] }),
                authority_revision(&env),
            );
        } else {
            println!("nothing to reconcile");
        }
        return Ok(ExitCode::SUCCESS);
    }

    // §7.4/§16.2: a JSON destructive command never prompts and emits exactly
    // ONE document, so the preview has to travel inside the confirmation.
    if envelope && !yes {
        let (mut document, exit) = output::confirmation_required(
            ACTION,
            &reconcile_target_list(&targets),
            authority_revision(&env),
        );
        // The same shape every other branch of this verb emits — `targets`
        // beside `results` — so one consumer field works on all of them; a
        // declined run simply resolved nothing.
        document["result"] = json!({ "targets": targets, "results": [] });
        println!("{document}");
        return Ok(ExitCode::from(exit.code()));
    }
    if !envelope {
        // Preview before any mutation, including under --yes: the operator
        // sees which rule ran on which row.
        for target in &targets {
            println!(
                "{}\t{}\t{}/{}\t{}\t{}",
                target.space_no,
                target.logical_name,
                target.kind,
                target.state,
                target.duty,
                if target.in_flight {
                    "in flight"
                } else {
                    "crashed"
                },
            );
        }
        if let Err(code) = confirm(
            ACTION,
            format,
            &reconcile_target_list(&targets),
            &format!("Reconcile {} stranded operation(s)?", targets.len()),
            yes,
            Some(&env),
        ) {
            return Ok(code);
        }
    }

    let results: Vec<operations::ReconcileResult> = targets
        .iter()
        .map(|target| {
            let backend = reconcile_provider(&env, target);
            operations::reconcile_apply(&env, target, backend.as_ref().map(ReconcileNative::lend))
        })
        .collect();
    let all_ok = results.iter().all(|result| result.ok);
    if envelope {
        // An unresolved row is a typed error beside the rows that did
        // resolve — the §16.3 partial (7), never a resultless failure.
        let errors: Vec<TypedError> = results
            .iter()
            .filter(|result| !result.ok)
            .map(|result| {
                let mut error = TypedError::new(
                    reconcile_error_code(result.outcome),
                    format!("{}: {}", result.logical_name, result.detail),
                );
                error.target = Some(result.space_uid.0.to_string());
                error
            })
            .collect();
        println!(
            "{}",
            output::document(
                ACTION,
                all_ok,
                json!({ "targets": targets, "results": results }),
                &errors,
                authority_revision(&env),
            )
        );
        return Ok(ExitCode::from(
            output::document_exit(all_ok, true, &errors).code(),
        ));
    }
    for result in &results {
        println!(
            "{}\t{}\t{}",
            result.logical_name,
            result.outcome.as_str(),
            result.detail
        );
    }
    Ok(if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(7)
    })
}

/// Neither a skip nor a fail-closed is an internal failure: one says a live
/// process owns the row, the other says the decision table refused to guess.
/// Both are §16.3 conflicts (exit 4) inside a partial document.
fn reconcile_error_code(outcome: operations::ReconcileOutcome) -> ErrorCode {
    match outcome {
        operations::ReconcileOutcome::SkippedInFlight => ErrorCode::OperationInProgress,
        _ => ErrorCode::RepairRequired,
    }
}

fn reconcile_target_list(targets: &[operations::ReconcileTarget]) -> String {
    targets
        .iter()
        .map(|target| target.logical_name.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Restrict the pass to the named Spaces. A ref that matches no stranded row
/// is not-found (3), never a silent empty run — the operator asked about a
/// specific Space and deserves to be told it is not stranded.
fn reconcile_filter(
    targets: &mut Vec<operations::ReconcileTarget>,
    refs: &[String],
) -> Result<(), TypedError> {
    let mut wanted = Vec::new();
    for spelling in refs {
        let parsed = parse_ref(spelling).map_err(|e| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("invalid ref {spelling:?}: {e:?}"),
            )
        })?;
        if !targets
            .iter()
            .any(|target| reconcile_ref_matches(&parsed.space, target))
        {
            let mut error = TypedError::new(
                ErrorCode::NotFound,
                format!("{spelling:?} has no unfinished operation to reconcile"),
            );
            error.target = Some(spelling.clone());
            return Err(error);
        }
        wanted.push(parsed.space);
    }
    targets.retain(|target| {
        wanted
            .iter()
            .any(|shape| reconcile_ref_matches(shape, target))
    });
    Ok(())
}

/// A stranded row is not resolvable yet, so it cannot be matched through
/// `resolve` (which requires an ACTIVE Space) — the ref is compared against
/// the journal's own identity instead. Refs for other hosts match nothing:
/// reconciliation is an owner-local act.
fn reconcile_ref_matches(shape: &SpaceRefShape, target: &operations::ReconcileTarget) -> bool {
    match shape {
        SpaceRefShape::Canonical { space, .. } => target.space_uid == *space,
        SpaceRefShape::Numbered { host: None, no } => target.space_no.get() == no.get(),
        SpaceRefShape::Named { host: None, name } => target.logical_name == *name,
        _ => false,
    }
}

/// The concrete backend behind a stranded row. Kept concrete rather than
/// boxed as `dyn Provider` because a crashed Wez adoption also needs that
/// provider's CAS rename — the compensation `adopt_wez` performs itself — and
/// a `dyn Provider` cannot offer it.
enum ReconcileNative {
    Tmux(
        dmux::backend::tmux::TmuxProvider<dmux::backend::tmux::SystemRunner>,
        InventoryScope,
    ),
    Wez(
        dmux::backend::wez::WezProvider<dmux::backend::wez::SystemRunner>,
        InventoryScope,
    ),
}

impl ReconcileNative {
    fn lend(&self) -> operations::ReconcileBackend<'_> {
        match self {
            ReconcileNative::Tmux(provider, scope) => {
                operations::ReconcileBackend::scan_only(provider, scope)
            }
            ReconcileNative::Wez(provider, scope) => {
                operations::ReconcileBackend::restorable(provider, scope, provider)
            }
        }
    }
}

/// The provider/scope for a stranded Space's backend instance, or `None` when
/// it cannot be reached. A tmux adoption reservation is decidable from the
/// registry alone; every other duty — and every Wez adoption, whose rename may
/// already have landed — fails closed without one, which is the point.
fn reconcile_provider(
    env: &OperationEnv,
    target: &operations::ReconcileTarget,
) -> Option<ReconcileNative> {
    let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).ok()?;
    let info = registry
        .backend_instance_info(target.backend_instance)
        .ok()?;
    match info.backend {
        Backend::Tmux => {
            let namespace = info.socket_path?;
            Some(ReconcileNative::Tmux(
                dmux::backend::tmux::TmuxProvider::new(namespace.clone()),
                // Every managed tmux mutation — `remove` included — is
                // refused without the current epoch, so a scope built
                // without it makes `remove_verify_absence` structurally
                // unreachable. Same source `adopt_cli::owner_scope` uses.
                match registry
                    .backend_server(target.backend_instance)
                    .ok()
                    .and_then(|server| server.server_epoch)
                {
                    Some(epoch) => InventoryScope::managed(Backend::Tmux, namespace, epoch),
                    None => InventoryScope::unmanaged_endpoint(Backend::Tmux, namespace),
                },
            ))
        }
        Backend::Wez => {
            let (socket, epoch) = verified_wez_target(env, Some(target.backend_instance)).ok()?;
            let (bin, config) = production_wez_paths();
            Some(ReconcileNative::Wez(
                dmux::backend::wez::WezProvider::new(&bin, config),
                InventoryScope::managed(Backend::Wez, socket, epoch),
            ))
        }
    }
}

#[derive(Subcommand)]
pub enum HostCmd {
    /// List enrolled hosts and their routes
    Ls {
        /// Deprecated: bare row array (use --format json)
        #[arg(long)]
        json: bool,
    },

    /// Set a host's friendly label
    Label {
        /// Alias, current label, or HostUid. Deliberately not named `host`:
        /// that clap id belongs to the global `-H/--host`, and sharing it
        /// bound every value here to the legacy-host gate as well, which
        /// refused any spelling but `macie`/`archie` before this verb ran.
        #[arg(value_name = "HOST")]
        target: String,

        new_label: String,
    },

    /// Disable a host's routes and tombstone its refs (plan §12.2).
    /// Cannot target the local host; re-enrollment reactivates it.
    Forget {
        /// Alias, current label, or HostUid (see `Label` on the name).
        #[arg(value_name = "HOST")]
        target: String,

        /// Forget without asking
        #[arg(short, long)]
        yes: bool,
    },
}

pub fn host(cmd: HostCmd, format: Option<OutputFormat>) -> ExitCode {
    let action = match &cmd {
        HostCmd::Ls { .. } => "host_list",
        HostCmd::Label { .. } => "host_label",
        HostCmd::Forget { .. } => "host_forget",
    };
    match host_cmd(cmd, format) {
        Ok(code) => code,
        Err(error) => refuse(action, format, &error, None),
    }
}

fn host_cmd(cmd: HostCmd, format: Option<OutputFormat>) -> Result<ExitCode, TypedError> {
    let env = OperationEnv::production().map_err(runtime_error)?;
    match cmd {
        HostCmd::Ls { json } => {
            let envelope = format == Some(OutputFormat::Json);
            if json {
                eprintln!("{}", crate::JSON_FLAG_HINT);
            }
            let listings = dmux::remote::hosts::list(&env)?;
            if envelope || json {
                let doc: Vec<_> = listings
                    .iter()
                    .map(|l| {
                        json!({
                            "host_uid": l.host.host_uid.0.to_string(),
                            "alias": l.host.alias,
                            "label": l.host.label,
                            "lifecycle": l.host.lifecycle.as_str(),
                            "enrolled_at": l.host.enrolled_at,
                            "routes": l.routes.iter().map(|r| json!({
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
                if envelope {
                    emit_document("host_list", Value::Array(doc), authority_revision(&env));
                } else {
                    println!("{}", serde_json::to_string(&doc).map_err(encoding_error)?);
                }
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
        HostCmd::Label { target, new_label } => {
            let row = dmux::remote::hosts::label(&env, &target, &new_label)?;
            Ok(report(
                "host_label",
                format,
                &env,
                host_json(&row),
                Vec::new(),
                || {},
            ))
        }
        HostCmd::Forget { target, yes } => {
            if let Err(code) = confirm(
                "host_forget",
                format,
                &target,
                &format!("Forget host {target:?} (disables its routes)?"),
                yes,
                Some(&env),
            ) {
                return Ok(code);
            }
            let row = dmux::remote::hosts::forget(&env, &target, true)?;
            Ok(report(
                "host_forget",
                format,
                &env,
                host_json(&row),
                Vec::new(),
                || {
                    println!(
                        "forgot {} ({})",
                        row.alias.as_deref().unwrap_or("?"),
                        row.host_uid.0
                    )
                },
            ))
        }
    }
}

fn host_json(row: &dmux::registry::HostRow) -> Value {
    json!({
        "host_uid": row.host_uid.0.to_string(),
        "alias": row.alias,
        "label": row.label,
        "lifecycle": row.lifecycle.as_str(),
    })
}

#[derive(Subcommand)]
pub enum ContextCmd {
    /// Acknowledge this pane's marker for an adopted Space (plan §10.3):
    /// derives the current epoch-qualified refs from the pane environment,
    /// records the stamp, and reports how many panes are still pending.
    Stamp { space: String },
}

pub fn context(cmd: ContextCmd, format: Option<OutputFormat>) -> ExitCode {
    match context_cmd(cmd, format) {
        Ok(code) => code,
        Err(error) => refuse("context_stamp", format, &error, None),
    }
}

fn context_cmd(cmd: ContextCmd, format: Option<OutputFormat>) -> Result<ExitCode, TypedError> {
    match cmd {
        ContextCmd::Stamp { space } => {
            let (target, _) = resolve(&space)?;
            let pane = std::env::var("TMUX_PANE")
                .or_else(|_| std::env::var("WEZTERM_PANE"))
                .map_err(|_| {
                    TypedError::new(
                        ErrorCode::Usage,
                        "neither TMUX_PANE nor WEZTERM_PANE is set",
                    )
                })?;
            let outcome = operations::context_stamp(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
                &pane,
            )
            .map_err(|e| typed_op(&e))?;
            let result = serde_json::to_value(&outcome).map_err(encoding_error)?;
            let line = serde_json::to_string(&outcome).map_err(encoding_error)?;
            Ok(report(
                "context_stamp",
                format,
                &target.env,
                result,
                Vec::new(),
                || println!("{line}"),
            ))
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

/// The envelope's `authority_revision` (plan §16.2): the head of the very
/// registry the command just read. Every caller here is already committed to
/// emitting a document, so an unreadable head stamps 0 rather than replacing
/// the report with a bare error line — and a registry that is not on disk is
/// never created merely to fill the field.
fn authority_revision(env: &OperationEnv) -> u64 {
    if !env.db_path.exists() {
        return 0;
    }
    Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .ok()
        .and_then(|registry| registry.authority_head().ok())
        .map_or(0, |head| head.revision)
}

/// The head a refusal carries when the failure happened before this command
/// resolved an env of its own: the production registry it would have read.
fn refusal_revision(env: Option<&OperationEnv>) -> u64 {
    match env {
        Some(env) => authority_revision(env),
        None => crate::production_authority_revision(),
    }
}

/// One §16.2 document on stdout and nothing else — the whole of what a
/// `--format json` command prints when it succeeded.
fn emit_document(action: &str, result: Value, revision: u64) {
    println!("{}", output::document(action, true, result, &[], revision));
}

/// Case 43: a refusal under `--format json` is a document too, so a caller
/// never has to read stderr to find out why stdout was empty. Human mode
/// keeps the one-line diagnostic and prints nothing on stdout.
fn refuse(
    action: &str,
    format: Option<OutputFormat>,
    error: &TypedError,
    env: Option<&OperationEnv>,
) -> ExitCode {
    if format == Some(OutputFormat::Json) {
        println!(
            "{}",
            output::document(
                action,
                false,
                Value::Null,
                std::slice::from_ref(error),
                refusal_revision(env),
            )
        );
    } else {
        eprintln!("dmux: {}", error.message);
    }
    ExitCode::from(error.code.exit_status().code())
}

/// Report work that already happened. A result that coexists with a typed
/// error — created but not presented — is the §16.2 partial (7), never a
/// resultless failure, and in JSON it is still exactly one document.
fn report(
    action: &str,
    format: Option<OutputFormat>,
    env: &OperationEnv,
    result: Value,
    errors: Vec<TypedError>,
    human: impl FnOnce(),
) -> ExitCode {
    if format == Some(OutputFormat::Json) {
        println!(
            "{}",
            output::document(action, true, result, &errors, authority_revision(env))
        );
    } else {
        human();
        for error in &errors {
            eprintln!("dmux: {}", error.message);
        }
    }
    ExitCode::from(output::document_exit(true, true, &errors).code())
}

/// The `target` of a batch refusal: every token the refused batch covers,
/// since the confirmation is about the batch and not about one of them.
fn target_list(targets: &[operations::RepairTarget]) -> String {
    targets
        .iter()
        .map(|target| target.native_token.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// The typed error an `operations` failure becomes. Same codes the GUI
/// surface answers with (`gui_cli::typed_operation`), so one condition does
/// not get two names depending on which door it came through.
fn typed_op(error: &OpError) -> TypedError {
    let code = match error {
        OpError::NameConflict(_) => ErrorCode::NameConflict,
        OpError::Indeterminate(_) => ErrorCode::ProviderUnavailable,
        OpError::NotFound(_) => ErrorCode::NotFound,
        OpError::Refused(_) => ErrorCode::RepairRequired,
        OpError::StaleRef(_) => ErrorCode::BackendEpochChanged,
        OpError::Registry(detail) if detail.contains("registry busy") => ErrorCode::RegistryBusy,
        OpError::Registry(detail) if detail.contains("unfinished operation") => {
            ErrorCode::OperationInProgress
        }
        OpError::Bootstrap(_) | OpError::Lock(_) | OpError::Provider(_) | OpError::Registry(_) => {
            ErrorCode::OperationFailed
        }
    };
    TypedError::new(code, error.to_string())
}

fn registry_error(error: impl std::fmt::Display) -> TypedError {
    TypedError::new(ErrorCode::OperationFailed, format!("registry: {error}"))
}

fn runtime_error(error: impl std::fmt::Display) -> TypedError {
    TypedError::new(
        ErrorCode::OperationFailed,
        format!("runtime paths: {error}"),
    )
}

fn encoding_error(error: impl std::fmt::Display) -> TypedError {
    TypedError::new(ErrorCode::OperationFailed, format!("encoding: {error}"))
}

/// Re-export kept for the binary's other callers; the resolution itself
/// lives in the lib so the remote owner agent can build a Wez provider.
pub use dmux::runtime::production_wez_paths;

pub(crate) fn verified_wez_target(
    env: &OperationEnv,
    expected_instance: Option<BackendInstanceUid>,
) -> Result<(String, ServerEpoch), TypedError> {
    // A descriptor that does not match, or is not published at all, is the
    // provider being unavailable (§16.3 exit 6), never an internal failure.
    let unavailable = |detail: &str| TypedError::new(ErrorCode::ProviderUnavailable, detail);
    let registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(registry_error)?;
    let instance = match expected_instance {
        Some(instance) => instance,
        None => registry
            .backend_instance_for_backend(Backend::Wez)
            .map_err(registry_error)?
            .ok_or_else(|| unavailable("registry has no managed Wez backend instance"))?,
    };
    let info = registry
        .backend_instance_info(instance)
        .map_err(registry_error)?;
    if info.backend != Backend::Wez {
        return Err(TypedError::new(
            ErrorCode::BackendMismatch,
            "registered backend instance is not Wez",
        ));
    }
    let server = registry.backend_server(instance).map_err(registry_error)?;
    let epoch = server
        .server_epoch
        .ok_or_else(|| unavailable("managed Wez backend has no published server epoch"))?;
    let descriptor = dmux::runtime::read_verified_ready_wez_descriptor_in(
        &env.lock_dir,
        Some(instance.0),
        Some(epoch.0),
    )
    .map_err(runtime_error)?
    .ok_or_else(|| unavailable("managed mux descriptor absent (service not running)"))?;
    if info.socket_path.as_deref() != Some(descriptor.socket.as_str())
        || server.server_pid != Some(i64::from(descriptor.pid))
        || server.server_start_token.as_deref() != Some(descriptor.start_token.as_str())
        || server.socket_dev
            != descriptor
                .socket_dev
                .and_then(|value| i64::try_from(value).ok())
        || server.socket_ino
            != descriptor
                .socket_ino
                .and_then(|value| i64::try_from(value).ok())
    {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            "managed Wez descriptor differs from registry socket/process incarnation",
        ));
    }
    Ok((descriptor.socket, epoch))
}

/// The pane-bootstrap helper is installed beside dmux.
fn helper_bin() -> Result<String, TypedError> {
    let exe = std::env::current_exe().map_err(runtime_error)?;
    let sibling = exe.with_file_name("pane-bootstrap");
    if sibling.exists() {
        return Ok(sibling.display().to_string());
    }
    Ok("pane-bootstrap".to_string())
}

/// Resolve a local Space ref (name, number, or canonical URI) and build its
/// provider/scope. Remote-host tokens arrive with P8b.
fn resolve(space_ref: &str) -> Result<(Target, Option<ChildRefShape>), TypedError> {
    // §16.3 distinguishes what went wrong: a misspelled ref is validation
    // (2), a ref that names nothing is target-not-found (3), and only the
    // registry/provider itself failing is an operation failure (1).
    let not_found = |detail: String| TypedError::new(ErrorCode::NotFound, detail);
    let unsupported = || {
        TypedError::new(
            ErrorCode::Usage,
            "refs for other hosts arrive with P8b".to_string(),
        )
    };
    let env = OperationEnv::production().map_err(runtime_error)?;
    let parsed: ParsedRef = parse_ref(space_ref).map_err(|e| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("invalid ref {space_ref:?}: {e:?}"),
        )
    })?;
    let registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(registry_error)?;
    let identity = registry.identity().map_err(registry_error)?;

    let rows = registry.spaces().map_err(registry_error)?;
    let row = match &parsed.space {
        SpaceRefShape::Canonical { host, space } => {
            if *host != identity.host_uid {
                return Err(unsupported());
            }
            rows.iter()
                .find(|r| r.space_uid == *space)
                .ok_or_else(|| not_found(format!("no Space {}", space.0)))?
        }
        SpaceRefShape::Numbered { host, no } => {
            if host.is_some() {
                return Err(unsupported());
            }
            rows.iter()
                .find(|r| r.space_no == *no && r.lifecycle.occupies_name())
                .ok_or_else(|| not_found(format!("no Space number {no}")))?
        }
        SpaceRefShape::Named { host, name } => {
            if host.is_some() {
                return Err(unsupported());
            }
            let matches: Vec<_> = rows
                .iter()
                .filter(|r| r.logical_name == *name && r.lifecycle.occupies_name())
                .collect();
            match matches.as_slice() {
                [one] => *one,
                [] => return Err(not_found(format!("no Space named {name:?}"))),
                _ => {
                    return Err(TypedError::new(
                        ErrorCode::AmbiguousTarget,
                        format!("name {name:?} is ambiguous across backends; use the Space number"),
                    ));
                }
            }
        }
    };
    if row.lifecycle != dmux::model::Lifecycle::Active {
        let mut error = TypedError::new(
            ErrorCode::SpaceAbsent,
            format!(
                "Space {:?} is {:?}, not active",
                row.logical_name, row.lifecycle
            ),
        );
        error.target = Some(space_ref.to_string());
        return Err(error);
    }

    let info = registry
        .backend_instance_info(row.backend_instance)
        .map_err(registry_error)?;
    let (provider, scope): (Box<dyn Provider>, InventoryScope) = match info.backend {
        Backend::Tmux => {
            let namespace = info.socket_path.ok_or_else(|| {
                TypedError::new(
                    ErrorCode::ProviderUnavailable,
                    "tmux instance has no namespace recorded",
                )
            })?;
            (
                Box::new(dmux::backend::tmux::TmuxProvider::new(namespace.clone())),
                InventoryScope::unmanaged_endpoint(Backend::Tmux, namespace),
            )
        }
        Backend::Wez => {
            let (socket, epoch) = verified_wez_target(&env, Some(row.backend_instance))?;
            let (bin, config) = production_wez_paths();
            (
                Box::new(dmux::backend::wez::WezProvider::new(&bin, config)),
                InventoryScope::managed(Backend::Wez, socket, epoch),
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
fn ambient_space_ref() -> Result<String, TypedError> {
    match (
        std::env::var("DMUX_HOST_UID"),
        std::env::var("DMUX_SPACE_UID"),
    ) {
        (Ok(host), Ok(space)) => Ok(format!("dmux://{host}/spaces/{space}")),
        _ => Err(TypedError::new(
            ErrorCode::Usage,
            "no Space ref given and this pane carries no dmux markers",
        )),
    }
}

/// §11.3: `--dir` validated on the owner host; otherwise the invoking
/// Split's cwd when run inside the target Space.
fn requested_cwd(
    dir: Option<String>,
    target_space: dmux::model::SpaceUid,
) -> Result<Option<String>, TypedError> {
    if let Some(dir) = dir {
        let canonical = std::fs::canonicalize(&dir)
            .map_err(|e| TypedError::new(ErrorCode::Usage, format!("--dir {dir:?}: {e}")))?;
        if !canonical.is_dir() {
            return Err(TypedError::new(
                ErrorCode::Usage,
                format!("--dir {dir:?} is not a directory"),
            ));
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
) -> Result<ChildRefShape, TypedError> {
    let child = child.ok_or_else(|| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("{what} requires a child ref (see `dmux group ls`)"),
        )
    })?;
    if child.kind != kind {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            format!("{what} requires a {kind:?} ref, got {:?}", child.kind),
        ));
    }
    Ok(child)
}

/// Confirmation per §7.4: without --yes, prompt on a TTY; decline and
/// non-TTY both change nothing and exit 5. A JSON command never prompts —
/// it answers with the one `confirmation_required` document instead.
fn confirm(
    action: &str,
    format: Option<OutputFormat>,
    target: &str,
    prompt: &str,
    yes: bool,
    env: Option<&OperationEnv>,
) -> Result<(), ExitCode> {
    if yes {
        return Ok(());
    }
    if format == Some(OutputFormat::Json) {
        let (document, exit) = output::confirmation_required(action, target, refusal_revision(env));
        println!("{document}");
        return Err(ExitCode::from(exit.code()));
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
    Ok(())
}

/// Presentation after a mutation: tmux activates in place; Wez has no
/// trusted GUI bridge before P9. Either way the child exists, so a failure
/// to present is a typed error beside a real result — the §7.4 partial (7),
/// never a resultless failure.
fn present(
    target: &Target,
    handle: &dmux::model::ProviderHandle,
    kind: dmux::model::ChildKind,
    no_connect: bool,
) -> Option<TypedError> {
    if no_connect {
        return None;
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
            result.err().map(|e| {
                TypedError::new(
                    ErrorCode::BridgeUnavailable,
                    format!("created, but activation failed: {e:?}"),
                )
            })
        }
        Backend::Wez => Some(TypedError::new(
            ErrorCode::BridgeUnavailable,
            "created, not presented (no trusted GUI bridge before P9)",
        )),
    }
}

fn parse_child_of(
    target_ref: &str,
) -> Result<(Target, ChildRefShape, dmux::model::ChildKind), TypedError> {
    let (target, child) = resolve(target_ref)?;
    let child = child.ok_or_else(|| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("{target_ref:?} has no child component"),
        )
    })?;
    Ok((target, child.clone(), child.kind))
}

pub fn group(cmd: GroupCmd, format: Option<OutputFormat>) -> ExitCode {
    let action = match &cmd {
        GroupCmd::Ls { .. } => "group_list",
        GroupCmd::New { .. } => "group_new",
        GroupCmd::Rename { .. } => "group_rename",
        GroupCmd::Rm { .. } => "group_rm",
        GroupCmd::Con { .. } => "group_con",
    };
    match group_cmd(cmd, format) {
        Ok(code) => code,
        Err(error) => refuse(action, format, &error, None),
    }
}

fn group_cmd(cmd: GroupCmd, format: Option<OutputFormat>) -> Result<ExitCode, TypedError> {
    match cmd {
        GroupCmd::Ls { space, json } => {
            if json {
                eprintln!("{}", crate::JSON_FLAG_HINT);
            }
            let space_ref = match space {
                Some(space) => space,
                None => ambient_space_ref()?,
            };
            let (target, _) = resolve(&space_ref)?;
            let tree = operations::hierarchy(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
            )
            .map_err(|e| typed_op(&e))?;
            let result = serde_json::to_value(&tree).map_err(encoding_error)?;
            // The deprecated flag prints the serialized tree itself, not a
            // re-encoded `Value`: scripts compare it byte for byte.
            let legacy = serde_json::to_string(&tree).map_err(encoding_error)?;
            Ok(report(
                "group_list",
                format,
                &target.env,
                result,
                Vec::new(),
                || {
                    if json {
                        println!("{legacy}");
                        return;
                    }
                    for group in &tree.groups {
                        println!(
                            "{}\t{}\t{} split{}",
                            group.group_ref,
                            group.title.as_deref().unwrap_or("-"),
                            group.splits.len(),
                            if group.splits.len() == 1 { "" } else { "s" },
                        );
                    }
                },
            ))
        }
        GroupCmd::New {
            space,
            dir,
            no_connect,
            command,
        } => {
            let (target, _) = resolve(&space)?;
            let cwd = requested_cwd(dir, target.space_uid)?;
            let created = operations::group_new(
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
            )
            .map_err(|e| typed_op(&e))?;
            let shape = parse_ref(&format!("x/{}", created.group_ref))
                .ok()
                .and_then(|p| p.child)
                .ok_or_else(|| {
                    TypedError::new(
                        ErrorCode::PostconditionFailed,
                        "internal: unparseable created ref",
                    )
                })?;
            let presented = present(
                &target,
                &shape.handle,
                dmux::model::ChildKind::Group,
                no_connect,
            );
            let line = format!("{}/{}", target.logical_name, created.group_ref);
            Ok(report(
                "group_new",
                format,
                &target.env,
                json!({
                    "space": target.logical_name,
                    "space_uid": target.space_uid.0.to_string(),
                    "group_ref": created.group_ref,
                    "presented": !no_connect && presented.is_none(),
                }),
                presented.into_iter().collect(),
                || println!("{line}"),
            ))
        }
        GroupCmd::Rename { group, new_name } => {
            let (target, child, _) = parse_child_of(&group)?;
            operations::group_rename(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
                &child,
                &new_name,
                Uuid::new_v4(),
            )
            .map_err(|e| typed_op(&e))?;
            Ok(report(
                "group_rename",
                format,
                &target.env,
                json!({
                    "space_uid": target.space_uid.0.to_string(),
                    "group_ref": dmux::refs::child_suffix(&child),
                    "title": new_name,
                }),
                Vec::new(),
                || {},
            ))
        }
        GroupCmd::Rm { groups, yes } => {
            if groups.is_empty() {
                return Err(TypedError::new(ErrorCode::Usage, "no group refs given"));
            }
            if let Err(code) = confirm(
                "group_rm",
                format,
                &groups.join(","),
                &format!("Remove {} group(s)?", groups.len()),
                yes,
                None,
            ) {
                return Ok(code);
            }
            Ok(remove_children(
                "group_rm",
                format,
                &groups,
                dmux::model::ChildKind::Group,
            ))
        }
        GroupCmd::Con { group } => {
            let (target, child, _) = parse_child_of(&group)?;
            let child = require_child(Some(child), dmux::model::ChildKind::Group, "group con")?;
            let presented = present(&target, &child.handle, dmux::model::ChildKind::Group, false);
            Ok(report(
                "group_con",
                format,
                &target.env,
                json!({
                    "space_uid": target.space_uid.0.to_string(),
                    "group_ref": dmux::refs::child_suffix(&child),
                    "presented": presented.is_none(),
                }),
                presented.into_iter().collect(),
                || {},
            ))
        }
    }
}

/// One removal pass over a batch of child refs. Every ref is attempted and
/// reported: a batch that removed something and failed something else is the
/// §16.3 partial (7), not the first error's status, and the refs that did go
/// are named whichever way it ended.
fn remove_children(
    action: &str,
    format: Option<OutputFormat>,
    refs: &[String],
    kind: dmux::model::ChildKind,
) -> ExitCode {
    let mut removed: Vec<Value> = Vec::new();
    let mut errors: Vec<TypedError> = Vec::new();
    let mut env: Option<OperationEnv> = None;
    for child_ref in refs {
        let attempt = parse_child_of(child_ref).and_then(|(target, child, _)| {
            let child = require_child(Some(child), kind, action)?;
            let outcome = match kind {
                dmux::model::ChildKind::Group => operations::group_remove(
                    &target.env,
                    target.provider.as_ref(),
                    &target.scope,
                    target.space_uid,
                    &child,
                    Uuid::new_v4(),
                ),
                dmux::model::ChildKind::Split => operations::split_remove(
                    &target.env,
                    target.provider.as_ref(),
                    &target.scope,
                    target.space_uid,
                    &child,
                    Uuid::new_v4(),
                ),
            };
            outcome.map_err(|e| typed_op(&e))?;
            Ok(target)
        });
        match attempt {
            Ok(target) => {
                removed.push(json!({
                    "space_uid": target.space_uid.0.to_string(),
                    "ref": child_ref,
                }));
                env = Some(target.env);
            }
            Err(mut error) => {
                error.target = Some(child_ref.clone());
                errors.push(error);
            }
        }
    }
    let ok = !removed.is_empty();
    let status = output::document_exit(ok, ok, &errors);
    if format == Some(OutputFormat::Json) {
        println!(
            "{}",
            output::document(
                action,
                ok,
                Value::Array(removed),
                &errors,
                refusal_revision(env.as_ref()),
            )
        );
    } else {
        for error in &errors {
            eprintln!("dmux: {}", error.message);
        }
    }
    ExitCode::from(status.code())
}

pub fn split(cmd: SplitCmd, format: Option<OutputFormat>) -> ExitCode {
    let action = match &cmd {
        SplitCmd::Ls { .. } => "split_list",
        SplitCmd::New { .. } => "split_new",
        SplitCmd::Rm { .. } => "split_rm",
        SplitCmd::Con { .. } => "split_con",
    };
    match split_cmd(cmd, format) {
        Ok(code) => code,
        Err(error) => refuse(action, format, &error, None),
    }
}

fn split_cmd(cmd: SplitCmd, format: Option<OutputFormat>) -> Result<ExitCode, TypedError> {
    match cmd {
        SplitCmd::Ls { group, json } => {
            if json {
                eprintln!("{}", crate::JSON_FLAG_HINT);
            }
            let group_ref = match group {
                Some(group) => group,
                None => {
                    let space = ambient_space_ref()?;
                    let suffix = std::env::var("DMUX_GROUP_REF").map_err(|_| {
                        TypedError::new(
                            ErrorCode::Usage,
                            "no Group ref given and no DMUX_GROUP_REF marker",
                        )
                    })?;
                    format!("{space}/{suffix}")
                }
            };
            let (target, child, _) = parse_child_of(&group_ref)?;
            let tree = operations::hierarchy(
                &target.env,
                target.provider.as_ref(),
                &target.scope,
                target.space_uid,
            )
            .map_err(|e| typed_op(&e))?;
            let group_ref = dmux::refs::child_suffix(&child);
            let Some(listed) = tree.groups.iter().find(|g| g.group_ref == group_ref) else {
                let mut error = TypedError::new(
                    ErrorCode::NotFound,
                    format!("group {group_ref} not in the live tree"),
                );
                error.target = Some(group_ref);
                return Err(error);
            };
            let result = serde_json::to_value(listed).map_err(encoding_error)?;
            let legacy = serde_json::to_string(listed).map_err(encoding_error)?;
            Ok(report(
                "split_list",
                format,
                &target.env,
                result,
                Vec::new(),
                || {
                    if json {
                        println!("{legacy}");
                        return;
                    }
                    for split in &listed.splits {
                        println!(
                            "{}\t{}\t{}",
                            split.split_ref,
                            split.title.as_deref().unwrap_or("-"),
                            split.cwd.as_deref().unwrap_or("-"),
                        );
                    }
                },
            ))
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
            let created = operations::split_new(
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
            )
            .map_err(|e| typed_op(&e))?;
            let shape = parse_ref(&format!("x/{}", created.split_ref))
                .ok()
                .and_then(|p| p.child)
                .ok_or_else(|| {
                    TypedError::new(
                        ErrorCode::PostconditionFailed,
                        "internal: unparseable created ref",
                    )
                })?;
            let presented = present(
                &target,
                &shape.handle,
                dmux::model::ChildKind::Split,
                no_connect,
            );
            let line = format!("{}/{}", target.logical_name, created.split_ref);
            Ok(report(
                "split_new",
                format,
                &target.env,
                json!({
                    "space": target.logical_name,
                    "space_uid": target.space_uid.0.to_string(),
                    "split_ref": created.split_ref,
                    "presented": !no_connect && presented.is_none(),
                }),
                presented.into_iter().collect(),
                || println!("{line}"),
            ))
        }
        SplitCmd::Rm { splits, yes } => {
            if splits.is_empty() {
                return Err(TypedError::new(ErrorCode::Usage, "no split refs given"));
            }
            if let Err(code) = confirm(
                "split_rm",
                format,
                &splits.join(","),
                &format!("Remove {} split(s)?", splits.len()),
                yes,
                None,
            ) {
                return Ok(code);
            }
            Ok(remove_children(
                "split_rm",
                format,
                &splits,
                dmux::model::ChildKind::Split,
            ))
        }
        SplitCmd::Con { split } => {
            let (target, child, _) = parse_child_of(&split)?;
            let child = require_child(Some(child), dmux::model::ChildKind::Split, "split con")?;
            let presented = present(&target, &child.handle, dmux::model::ChildKind::Split, false);
            Ok(report(
                "split_con",
                format,
                &target.env,
                json!({
                    "space_uid": target.space_uid.0.to_string(),
                    "split_ref": dmux::refs::child_suffix(&child),
                    "presented": presented.is_none(),
                }),
                presented.into_iter().collect(),
                || {},
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use dmux::model::ServerEpoch;
    use uuid::Uuid;

    use super::*;

    /// Every managed tmux mutation refuses without the current epoch —
    /// `remove_space_inner` answers `remove requires the current epoch` — so a
    /// reconcile scope built with `expected_epoch: None` made
    /// `remove_verify_absence` structurally unreachable: a crashed `dmux rm`
    /// could only ever fail closed, whatever the backend said.
    #[test]
    fn a_reconcile_scope_carries_the_epoch_every_tmux_mutation_requires() {
        let dir = tempfile::tempdir().unwrap();
        let env = OperationEnv {
            db_path: dir.path().join("registry.sqlite3"),
            lock_dir: dir.path().join("locks"),
        };
        std::fs::create_dir_all(&env.lock_dir).unwrap();
        let epoch = ServerEpoch(Uuid::from_u128(0x5eed));
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-scratch"), None)
            .unwrap();
        registry
            .publish_backend_server(instance, epoch, Some(4242), Some("start"), None, None)
            .unwrap();
        registry
            .reserve_space("stranded", instance, Uuid::new_v4())
            .unwrap();
        drop(registry);

        let targets = operations::reconcile_scan(&env).unwrap();
        match reconcile_provider(&env, &targets[0]).expect("a registered tmux instance is usable") {
            ReconcileNative::Tmux(_, scope) => {
                assert_eq!(scope.expected_epoch(), Some(epoch), "{scope:?}");
                assert_eq!(scope.endpoint, "dmux-scratch");
            }
            ReconcileNative::Wez(..) => panic!("tmux instance resolved as wez"),
        }
    }
}
