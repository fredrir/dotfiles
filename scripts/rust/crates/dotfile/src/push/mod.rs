pub mod protocol;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use hostkit::shell::quote as shell_quote;
use hostkit::ssh::Session;

use crate::cli::{Resolution, SyncCli};
use crate::context::Context;
use crate::event::{Action, Event, EventSink, Phase};
use protocol::Message;

const HOSTS_FILE: &str = "config/hosts.dotfile";

use sysinfo::inventory::Host;

#[derive(Debug)]
struct Failure {
    phase: Phase,
    message: String,
    hint: Option<String>,
}

impl Failure {
    fn push(message: impl Into<String>) -> Self {
        Self {
            phase: Phase::Push,
            message: message.into(),
            hint: None,
        }
    }

    fn remote(message: impl Into<String>) -> Self {
        Self {
            phase: Phase::Remote,
            message: message.into(),
            hint: None,
        }
    }
}

#[derive(Debug)]
struct LocalBranch {
    name: String,
    oid: String,
    upstream: String,
    ahead: usize,
}

#[derive(Debug)]
struct RemoteState {
    branch: String,
    changes: Vec<String>,
}

#[derive(Debug)]
pub struct PushPlan {
    host: String,
    directory: String,
    branch: LocalBranch,
}

pub trait DecisionClient: Send + Sync {
    fn discard_remote_changes(&self, host: &str, changes: &[String]) -> Result<bool, String>;

    fn resolve_remote_prompt(
        &self,
        prompt: crate::decision::Prompt,
    ) -> Result<crate::decision::Choice, String> {
        match prompt {
            crate::decision::Prompt::RemoteChanges { host, changes } => {
                if self.discard_remote_changes(&host, &changes)? {
                    Ok(crate::decision::Choice::Discard)
                } else {
                    Ok(crate::decision::Choice::Cancel)
                }
            }
            _ => Err("remote sync requested an interactive merge decision".to_string()),
        }
    }
}

struct RejectDecisions;

impl DecisionClient for RejectDecisions {
    fn discard_remote_changes(&self, _host: &str, _changes: &[String]) -> Result<bool, String> {
        Ok(false)
    }
}

impl DecisionClient for crate::decision::Client {
    fn discard_remote_changes(&self, host: &str, changes: &[String]) -> Result<bool, String> {
        match self.choose(crate::decision::Prompt::RemoteChanges {
            host: host.to_string(),
            changes: changes.to_vec(),
        })? {
            crate::decision::Choice::Discard => Ok(true),
            crate::decision::Choice::Cancel | crate::decision::Choice::Abort => Ok(false),
            choice => Err(format!(
                "the sync interface returned invalid remote choice {choice:?}"
            )),
        }
    }

    fn resolve_remote_prompt(
        &self,
        prompt: crate::decision::Prompt,
    ) -> Result<crate::decision::Choice, String> {
        self.choose(prompt)
    }
}

impl PushPlan {
    pub fn host(&self) -> &str {
        &self.host
    }
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child is available")
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let status = self.child().wait()?;
        self.0 = None;
        Ok(status)
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn run(context: &Context, cli: &SyncCli, events: &dyn EventSink) -> Result<(), String> {
    let plan = match preflight_inner(context, cli) {
        Ok(plan) => plan,
        Err(failure) => return finish(Err(failure), events),
    };
    run_preflighted(context, cli, plan, events)
}

pub fn preflight(context: &Context, cli: &SyncCli) -> Result<PushPlan, String> {
    preflight_inner(context, cli).map_err(|failure| failure.message)
}

pub fn preflight_for_host(
    context: &Context,
    cli: &SyncCli,
    host: String,
    events: &dyn EventSink,
) -> Result<PushPlan, String> {
    preflight_host(context, cli, host, Some(events)).map_err(|failure| failure.message)
}

pub fn run_preflighted(
    context: &Context,
    cli: &SyncCli,
    plan: PushPlan,
    events: &dyn EventSink,
) -> Result<(), String> {
    run_preflighted_with_decisions(context, cli, plan, events, &RejectDecisions)
}

pub fn run_preflighted_with_decisions(
    context: &Context,
    cli: &SyncCli,
    plan: PushPlan,
    events: &dyn EventSink,
    decisions: &dyn DecisionClient,
) -> Result<(), String> {
    run_preflighted_with_decisions_summary(context, cli, plan, events, decisions).map(|_| ())
}

pub fn run_preflighted_with_decisions_summary(
    context: &Context,
    cli: &SyncCli,
    plan: PushPlan,
    events: &dyn EventSink,
    decisions: &dyn DecisionClient,
) -> Result<usize, String> {
    finish(execute(context, cli, plan, events, decisions), events)
}

fn finish<T>(result: Result<T, Failure>, events: &dyn EventSink) -> Result<T, String> {
    match result {
        Ok(value) => Ok(value),
        Err(failure) => {
            events.emit(Event::Failed {
                phase: failure.phase,
                message: failure.message.clone(),
                hint: failure.hint,
            });
            Err(failure.message)
        }
    }
}

pub fn resolve_host(context: &Context, requested: Option<&str>) -> Result<String, String> {
    resolve_host_inner(context, requested).map_err(|failure| failure.message)
}

fn preflight_inner(context: &Context, cli: &SyncCli) -> Result<PushPlan, Failure> {
    active(Phase::Push)?;
    let host = resolve_host_inner(context, cli.to.as_deref())?;
    preflight_host(context, cli, host, None)
}

fn preflight_host(
    context: &Context,
    cli: &SyncCli,
    host: String,
    events: Option<&dyn EventSink>,
) -> Result<PushPlan, Failure> {
    let directory = repo_directory(context);
    if !cli.dry_run {
        if let Some(events) = events {
            events.emit(Event::Progress {
                phase: Phase::Preflight,
                completed: 1,
                total: Some(3),
                label: format!("{host} | refreshing upstream"),
            });
        }
        fetch_upstream(context)?;
    }
    let branch = current_branch(context)?;
    if let Some(events) = events {
        events.emit(Event::Progress {
            phase: Phase::Preflight,
            completed: 3,
            total: Some(3),
            label: format!("{host} | ready"),
        });
    }
    Ok(PushPlan {
        host,
        directory,
        branch,
    })
}

fn execute(
    context: &Context,
    cli: &SyncCli,
    plan: PushPlan,
    events: &dyn EventSink,
    decisions: &dyn DecisionClient,
) -> Result<usize, Failure> {
    let PushPlan {
        host,
        directory,
        branch,
    } = plan;
    events.emit(Event::PhaseStarted {
        phase: Phase::Push,
        total: Some(3),
    });

    if cli.dry_run {
        events.emit(Event::Item {
            action: Action::Push,
            path: PathBuf::from(&host),
            detail: format!(
                "would push '{}', then pull and sync ~/{directory}",
                branch.name
            ),
            changed: false,
        });
        events.emit(Event::Progress {
            phase: Phase::Push,
            completed: 3,
            total: Some(3),
            label: format!("{host}:~/{directory}"),
        });
        return Ok(0);
    }

    active(Phase::Push)?;
    push_branch(context, &branch, events)?;
    events.emit(Event::Progress {
        phase: Phase::Push,
        completed: 1,
        total: Some(3),
        label: branch.upstream.clone(),
    });
    events.emit(Event::PhaseStarted {
        phase: Phase::Remote,
        total: Some(2),
    });

    active(Phase::Remote)?;
    protocol_session(context, &host, &directory, &branch, cli, events, decisions)
}

fn resolve_host_inner(context: &Context, requested: Option<&str>) -> Result<String, Failure> {
    if !executable_exists(context, "ssh") {
        return Err(Failure::push(
            "ssh is not installed, so --push has no way to reach the other machine",
        ));
    }
    let hosts = read_hosts(&context.root.join(HOSTS_FILE))?;
    let known = hosts
        .iter()
        .map(|host| host.name.as_str())
        .collect::<Vec<_>>();
    let local = resolve_local_host(context, &hosts);

    if let Some(requested) = requested.filter(|value| !value.is_empty()) {
        if !known.contains(&requested) {
            return Err(Failure::push(format!(
                "unknown machine '{requested}' ({HOSTS_FILE} knows: {})",
                known.join(", ")
            )));
        }
        if local.as_deref() == Some(requested) {
            return Err(Failure::push(format!("'{requested}' is this machine")));
        }
        return Ok(requested.to_string());
    }

    let Some(local) = local else {
        return Err(Failure::push(
            "cannot tell which machine this is; name the other one: dotfile sync -p --to <host>",
        ));
    };
    let others = known
        .iter()
        .copied()
        .filter(|name| *name != local)
        .collect::<Vec<_>>();
    match others.as_slice() {
        [] => Err(Failure::push(format!(
            "{HOSTS_FILE} lists no machine besides '{local}'"
        ))),
        [peer] => Ok((*peer).to_string()),
        peers => Err(Failure::push(format!(
            "'{local}' has several peers ({}); name one with --to",
            peers.join(", ")
        ))),
    }
}

fn read_hosts(path: &Path) -> Result<Vec<Host>, Failure> {
    let source = fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Failure::push(format!(
                "{HOSTS_FILE} is missing, so --push cannot tell which machines exist"
            ))
        } else {
            Failure::push(format!("cannot read {HOSTS_FILE}: {error}"))
        }
    })?;
    let hosts = sysinfo::inventory::parse_hosts(&source)
        .map_err(|error| Failure::push(format!("{HOSTS_FILE}: {error}")))?;
    if hosts.is_empty() {
        return Err(Failure::push(format!("{HOSTS_FILE} lists no machines")));
    }
    Ok(hosts)
}

fn resolve_local_host(context: &Context, hosts: &[Host]) -> Option<String> {
    let inventory = context.inventory();
    let pinned = sysinfo::inventory::resolve_with(&inventory, hosts, "", &[]);
    let candidate = if pinned.is_empty() {
        sysinfo::inventory::resolve_with(&inventory, hosts, "", &local_hostnames(context))
    } else {
        pinned
    };
    hosts
        .iter()
        .find(|host| host.name == candidate)
        .map(|host| host.name.clone())
}

fn local_hostnames(context: &Context) -> Vec<String> {
    let hostname = context
        .env("HOSTNAME")
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty());
    sysinfo::inventory::local_hostnames_with(hostname.as_deref(), |words| {
        let result = crate::process::output(
            context.command(words[0]).args(&words[1..]),
            hostkit::process::CaptureLimits::default(),
            Duration::from_secs(3),
        )
        .ok()?;
        (result.status.success() && !result.stdout_truncated)
            .then(|| String::from_utf8_lossy(&result.stdout).trim().to_string())
    })
}

fn executable_exists(context: &Context, name: &str) -> bool {
    let Some(path) = context.env("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| is_executable(&directory.join(name)))
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn repo_directory(context: &Context) -> String {
    context
        .root
        .strip_prefix(&context.home)
        .ok()
        .and_then(|path| path.to_str())
        .filter(|path| !path.is_empty())
        .unwrap_or("dotfiles")
        .to_string()
}

fn current_branch(context: &Context) -> Result<LocalBranch, Failure> {
    let status = git(
        context,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=no",
        ],
    )?;
    if !status.status.success() {
        return Err(Failure::push(
            first_line(&status.stderr).unwrap_or("this repository has no branch checked out"),
        ));
    }
    let mut name = None;
    let mut oid = None;
    let mut upstream = None;
    let mut ahead = None;
    let mut behind = None;
    for line in String::from_utf8_lossy(&status.stdout).lines() {
        if let Some(value) = line.strip_prefix("# branch.head ") {
            name = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.oid ") {
            oid = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.upstream ") {
            upstream = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.ab +") {
            let mut counts = value.split_whitespace();
            ahead = counts.next().and_then(|count| count.parse::<usize>().ok());
            behind = counts
                .next()
                .and_then(|count| count.strip_prefix('-'))
                .and_then(|count| count.parse::<usize>().ok());
        }
    }
    let name = name.ok_or_else(|| Failure::push("this repository has no branch checked out"))?;
    if name == "(detached)" {
        return Err(Failure::push(
            "this machine is on a detached HEAD, so there is nothing to push",
        ));
    }
    let oid = oid.ok_or_else(|| Failure::push(format!("git returned no commit for '{name}'")))?;
    let upstream = upstream.ok_or_else(|| {
        Failure::push(format!(
            "'{name}' tracks no remote branch (git push -u origin {name} once)"
        ))
    })?;
    let ahead = ahead.ok_or_else(|| {
        Failure::push(format!(
            "git returned no ahead count for tracked branch {upstream}"
        ))
    })?;
    let behind = behind.ok_or_else(|| {
        Failure::push(format!(
            "git returned no behind count for tracked branch {upstream}"
        ))
    })?;
    if behind > 0 {
        return Err(Failure::push(format!(
            "'{name}' is {behind} commit(s) behind {upstream}; pull with --ff-only or rebase before dotfile sync -p"
        )));
    }
    Ok(LocalBranch {
        name,
        oid,
        upstream,
        ahead,
    })
}

fn git(context: &Context, arguments: &[&str]) -> Result<Output, Failure> {
    let mut command = context.command("git");
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(&context.root)
        .args(arguments);
    captured_output(command, Phase::Push)
        .map_err(|error| Failure::push(format!("cannot run git: {error}")))
}

fn fetch_upstream(context: &Context) -> Result<(), Failure> {
    let output = git(context, &["fetch", "--quiet", "--no-tags"])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Failure::push(format!(
            "cannot refresh the tracked branch: {}",
            first_line(&output.stderr).unwrap_or("git fetch failed")
        )))
    }
}

fn push_branch(
    context: &Context,
    branch: &LocalBranch,
    events: &dyn EventSink,
) -> Result<(), Failure> {
    if branch.ahead == 0 {
        events.emit(Event::Item {
            action: Action::Check,
            path: PathBuf::from(&branch.upstream),
            detail: "no committed changes to push".to_string(),
            changed: false,
        });
    } else {
        events.emit(Event::Item {
            action: Action::Push,
            path: PathBuf::from(&branch.upstream),
            detail: format!("{} committed change(s)", branch.ahead),
            changed: true,
        });
    }
    let output = git(context, &["push"])?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|line| line.contains("rejected") || line.contains("fetch first"))
            .or_else(|| stderr.lines().map(str::trim).find(|line| !line.is_empty()))
            .unwrap_or("git push failed");
        Err(Failure::push(format!(
            "{reason}; fetch and pull --ff-only or rebase before retrying"
        )))
    }
}

fn unsupported_remote(host: &str, directory: &str) -> Failure {
    remote_upgrade_failure(
        host,
        directory,
        format!(
            "remote does not support native push protocol {}",
            protocol::VERSION
        ),
    )
}

fn remote_upgrade_failure(host: &str, directory: &str, reason: String) -> Failure {
    Failure::remote(format!(
        "{host}: {reason}; update ~/{directory} and run ./setup.sh --commands-only on {host}, then retry"
    ))
}

fn protocol_session(
    context: &Context,
    host: &str,
    directory: &str,
    branch: &LocalBranch,
    cli: &SyncCli,
    events: &dyn EventSink,
    decisions: &dyn DecisionClient,
) -> Result<usize, Failure> {
    let local_branch = branch.name.as_str();
    let local_head = branch.oid.as_str();
    active(Phase::Remote)?;
    let script = protocol_script(
        host,
        directory,
        local_head,
        remote_resolution(cli),
        cli.force,
    );
    let mut child = ChildGuard::new(spawn_ssh(context, host, &script)?);
    let mut stdin =
        child.child().stdin.take().ok_or_else(|| {
            Failure::remote(format!("{host}: cannot open the remote control stream"))
        })?;
    let stdout = child
        .child()
        .stdout
        .take()
        .ok_or_else(|| Failure::remote(format!("{host}: cannot read the remote stream")))?;
    let stderr = child
        .child()
        .stderr
        .take()
        .ok_or_else(|| Failure::remote(format!("{host}: cannot read remote errors")))?;
    let stderr_thread = drain(stderr);
    let (lines, stdout_thread) = line_stream(stdout);
    let mut hello = false;
    let mut ready = false;
    let mut state = RemoteState {
        branch: String::new(),
        changes: Vec::new(),
    };

    loop {
        let line = match next_remote_line(&lines, host) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(failure) => {
                drop(stdin);
                terminate_child(child, stderr_thread, stdout_thread);
                return Err(failure);
            }
        };
        match protocol::decode(&line) {
            Ok(Message::Hello {
                version,
                host: remote_host,
            }) => {
                if version != protocol::VERSION {
                    send_decision(&mut stdin, &Message::Cancel)?;
                    drop(stdin);
                    let _ = finish_child(child, stderr_thread, stdout_thread);
                    return Err(remote_upgrade_failure(
                        host,
                        directory,
                        format!(
                            "push protocol {version} is incompatible with {}",
                            protocol::VERSION
                        ),
                    ));
                }
                if remote_host != host {
                    send_decision(&mut stdin, &Message::Cancel)?;
                    drop(stdin);
                    let _ = finish_child(child, stderr_thread, stdout_thread);
                    return Err(Failure::remote(format!(
                        "{host}: remote identified itself as '{remote_host}'"
                    )));
                }
                hello = true;
            }
            Ok(Message::State { branch }) if hello => state.branch = branch,
            Ok(Message::Change { value }) if hello => state.changes.push(value),
            Ok(Message::Ready) if hello => {
                ready = true;
                break;
            }
            Ok(Message::Error {
                operation,
                value,
                code: _,
            }) if hello => {
                drop(stdin);
                let _ = finish_child(child, stderr_thread, stdout_thread);
                return Err(remote_operation_failure(host, &operation, &value));
            }
            Ok(message) if hello => {
                drop(stdin);
                let _ = finish_child(child, stderr_thread, stdout_thread);
                return Err(Failure::remote(format!(
                    "{host}: unexpected push protocol frame {message:?}"
                )));
            }
            Err(error) if hello => {
                drop(stdin);
                let _ = finish_child(child, stderr_thread, stdout_thread);
                return Err(Failure::remote(format!("{host}: {error}")));
            }
            Err(_) | Ok(_) => {
                drop(stdin);
                let _ = finish_child(child, stderr_thread, stdout_thread);
                return Err(unsupported_remote(host, directory));
            }
        }
    }

    if !hello {
        drop(stdin);
        let (success, stderr) = finish_child(child, stderr_thread, stdout_thread)?;
        if !success && let Some(reason) = first_line(stderr.as_bytes()) {
            return Err(Failure::remote(format!("{host}: {reason}")));
        }
        return Err(unsupported_remote(host, directory));
    }
    if !ready {
        drop(stdin);
        let (_, stderr) = finish_child(child, stderr_thread, stdout_thread)?;
        return Err(Failure::remote(format!(
            "{host}: {}",
            first_line(stderr.as_bytes()).unwrap_or("remote session ended before it was ready")
        )));
    }
    if state.branch.is_empty() {
        send_decision(&mut stdin, &Message::Cancel)?;
        drop(stdin);
        let (_, stderr) = finish_child(child, stderr_thread, stdout_thread)?;
        return Err(Failure::remote(format!(
            "{host}: {}",
            first_line(stderr.as_bytes()).unwrap_or("cannot read repository state")
        )));
    }
    if state.branch != local_branch {
        send_decision(&mut stdin, &Message::Cancel)?;
        drop(stdin);
        let _ = finish_child(child, stderr_thread, stdout_thread);
        return Err(branch_mismatch(host, local_branch, &state.branch));
    }
    report_remote_changes(host, &state, cli.force, events);
    let decision = if state.changes.is_empty() {
        Message::Continue
    } else if cli.force
        || decisions
            .discard_remote_changes(host, &state.changes)
            .map_err(Failure::remote)?
    {
        Message::Discard
    } else {
        Message::Cancel
    };
    send_decision(&mut stdin, &decision)?;
    if decision == Message::Cancel {
        drop(stdin);
        let _ = finish_child(child, stderr_thread, stdout_thread);
        return Err(dirty_failure(host));
    }

    let mut completed = false;
    let mut sync_ready = false;
    let mut remote_finished = false;
    let mut remote_changed = 0;
    loop {
        let line = match next_remote_line(&lines, host) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(failure) => {
                drop(stdin);
                terminate_child(child, stderr_thread, stdout_thread);
                return Err(failure);
            }
        };
        match protocol::decode(&line) {
            Ok(Message::Phase { operation }) => {
                let completed_steps = match operation.as_str() {
                    "discard" => 0,
                    "pull" => 0,
                    "update" | "sync" => 1,
                    _ => 1,
                };
                events.emit(Event::Progress {
                    phase: Phase::Remote,
                    completed: completed_steps,
                    total: Some(2),
                    label: format!("{host} | {operation}"),
                });
            }
            Ok(Message::Log { operation, value }) => {
                let action = if operation == "pull" {
                    Action::Pull
                } else {
                    Action::Sync
                };
                events.emit(Event::Item {
                    action,
                    path: PathBuf::from(host),
                    detail: value,
                    changed: false,
                });
            }
            Ok(Message::SyncReady { version }) => {
                if version != protocol::VERSION {
                    drop(stdin);
                    let _ = finish_child(child, stderr_thread, stdout_thread);
                    return Err(Failure::remote(format!(
                        "{host}: wire protocol {version} is incompatible with {}",
                        protocol::VERSION
                    )));
                }
                sync_ready = true;
                events.emit(Event::Progress {
                    phase: Phase::Remote,
                    completed: 1,
                    total: Some(2),
                    label: format!("{host} | sync"),
                });
            }
            Ok(Message::DecisionRequest { id, prompt }) if sync_ready => {
                let prompt = qualify_remote_prompt(host, prompt);
                let choice = match decisions.resolve_remote_prompt(prompt.clone()) {
                    Ok(choice) if prompt.accepts(choice) => choice,
                    Ok(choice) => {
                        drop(stdin);
                        let _ = finish_child(child, stderr_thread, stdout_thread);
                        return Err(Failure::remote(format!(
                            "{host}: sync interface returned invalid choice {choice:?}"
                        )));
                    }
                    Err(error) => {
                        let _ = send_decision(
                            &mut stdin,
                            &Message::DecisionResponse {
                                id,
                                choice: prompt.cancellation(),
                            },
                        );
                        drop(stdin);
                        let _ = finish_child(child, stderr_thread, stdout_thread);
                        return Err(Failure::remote(format!("{host}: {error}")));
                    }
                };
                if let Err(failure) =
                    send_decision(&mut stdin, &Message::DecisionResponse { id, choice })
                {
                    drop(stdin);
                    terminate_child(child, stderr_thread, stdout_thread);
                    return Err(failure);
                }
            }
            Ok(Message::Event { value }) if sync_ready => {
                if let Event::Finished(summary) = &value {
                    remote_finished = true;
                    remote_changed = summary.changed;
                }
                translate_remote_event(host, value, events);
            }
            Ok(Message::Completed) if sync_ready && remote_finished => completed = true,
            Ok(Message::Error {
                operation,
                value,
                code: _,
            }) => {
                drop(stdin);
                let _ = finish_child(child, stderr_thread, stdout_thread);
                return Err(remote_operation_failure(host, &operation, &value));
            }
            Ok(message) => {
                drop(stdin);
                let _ = finish_child(child, stderr_thread, stdout_thread);
                return Err(Failure::remote(format!(
                    "{host}: unexpected push protocol frame {message:?}"
                )));
            }
            Err(error) => {
                drop(stdin);
                let _ = finish_child(child, stderr_thread, stdout_thread);
                return Err(Failure::remote(format!("{host}: {error}")));
            }
        }
    }
    drop(stdin);
    let (success, stderr) = finish_child(child, stderr_thread, stdout_thread)?;
    if !success || !completed {
        return Err(Failure::remote(format!(
            "{host}: {}",
            first_line(stderr.as_bytes()).unwrap_or("remote sync ended unexpectedly")
        )));
    }
    events.emit(Event::Progress {
        phase: Phase::Remote,
        completed: 2,
        total: Some(2),
        label: format!("{host} synced"),
    });
    Ok(remote_changed)
}

fn spawn_ssh(context: &Context, host: &str, script: &str) -> Result<Child, Failure> {
    Session::new(host)
        .script(script)
        .command()
        .envs(&context.process_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::remote(format!("{host}: cannot start ssh: {error}")))
}

fn drain(mut stream: impl Read + Send + 'static) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stream.read_to_string(&mut output);
        output
    })
}

fn line_stream(
    stream: impl Read + Send + 'static,
) -> (Receiver<Result<String, String>>, JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let value = line.map_err(|error| error.to_string());
            if sender.send(value).is_err() {
                break;
            }
        }
    });
    (receiver, thread)
}

fn finish_child(
    mut child: ChildGuard,
    stderr: JoinHandle<String>,
    stdout: JoinHandle<()>,
) -> Result<(bool, String), Failure> {
    let status = child
        .wait()
        .map_err(|error| Failure::remote(format!("cannot wait for ssh: {error}")))?;
    let _ = stdout.join();
    let stderr = stderr.join().unwrap_or_default();
    Ok((status.success(), stderr))
}

fn terminate_child(mut child: ChildGuard, stderr: JoinHandle<String>, stdout: JoinHandle<()>) {
    if let Some(process) = child.0.as_mut() {
        let _ = process.kill();
    }
    let _ = child.wait();
    let _ = stdout.join();
    let _ = stderr.join();
}

fn next_remote_line(
    lines: &Receiver<Result<String, String>>,
    host: &str,
) -> Result<Option<String>, Failure> {
    loop {
        active(Phase::Remote)?;
        match lines.recv_timeout(Duration::from_millis(40)) {
            Ok(Ok(line)) => return Ok(Some(line)),
            Ok(Err(error)) => {
                return Err(Failure::remote(format!(
                    "{host}: cannot read remote response: {error}"
                )));
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(None),
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn send_decision(stdin: &mut impl Write, decision: &Message) -> Result<(), Failure> {
    let frame = protocol::encode(decision).map_err(Failure::remote)?;
    writeln!(stdin, "{frame}")
        .and_then(|()| stdin.flush())
        .map_err(|error| Failure::remote(format!("cannot answer the remote: {error}")))
}

fn qualify_remote_prompt(host: &str, prompt: crate::decision::Prompt) -> crate::decision::Prompt {
    match prompt {
        crate::decision::Prompt::Merge {
            path,
            key,
            repo,
            live,
        } => crate::decision::Prompt::Merge {
            path: PathBuf::from(format!("{host}:{}", path.display())),
            key,
            repo,
            live,
        },
        crate::decision::Prompt::MergeTarget {
            path,
            key,
            targets,
            default,
        } => crate::decision::Prompt::MergeTarget {
            path: PathBuf::from(format!("{host}:{}", path.display())),
            key,
            targets,
            default,
        },
        crate::decision::Prompt::Overwrite {
            subject,
            path,
            detail,
            repo,
            live,
            index,
            total,
        } => crate::decision::Prompt::Overwrite {
            subject,
            path: PathBuf::from(format!("{host}:{}", path.display())),
            detail,
            repo,
            live,
            index,
            total,
        },
        crate::decision::Prompt::RemoteChanges { changes, .. } => {
            crate::decision::Prompt::RemoteChanges {
                host: host.to_string(),
                changes,
            }
        }
    }
}

fn translate_remote_event(host: &str, event: Event, events: &dyn EventSink) {
    match event {
        Event::Started { profile, .. } => events.emit(Event::Progress {
            phase: Phase::Remote,
            completed: 1,
            total: Some(2),
            label: format!("{host} | {profile}"),
        }),
        Event::PhaseStarted { phase, .. } => events.emit(Event::Progress {
            phase: Phase::Remote,
            completed: 1,
            total: Some(2),
            label: format!("{host} | {}", phase_label(phase)),
        }),
        Event::Progress { phase, label, .. } => events.emit(Event::Progress {
            phase: Phase::Remote,
            completed: 1,
            total: Some(2),
            label: if label.is_empty() {
                format!("{host} | {}", phase_label(phase))
            } else {
                format!("{host} | {label}")
            },
        }),
        Event::Item {
            action,
            path,
            detail,
            changed,
        } => events.emit(Event::Item {
            action,
            path: PathBuf::from(format!("{host}:{}", path.display())),
            detail,
            changed,
        }),
        Event::Warning { message, hint } => events.emit(Event::Warning {
            message: format!("{host}: {message}"),
            hint,
        }),
        Event::Failed { message, hint, .. } => events.emit(Event::Failed {
            phase: Phase::Remote,
            message: format!("{host}: {message}"),
            hint,
        }),
        Event::Finished(_) => {}
    }
}

fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Preflight => "preflight",
        Phase::Tooling => "tooling",
        Phase::Artifacts => "artifacts",
        Phase::Plan => "plan",
        Phase::Links => "links",
        Phase::Secrets => "secrets",
        Phase::Merge => "merge",
        Phase::Push => "push",
        Phase::Remote => "remote",
        Phase::Integrations => "integrations",
    }
}

fn report_remote_changes(host: &str, state: &RemoteState, force: bool, events: &dyn EventSink) {
    if state.changes.is_empty() {
        return;
    }
    events.emit(Event::Warning {
        message: format!("{host} has {} uncommitted change(s)", state.changes.len()),
        hint: force.then(|| "discarding them because --force was passed".to_string()),
    });
    for change in &state.changes {
        events.emit(Event::Item {
            action: Action::Check,
            path: PathBuf::from(host),
            detail: change.clone(),
            changed: true,
        });
    }
}

fn branch_mismatch(host: &str, local_branch: &str, remote_branch: &str) -> Failure {
    Failure::remote(format!(
        "{host} is on '{remote_branch}' but this machine is on '{local_branch}'; check '{local_branch}' out there first"
    ))
}

fn dirty_failure(host: &str) -> Failure {
    Failure::remote(format!(
        "{host}'s working tree is not clean, rerun with --force to discard it"
    ))
}

fn protocol_script(
    host: &str,
    directory: &str,
    expected_head: &str,
    resolution: Resolution,
    force: bool,
) -> String {
    let hello = protocol::encode(&Message::Hello {
        version: protocol::VERSION,
        host: host.to_string(),
    })
    .unwrap_or_else(|_| {
        format!(
            "{{\"message\":\"hello\",\"version\":{},\"host\":\"\"}}",
            protocol::VERSION
        )
    });
    let sync = match (force, resolution) {
        (true, _) => format!("dotfile sync --wire {} --force", protocol::VERSION),
        (false, Resolution::Skip) => format!("dotfile sync --wire {}", protocol::VERSION),
        (false, Resolution::Repo) => {
            format!("dotfile sync --wire {} --resolve repo", protocol::VERSION)
        }
        (false, Resolution::Live) => {
            format!("dotfile sync --wire {} --resolve live", protocol::VERSION)
        }
    };
    let probe = format!("dotfile sync --wire-probe {}", protocol::VERSION);
    let mut lines = vec![
        "json_string() {".to_string(),
        "  LC_ALL=C awk 'BEGIN { ORS=\"\" } { if (NR > 1) printf \"\\\\n\"; for (i = 1; i <= length($0); i++) { c = substr($0, i, 1); if (c == \"\\\\\") printf \"\\\\\\\\\"; else if (c == \"\\\"\") printf \"\\\\\\\"\"; else if (c == \"\\t\") printf \"\\\\t\"; else if (c == \"\\r\") printf \"\\\\r\"; else printf \"%s\", c } }'".to_string(),
        "}".to_string(),
        "emit_error() {".to_string(),
        "  operation=$1".to_string(),
        "  value=$(printf '%s' \"$2\" | json_string)".to_string(),
        "  code=$3".to_string(),
        "  printf '{\"message\":\"error\",\"operation\":\"%s\",\"value\":\"%s\",\"code\":%s}\\n' \"$operation\" \"$value\" \"$code\"".to_string(),
        "}".to_string(),
        "emit_lines() {".to_string(),
        "  operation=$1".to_string(),
        "  value=$2".to_string(),
        "  if [ -n \"$value\" ]; then".to_string(),
        "    printf '%s\\n' \"$value\" | while IFS= read -r line; do".to_string(),
        "      encoded=$(printf '%s' \"$line\" | json_string)".to_string(),
        "      printf '{\"message\":\"log\",\"operation\":\"%s\",\"value\":\"%s\"}\\n' \"$operation\" \"$encoded\"".to_string(),
        "    done".to_string(),
        "  fi".to_string(),
        "}".to_string(),
        format!("printf '%s\\n' {}", shell_quote(&hello)),
        format!("cd \"$HOME\"/{} 2>/dev/null || {{ emit_error state 'cannot read repository' 1; exit 1; }}", shell_quote(directory)),
        format!("expected_head={}", shell_quote(expected_head)),
        "branch=$(git symbolic-ref --short -q HEAD 2>&1)".to_string(),
        "code=$?".to_string(),
        "if [ \"$code\" -ne 0 ] || [ -z \"$branch\" ]; then emit_error state \"${branch:-no branch checked out}\" \"$code\"; exit \"${code:-1}\"; fi".to_string(),
        "branch_json=$(printf '%s' \"$branch\" | json_string)".to_string(),
        "printf '{\"message\":\"state\",\"branch\":\"%s\"}\\n' \"$branch_json\"".to_string(),
        "tree_state=$(git -c core.quotePath=true status --porcelain 2>&1)".to_string(),
        "code=$?".to_string(),
        "if [ \"$code\" -ne 0 ]; then emit_error state \"$tree_state\" \"$code\"; exit \"$code\"; fi".to_string(),
        "current_head=$(git rev-parse HEAD 2>&1)".to_string(),
        "code=$?".to_string(),
        "if [ \"$code\" -ne 0 ]; then emit_error state \"$current_head\" \"$code\"; exit \"$code\"; fi".to_string(),
        "if [ -n \"$tree_state\" ]; then".to_string(),
        "  printf '%s\\n' \"$tree_state\" | while IFS= read -r line; do".to_string(),
        "    encoded=$(printf '%s' \"$line\" | json_string)".to_string(),
        "    printf '{\"message\":\"change\",\"value\":\"%s\"}\\n' \"$encoded\"".to_string(),
        "  done".to_string(),
        "fi".to_string(),
        "printf '{\"message\":\"ready\"}\\n'".to_string(),
        "IFS= read -r decision || exit 2".to_string(),
        "case \"$decision\" in".to_string(),
        "  '{\"message\":\"cancel\"}') exit 0 ;;".to_string(),
        "  '{\"message\":\"discard\"}')".to_string(),
        "    printf '{\"message\":\"phase\",\"operation\":\"discard\"}\\n'".to_string(),
        "    discard=$(git reset --hard 2>&1)".to_string(),
        "    code=$?".to_string(),
        "    emit_lines discard \"$discard\"".to_string(),
        "    if [ \"$code\" -ne 0 ]; then emit_error discard \"$discard\" \"$code\"; exit \"$code\"; fi".to_string(),
        "    clean=$(git clean -fd 2>&1)".to_string(),
        "    code=$?".to_string(),
        "    emit_lines discard \"$clean\"".to_string(),
        "    if [ \"$code\" -ne 0 ]; then emit_error discard \"$clean\" \"$code\"; exit \"$code\"; fi".to_string(),
        "    ;;".to_string(),
        "  '{\"message\":\"continue\"}') ;;".to_string(),
        "  *) emit_error control 'invalid client decision' 2; exit 2 ;;".to_string(),
        "esac".to_string(),
        "if [ \"$current_head\" != \"$expected_head\" ]; then".to_string(),
        "  printf '{\"message\":\"phase\",\"operation\":\"pull\"}\\n'".to_string(),
        "  pull=$(git pull --ff-only 2>&1)".to_string(),
        "  code=$?".to_string(),
        "  emit_lines pull \"$pull\"".to_string(),
        "  if [ \"$code\" -ne 0 ]; then emit_error pull \"$pull\" \"$code\"; exit \"$code\"; fi".to_string(),
        "fi".to_string(),
        "export PATH=\"$DOTFILES_COMPILED/:$PATH\"".to_string(),
        format!("if ! {probe} >/dev/null 2>&1; then"),
        "  printf '{\"message\":\"phase\",\"operation\":\"update\"}\\n'".to_string(),
        "  update=$(./setup.sh --commands-only 2>&1)".to_string(),
        "  code=$?".to_string(),
        "  emit_lines update \"$update\"".to_string(),
        "  if [ \"$code\" -ne 0 ]; then emit_error update \"$update\" \"$code\"; exit \"$code\"; fi".to_string(),
        format!("  if ! {probe} >/dev/null 2>&1; then emit_error update 'installed dotfile does not support the required native sync wire protocol; run ./setup.sh --commands-only on this machine' 1; exit 1; fi"),
        "fi".to_string(),
        "printf '{\"message\":\"phase\",\"operation\":\"sync\"}\\n'".to_string(),
        format!("exec {sync}"),
    ];
    lines.push(String::new());
    lines.join("\n")
}

fn remote_resolution(cli: &SyncCli) -> Resolution {
    if cli.force {
        Resolution::Repo
    } else {
        cli.resolve
    }
}

fn remote_operation_failure(host: &str, operation: &str, value: &str) -> Failure {
    let reason = value
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let mut message = if reason.is_empty() {
        format!("{host}: remote {operation} failed")
    } else {
        format!("{host}: {reason}")
    };
    if operation == "pull" {
        message.push_str("; pull with --ff-only or rebase there before retrying");
    } else if operation == "update" {
        message.push_str("; run ./setup.sh --commands-only on that machine and retry");
    }
    Failure::remote(message)
}

fn captured_output(mut command: Command, phase: Phase) -> Result<Output, String> {
    active(phase).map_err(|failure| failure.message)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = ChildGuard::new(command.spawn().map_err(|error| error.to_string())?);
    let stdout = child
        .child()
        .stdout
        .take()
        .ok_or_else(|| "cannot capture command output".to_string())?;
    let stderr = child
        .child()
        .stderr
        .take()
        .ok_or_else(|| "cannot capture command errors".to_string())?;
    let stdout = drain_bytes(stdout);
    let stderr = drain_bytes(stderr);
    let status = loop {
        active(phase).map_err(|failure| failure.message)?;
        match child
            .child()
            .try_wait()
            .map_err(|error| error.to_string())?
        {
            Some(status) => {
                child.0 = None;
                break status;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn drain_bytes(mut stream: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stream.read_to_end(&mut output);
        output
    })
}

fn active(phase: Phase) -> Result<(), Failure> {
    crate::cancel::check().map_err(|message| Failure {
        phase,
        message,
        hint: None,
    })
}

fn first_line(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
}
