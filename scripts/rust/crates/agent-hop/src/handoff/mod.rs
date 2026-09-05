mod child;
mod goals;
mod proxy;
mod rpc;
mod snapshot;

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hostkit::{Host, shell::quote};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cli::{Agent, Command};

const PROTOCOL: u32 = 1;
const POLL: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct State {
    pub protocol: u32,
    pub id: String,
    pub agent: String,
    pub session: Option<String>,
    pub workspace: PathBuf,
    pub agent_home: PathBuf,
    pub phase: String,
    pub pane: Option<String>,
    pub destination: Option<String>,
    pub destination_run: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub supervisor: Option<(u32, String)>,
    #[serde(default)]
    pub ownership_committed: bool,
    #[serde(default)]
    pub goal: Option<goals::GoalSnapshot>,
    #[serde(default)]
    pub goal_resume_pending: bool,
    #[serde(default)]
    pub goal_history: Vec<goals::GoalSnapshot>,
    #[serde(default)]
    pub validated_snapshot: Option<PathBuf>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Request {
    destination: String,
    id: String,
}

pub(crate) fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Run { agent, resume } => {
            let pane = pane(None)?;
            let id = new_id()?;
            let directory = new_run(&id)?;
            let agent_home = match agent {
                Agent::Codex => rpc::default_home()?,
                Agent::Claude => crate::local_home()?.join(".claude"),
            };
            let state = State {
                protocol: PROTOCOL,
                id,
                agent: agent.name().into(),
                session: resume,
                workspace: crate::physical_current_directory()?,
                agent_home,
                phase: "starting".into(),
                pane: Some(pane),
                destination: None,
                destination_run: None,
                error: None,
                supervisor: None,
                ownership_committed: false,
                goal: None,
                goal_resume_pending: false,
                goal_history: Vec::new(),
                validated_snapshot: None,
            };
            save(&directory, &state)?;
            serve(state, false)
        }
        Command::Move { pane: selected, to } => {
            let id = pane_run(selected.as_deref())?;
            let directory = run_dir(&id)?;
            let state = observed_state(&directory)?;
            if !matches!(state.phase.as_str(), "running" | "busy") {
                return Err(format!("run is {}; inspect agent-hop status", state.phase));
            }
            let destination = to.unwrap_or(Host::this()?.peer());
            if destination == Host::this()? {
                return Err("destination is this host".into());
            }
            let preflight = remote(destination, "preflight", &id, None)?;
            if preflight.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL.into()) {
                return Err("destination agent-hop protocol mismatch".into());
            }
            let request = Request {
                destination: destination.name().into(),
                id: new_id()?,
            };
            create_json(&directory.join("request.json"), &request)?;
            println!(
                "Move queued → {}; current turns finish, then managed root goals continue there. agent-hop status --run {id}",
                destination.name()
            );
            Ok(())
        }
        Command::Status { pane, run } => {
            let id = match run {
                Some(id) => id,
                None => pane_run(pane.as_deref())?,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&observed_state(&run_dir(&id)?)?).map_err(error)?
            );
            Ok(())
        }
        Command::Cancel { pane } => {
            let directory = run_dir(&pane_run(pane.as_deref())?)?;
            let state = read(&directory)?;
            let request: Request = if state.phase == "queued" {
                decode(&directory.join("claimed.json"))?
            } else if matches!(state.phase.as_str(), "running" | "busy") {
                decode(&directory.join("request.json")).map_err(|_| "no move is queued")?
            } else {
                return Err("handoff already preparing; inspect agent-hop status".into());
            };
            // Bind cancellation to this request so a race cannot cancel a future move.
            atomic_json(&directory.join("cancel.json"), &json!({"id":request.id}))?;
            println!("Queued move cancellation requested");
            Ok(())
        }
        Command::Follow { pane } => follow(&read(&run_dir(&pane_run(pane.as_deref())?)?)?),
        Command::Recover { pane, run } => {
            let id = match run {
                Some(id) => id,
                None => pane_run(pane.as_deref())?,
            };
            let directory = run_dir(&id)?;
            let lease = lease(&directory)?;
            let mut state = read(&directory)?;
            state.ownership_committed |= directory.join("activate.json").exists();
            if state.validated_snapshot.is_some() && !state.ownership_committed {
                return Err("this prepared checkpoint never owned execution; recover or retry from its authoritative source pane".into());
            }
            if let (Some(host), Some(id)) = (&state.destination, &state.destination_run) {
                let peer = Host::from_name(host)?;
                let remote_state = remote(peer, "status", id, None)?;
                if matches!(
                    remote_state.get("phase").and_then(Value::as_str),
                    Some("running" | "moved" | "closed")
                ) || remote_state
                    .get("ownership_committed")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    return Err(
                        "destination owns execution; use agent-hop follow or resume there".into(),
                    );
                }
                remote(peer, "abort", id, None)?;
                let until = Instant::now() + Duration::from_secs(20);
                loop {
                    let stopped = remote(peer, "status", id, None)?;
                    if stopped.get("ownership_committed").and_then(Value::as_bool) == Some(true) {
                        return Err("destination committed ownership while recovery was checking; source remains stopped".into());
                    }
                    if stopped.get("phase").and_then(Value::as_str) == Some("aborted") {
                        break;
                    }
                    if Instant::now() > until {
                        return Err("destination shutdown unverified; source remains fenced".into());
                    }
                    std::thread::sleep(POLL);
                }
            }
            state.phase = "starting".into();
            state.destination = None;
            state.destination_run = None;
            state.error = None;
            for marker in ["fenced.json", "activate.json", "abort.json"] {
                let path = directory.join(marker);
                if path.exists() {
                    fs::rename(&path, directory.join(format!("recovered-{marker}")))
                        .map_err(error)?;
                }
            }
            save(&directory, &state)?;
            drop(lease);
            serve(state, false)
        }
        Command::Handoff { operation, id } => internal(&operation, &id),
        _ => Err("invalid handoff command".into()),
    }
}

fn internal(operation: &str, id: &str) -> Result<(), String> {
    valid_id(id)?;
    match operation {
        "preflight" => {
            for program in ["tmux", "git"] {
                let result = Process::new(program)
                    .arg("--version")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                // tmux uses -V; existence, not an unsupported flag's exit code, is the probe.
                if result.is_err() {
                    return Err(format!("destination missing {program}"));
                }
            }
            println!(
                "{}",
                json!({"protocol":PROTOCOL,"home":crate::local_home()?})
            );
            Ok(())
        }
        "receive" => snapshot::receive(id),
        "serve" => serve(read(&run_dir(id)?)?, true),
        "status" => {
            let directory = run_dir(id)?;
            if !directory.exists() {
                println!("{}", json!({"protocol":PROTOCOL,"id":id,"phase":"absent"}));
                return Ok(());
            }
            let state = observed_state(&directory)?;
            println!("{}", serde_json::to_string(&state).map_err(error)?);
            Ok(())
        }
        "activate" | "abort" => {
            let directory = run_dir(id)?;
            // Separate SSH requests must make exactly one durable terminal decision.
            let _decision = decision_lease(&directory, operation)?;
            let state = read(&directory)?;
            if operation == "activate" && !supervisor_alive(&state)? {
                return Err("destination supervisor is no longer alive; source ownership must not be released".into());
            }
            if operation == "activate" && !matches!(state.phase.as_str(), "ready" | "running") {
                return Err(format!("destination is {}, not ready", state.phase));
            }
            if operation == "abort" && matches!(state.phase.as_str(), "running" | "moved") {
                return Err("destination already owns execution; cannot abort".into());
            }
            if operation == "abort"
                && (state.ownership_committed || directory.join("activate.json").exists())
            {
                return Err("destination has committed ownership; resume or recover there, never the stale source".into());
            }
            if operation == "abort"
                && let Ok(_lease) = lease(&directory)
            {
                let mut state = state;
                state.phase = "aborted".into();
                save(&directory, &state)?;
                println!("{}", json!({"protocol":PROTOCOL,"accepted":"abort"}));
                return Ok(());
            }
            atomic_json(
                &directory.join(format!("{operation}.json")),
                &json!({"id":id}),
            )?;
            println!("{}", json!({"protocol":PROTOCOL,"accepted":operation}));
            Ok(())
        }
        "hook" => claude_hook(id),
        _ => Err("invalid handoff operation".into()),
    }
}

fn serve(mut state: State, prepared: bool) -> Result<(), String> {
    let directory = run_dir(&state.id)?;
    state.supervisor = Some((
        std::process::id(),
        child::identity(std::process::id())?.ok_or("supervisor process missing")?,
    ));
    let _lease = lease(&directory)?;
    let current_pane = pane(None)?;
    state.pane = Some(current_pane.clone());
    tmux(&[
        "set-option",
        "-p",
        "-t",
        &current_pane,
        "remain-on-exit",
        "on",
    ])?;
    tmux(&[
        "set-option",
        "-p",
        "-t",
        &current_pane,
        "remain-on-exit-format",
        " agent session retained · use agent status / follow / recovery from the action palette ",
    ])?;
    tmux(&[
        "set-option",
        "-p",
        "-t",
        &current_pane,
        "@agent_hop_run",
        &state.id,
    ])?;
    save(&directory, &state)?;
    let result = if state.agent == "codex" {
        serve_codex(&directory, &mut state, prepared)
    } else {
        serve_claude(&directory, &mut state, prepared)
    };
    if let Err(ref error) = result {
        if state.phase != "commit-uncertain" {
            state.phase = "failed".into();
        }
        state.error = Some(error.clone());
        let _ = save(&directory, &state);
    }
    // Keep the run ID on an exited pane for receipt inspection.
    let _ = tmux(&["select-pane", "-e", "-t", &current_pane]);
    result
}

fn follow(state: &State) -> Result<(), String> {
    let destination = Host::from_name(
        state
            .destination
            .as_deref()
            .ok_or("no destination recorded")?,
    )?;
    let id = state
        .destination_run
        .as_deref()
        .ok_or("destination run missing")?;
    valid_id(id)?;
    let ready = remote(destination, "status", id, None)?;
    if ready.get("phase").and_then(Value::as_str) != Some("running") {
        return Err("destination not running; inspect agent-hop status".into());
    }
    let script = format!("tmux attach-session -t {}", quote(&format!("ah-{id}")));
    let status = hostkit::ssh::Session::new(destination.name())
        .interactive()
        .script(&script)
        .command()
        .status()
        .map_err(error)?;
    if status.success() {
        Ok(())
    } else {
        Err("destination attach ended with an error".into())
    }
}

#[allow(unsafe_code)]
fn lease(directory: &Path) -> Result<File, String> {
    use std::os::fd::AsRawFd;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("owner.lock"))
        .map_err(error)?;
    // Advisory lock is scoped to this run and released by the OS after crashes.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("managed run is already supervised".into());
    }
    // Native agent children inherit the lease: supervisor crashes cannot release ownership
    // while the app-server, UI, or their inherited children are still alive.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    if flags < 0
        || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0
    {
        return Err(error(std::io::Error::last_os_error()));
    }
    Ok(file)
}

#[allow(unsafe_code)]
fn decision_lease(directory: &Path, operation: &str) -> Result<File, String> {
    use std::os::fd::AsRawFd;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("decision.lock"))
        .map_err(error)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(error(std::io::Error::last_os_error()));
    }
    let opposite = if operation == "activate" {
        "abort.json"
    } else {
        "activate.json"
    };
    if directory.join(opposite).exists() {
        return Err(format!(
            "opposite ownership decision {opposite} is already durable"
        ));
    }
    Ok(file)
}

fn serve_codex(directory: &Path, state: &mut State, prepared: bool) -> Result<(), String> {
    let trusted_snapshot = validated_snapshot(state, directory)?;
    let mut server = rpc::Codex::start(
        &state.workspace,
        &state.agent_home,
        &directory.join("server.log"),
        trusted_snapshot,
    )?;
    if state.session.is_some() {
        state.session = Some(server.open(&state.workspace, state.session.as_deref())?);
    }
    let mut session = state.session.clone().unwrap_or_default();
    if prepared && let Some(goal) = &state.goal {
        goals::stage(&mut server, &session, goal)?;
    }
    if prepared && !session.is_empty() && !server.idle(&session)? {
        return Err("resumed destination has active work; ownership unchanged".into());
    }
    let pane = state.pane.as_deref().ok_or("pane missing")?.to_owned();
    if prepared {
        server.authenticated()?;
        tmux(&["select-pane", "-d", "-t", &pane])?;
    }
    let proxy = proxy::Proxy::start(&server.endpoint, &session, prepared)?;
    let mut ui = rpc::ui(
        &proxy.endpoint,
        &state.agent_home,
        &state.workspace,
        &session,
        trusted_snapshot,
    )?;
    let until = Instant::now() + Duration::from_secs(90);
    while !proxy.ready() {
        if prepared && directory.join("abort.json").exists() {
            ui.terminate()?;
            server.shutdown()?;
            state.phase = "aborted".into();
            save(directory, state)?;
            return Ok(());
        }
        if let Some(status) = ui.try_wait().map_err(error)? {
            return Err(format!(
                "native Codex UI exited before resuming the managed thread ({status})"
            ));
        }
        if Instant::now() > until {
            return Err("native Codex UI did not confirm session resume".into());
        }
        std::thread::sleep(POLL);
    }
    session = proxy.current_session();
    state.session = Some(session.clone());
    if prepared {
        if !server.idle(&session)? {
            return Err("destination active before ownership transfer".into());
        }
        state.phase = "ready".into();
        save(directory, state)?;
        loop {
            if directory.join("abort.json").exists() {
                ui.terminate()?;
                server.shutdown()?;
                state.phase = "aborted".into();
                save(directory, state)?;
                return Ok(());
            }
            if ui.try_wait().map_err(error)?.is_some()
                || !proxy.ready()
                || !server.idle(&session)?
            {
                return Err("prepared destination lost readiness".into());
            }
            if directory.join("activate.json").exists() {
                break;
            }
            std::thread::sleep(POLL);
        }
        state.ownership_committed = true;
        state.goal_resume_pending = state
            .goal
            .as_ref()
            .is_some_and(|goal| goal.status == "active");
        save(directory, state)?;
        if let Some(goal) = &state.goal {
            goals::activate(&mut server, &session, goal)?;
        }
        state.goal_resume_pending = false;
        save(directory, state)?;
        proxy.fence(false);
        tmux(&["select-pane", "-e", "-t", &pane])?;
    } else if state.goal_resume_pending {
        if let Some(goal) = &state.goal {
            if state.ownership_committed {
                goals::recover_destination(&mut server, &session, goal)?;
            } else {
                goals::rollback(&mut server, &session, goal)?;
            }
        }
        state.goal_resume_pending = false;
    }
    state.phase = "running".into();
    save(directory, state)?;
    loop {
        if ui.try_wait().map_err(error)?.is_some() {
            state.phase = "closed".into();
            save(directory, state)?;
            return server.shutdown();
        }
        if let Some(request) = take_request(directory, state)? {
            session = proxy.current_session();
            state.session = Some(session.clone());
            if let Some(goal) = &state.goal {
                state.goal_history.push(goal.clone());
            }
            state.goal = match goals::capture(&mut server, &session) {
                Ok(goal) => goal,
                Err(failure) => {
                    state.phase = "running".into();
                    state.error = Some(failure);
                    state.destination = None;
                    state.destination_run = None;
                    save(directory, state)?;
                    continue;
                }
            };
            state.goal_resume_pending = state
                .goal
                .as_ref()
                .is_some_and(|goal| goal.status == "active");
            state.phase = "queued".into();
            save(directory, state)?;
            if let Some(goal) = &state.goal
                && let Err(failure) = goals::pause(&mut server, &session, goal)
            {
                decline_codex_move(
                    directory,
                    state,
                    &mut server,
                    &session,
                    &proxy,
                    &pane,
                    failure,
                )?;
                continue;
            }
            let goal_session = session.clone();
            let mut queue_failure = None;
            loop {
                if cancellation_requested(directory, state)? {
                    break;
                }
                if ui.try_wait().map_err(error)?.is_some() {
                    return Err("source UI closed while move queued".into());
                }
                if proxy.ready() && proxy.current_session() != goal_session {
                    session = proxy.current_session();
                    break;
                }
                match goals::capture(&mut server, &goal_session) {
                    Err(failure) => {
                        queue_failure = Some(failure);
                        break;
                    }
                    Ok(Some(goal)) if goal.status == "active" => {
                        state.goal = Some(goal.clone());
                        state.goal_resume_pending = true;
                        save(directory, state)?;
                        if let Err(failure) = goals::pause(&mut server, &goal_session, &goal) {
                            queue_failure = Some(failure);
                            break;
                        }
                    }
                    _ => {}
                }
                // Approvals and existing turns must finish with the UI unfenced.
                // Fencing is a forwarding barrier, not evidence of an idle backend.
                if proxy.ready() && server.idle(&proxy.current_session())? {
                    proxy.fence(true);
                    if proxy.ready() {
                        session = proxy.current_session();
                        if server.idle(&session)? {
                            break;
                        }
                    }
                    // A turn, selection, or approval raced the first idle query.
                    // Release buffered replies and retry without stopping execution.
                    proxy.fence(false);
                }
                std::thread::sleep(POLL);
            }
            if let Some(failure) = queue_failure {
                decline_codex_move(
                    directory,
                    state,
                    &mut server,
                    &goal_session,
                    &proxy,
                    &pane,
                    failure,
                )?;
                continue;
            }
            if cancelled(directory, state)? {
                if let Some(goal) = &state.goal {
                    goals::rollback(&mut server, &goal_session, goal)?;
                }
                state.goal_resume_pending = false;
                save(directory, state)?;
                proxy.fence(false);
                continue;
            }
            if session != goal_session {
                if let Some(goal) = &state.goal {
                    goals::rollback(&mut server, &goal_session, goal)?;
                }
                state.goal_resume_pending = false;
                save(directory, state)?;
                atomic_json(&directory.join("request.json"), &request)?;
                proxy.fence(false);
                continue;
            }
            tmux(&["select-pane", "-d", "-t", &pane])?;
            if !proxy.ready() || !server.idle(&session)? {
                if let Some(goal) = &state.goal {
                    goals::rollback(&mut server, &goal_session, goal)?;
                }
                state.goal_resume_pending = false;
                save(directory, state)?;
                tmux(&["select-pane", "-e", "-t", &pane])?;
                atomic_json(&directory.join("request.json"), &request)?;
                proxy.fence(false);
                continue;
            }
            state.session = Some(session.clone());
            let thread = server.request("thread/read", json!({"threadId":session}))?;
            if let Some(cwd) = thread.pointer("/thread/cwd").and_then(Value::as_str) {
                state.workspace = PathBuf::from(cwd);
            }
            if let Some(goal) = &state.goal {
                match goals::refresh(&mut server, &session, goal) {
                    Ok(goal) => state.goal = goal,
                    Err(failure) => {
                        decline_codex_move(
                            directory,
                            state,
                            &mut server,
                            &session,
                            &proxy,
                            &pane,
                            failure,
                        )?;
                        continue;
                    }
                }
            }
            save(directory, state)?;
            server.pause()?;
            let ui_processes = child::descendants(ui.id())?;
            let preparation = prepare(directory, state, &request);
            if let Err(failure) = preparation {
                server.unpause()?;
                if let Some(goal) = &state.goal {
                    goals::rollback(&mut server, &session, goal)?;
                }
                state.goal_resume_pending = false;
                state.phase = "running".into();
                state.error = Some(failure);
                state.destination = None;
                state.destination_run = None;
                save(directory, state)?;
                proxy.fence(false);
                tmux(&["select-pane", "-e", "-t", &pane])?;
                continue;
            }
            if let Err(failure) = server.shutdown() {
                state.error = Some(failure.clone());
                save(directory, state)?;
                return Err(failure);
            }
            ui.terminate()?;
            child::require_gone(&ui_processes)?;
            // Every source-owned backend and UI process is gone before activation.
            state.phase = "source-stopped".into();
            save(directory, state)?;
            commit(directory, state, &request)?;
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

fn validated_snapshot(state: &State, directory: &Path) -> Result<bool, String> {
    let Some(root) = &state.validated_snapshot else {
        return Ok(false);
    };
    let expected = fs::canonicalize(directory.join("workspace")).map_err(error)?;
    if fs::canonicalize(root).map_err(error)? != *root {
        return Err("validated snapshot root was replaced or redirected".into());
    }
    let cwd = fs::canonicalize(&state.workspace).map_err(error)?;
    if *root != expected || !cwd.starts_with(root) {
        return Err("validated snapshot workspace escaped its transaction".into());
    }
    Ok(true)
}

fn decline_codex_move(
    directory: &Path,
    state: &mut State,
    server: &mut rpc::Codex,
    root: &str,
    proxy: &proxy::Proxy,
    pane: &str,
    mut failure: String,
) -> Result<(), String> {
    if let Some(goal) = &state.goal {
        match goals::rollback(server, root, goal) {
            Ok(()) => state.goal_resume_pending = false,
            Err(rollback) => {
                failure.push_str(&format!("; source goal rollback unconfirmed: {rollback}"))
            }
        }
    }
    state.phase = "running".into();
    state.error = Some(failure);
    state.destination = None;
    state.destination_run = None;
    save(directory, state)?;
    proxy.fence(false);
    tmux(&["select-pane", "-e", "-t", pane])?;
    Ok(())
}

fn take_request(directory: &Path, state: &mut State) -> Result<Option<Request>, String> {
    let path = directory.join("request.json");
    if !path.exists() {
        return Ok(None);
    }
    let request: Request = decode(&path)?;
    fs::rename(path, directory.join("claimed.json")).map_err(error)?;
    state.destination = Some(request.destination.clone());
    state.destination_run = Some(request.id.clone());
    state.error = None;
    Ok(Some(request))
}

fn cancelled(directory: &Path, state: &mut State) -> Result<bool, String> {
    if !directory.join("cancel.json").exists() {
        return Ok(false);
    }
    let marker: Value = decode(&directory.join("cancel.json"))?;
    fs::rename(
        directory.join("cancel.json"),
        directory.join("cancelled.json"),
    )
    .map_err(error)?;
    if marker.get("id").and_then(Value::as_str) != state.destination_run.as_deref() {
        return Ok(false);
    }
    state.phase = "running".into();
    state.destination = None;
    state.destination_run = None;
    save(directory, state)?;
    Ok(true)
}

fn cancellation_requested(directory: &Path, state: &State) -> Result<bool, String> {
    if !directory.join("cancel.json").exists() {
        return Ok(false);
    }
    let marker: Value = decode(&directory.join("cancel.json"))?;
    Ok(marker.get("id").and_then(Value::as_str) == state.destination_run.as_deref())
}

fn supervisor_alive(state: &State) -> Result<bool, String> {
    match &state.supervisor {
        Some((pid, identity)) => Ok(child::identity(*pid)?.as_ref() == Some(identity)),
        None => Ok(false),
    }
}

fn observed_state(directory: &Path) -> Result<State, String> {
    let mut state = read(directory)?;
    state.ownership_committed |= directory.join("activate.json").exists();
    if matches!(state.phase.as_str(), "running" | "ready" | "queued")
        && (lease(directory).is_ok() || !supervisor_alive(&state)?)
    {
        state.phase = "failed".into();
        state.error = Some(
            "execution supervisor exited unexpectedly; inspect ownership before recovery".into(),
        );
    }
    Ok(state)
}

fn prepare(directory: &Path, state: &mut State, request: &Request) -> Result<(), String> {
    state.phase = "preparing".into();
    save(directory, state)?;
    let peer = Host::from_name(&request.destination)?;
    let response = remote(peer, "preflight", &request.id, None)?;
    let home = response
        .get("home")
        .and_then(Value::as_str)
        .ok_or("destination returned no home")?;
    let bundle = snapshot::create(state, &request.id, Path::new(home))?;
    remote(
        peer,
        "receive",
        &request.id,
        Some(bundle.as_file().try_clone().map_err(error)?),
    )?;
    let until = Instant::now() + Duration::from_secs(90);
    loop {
        let status = remote(peer, "status", &request.id, None)?;
        match status.get("phase").and_then(Value::as_str) {
            Some("ready") => break,
            Some("failed" | "aborted" | "closed") => {
                return Err(format!(
                    "destination startup: {}",
                    status.get("error").unwrap_or(&Value::Null)
                ));
            }
            _ if Instant::now() < until => std::thread::sleep(Duration::from_millis(300)),
            _ => {
                let _ = remote(peer, "abort", &request.id, None);
                return Err(
                    "destination not ready; source retained; inspect destination receipt".into(),
                );
            }
        }
    }
    state.phase = "destination-ready".into();
    save(directory, state)
}

fn commit(directory: &Path, state: &mut State, request: &Request) -> Result<(), String> {
    let peer = Host::from_name(&request.destination)?;
    // Persist ambiguity *before* sending activation: a lost reply never resurrects a second owner.
    state.phase = "commit-uncertain".into();
    save(directory, state)?;
    remote(peer, "activate", &request.id, None).map_err(|e| {
        format!(
            "{e}; source stopped; inspect destination with ssh {} agent-hop status --run {}",
            peer.name(),
            request.id
        )
    })?;
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let status = remote(peer, "status", &request.id, None)?;
        match status.get("phase").and_then(Value::as_str) {
            Some("running") => break,
            Some("failed" | "closed") => {
                return Err(
                    "destination UI failed after ownership transfer; source transcript retained"
                        .into(),
                );
            }
            _ if Instant::now() < until => std::thread::sleep(POLL),
            _ => {
                return Err(
                    "activation pending; inspect destination receipt before resuming elsewhere"
                        .into(),
                );
            }
        }
    }
    state.phase = "moved".into();
    save(directory, state)?;
    println!(
        "\nExecution moved → {}. Attach: ssh -t {} tmux attach -t ah-{}",
        peer.name(),
        peer.name(),
        request.id
    );
    Ok(())
}

pub(super) fn remote(
    peer: Host,
    operation: &str,
    id: &str,
    input: Option<File>,
) -> Result<Value, String> {
    let args = [
        "__handoff".into(),
        operation.into(),
        "--id".into(),
        id.into(),
    ];
    let mut command =
        crate::remote::machine_ssh_session(peer, &crate::remote::machine_script(&args)).command();
    command
        .stdin(input.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let timeout = Duration::from_secs(if operation == "receive" { 300 } else { 25 });
    let output = crate::remote::bounded_handoff_output(command, peer, timeout)?;
    serde_json::from_slice(&output)
        .map_err(|e| format!("invalid {} handoff reply: {e}", peer.name()))
}

pub(super) fn new_id() -> Result<String, String> {
    Ok(format!(
        "{:x}-{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(error)?
            .as_nanos(),
        std::process::id()
    ))
}

pub(super) fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 80 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err("invalid run ID".into());
    }
    Ok(())
}

pub(super) fn run_dir(id: &str) -> Result<PathBuf, String> {
    valid_id(id)?;
    Ok(crate::local_home()?
        .join(".local/state/agent-hop/runs")
        .join(id))
}

pub(super) fn new_run(id: &str) -> Result<PathBuf, String> {
    let directory = run_dir(id)?;
    let parent = directory.parent().ok_or("run parent missing")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(error)?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|e| format!("run already exists or cannot be created: {e}"))?;
    Ok(directory)
}

pub(super) fn save(directory: &Path, state: &State) -> Result<(), String> {
    atomic_json(&directory.join("state.json"), state)
}
pub(super) fn read(directory: &Path) -> Result<State, String> {
    decode(&directory.join("state.json"))
}
fn decode<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("handoff record too large".into());
    }
    serde_json::from_slice(&bytes).map_err(error)
}
fn create_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("request parent missing")?)
        .map_err(error)?;
    serde_json::to_writer(&mut file, value).map_err(error)?;
    file.as_file().sync_all().map_err(error)?;
    file.persist_noclobber(path)
        .map_err(|e| format!("request already queued or cannot be written: {e}"))?;
    Ok(())
}
pub(super) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("record parent missing")?)
        .map_err(error)?;
    serde_json::to_writer(&mut file, value).map_err(error)?;
    file.as_file().sync_all().map_err(error)?;
    file.persist(path).map_err(error)?;
    File::open(path.parent().ok_or("record parent missing")?)
        .map_err(error)?
        .sync_all()
        .map_err(error)?;
    Ok(())
}

fn pane(selected: Option<&str>) -> Result<String, String> {
    let pane = selected
        .map(str::to_owned)
        .or_else(|| std::env::var("TMUX_PANE").ok())
        .ok_or("managed agents require a tmux pane")?;
    if !pane.starts_with('%') || pane.len() < 2 || !pane[1..].bytes().all(|c| c.is_ascii_digit()) {
        return Err("invalid tmux pane".into());
    }
    Ok(pane)
}
fn pane_run(selected: Option<&str>) -> Result<String, String> {
    let pane = pane(selected)?;
    let id = tmux(&["show-option", "-p", "-v", "-t", &pane, "@agent_hop_run"])?;
    if id.is_empty() {
        return Err("unmanaged agent: start with agent-hop run codex or agent-hop run claude; history copying does not stop active execution".into());
    }
    valid_id(&id)?;
    Ok(id)
}
pub(super) fn tmux(args: &[&str]) -> Result<String, String> {
    let output = Process::new("tmux").args(args).output().map_err(error)?;
    if !output.status.success() {
        return Err(format!(
            "tmux: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}
pub(super) fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// Claude exposes lifecycle hooks, unlike Codex's directly queryable control socket.
#[derive(Default, Deserialize, Serialize)]
struct HookState {
    session: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    idle: bool,
    active: std::collections::BTreeSet<String>,
}

fn claude_hook(id: &str) -> Result<(), String> {
    let directory = run_dir(id)?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    let event: Value = serde_json::from_slice(&bytes).map_err(error)?;
    let name = event
        .get("hook_event_name")
        .and_then(Value::as_str)
        .ok_or("hook event missing")?;
    let session = event
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or("hook session missing")?;
    crate::session::SessionId::new(session)?;
    let startup = name == "SessionStart" && !directory.join("startup-ready.json").exists();
    if startup && let Some(pane) = read(&directory)?.pane {
        tmux(&["select-pane", "-d", "-t", &pane])?;
    }
    let lock = directory.join("hook.lock");
    let until = Instant::now() + Duration::from_secs(3);
    while fs::create_dir(&lock).is_err() {
        if Instant::now() > until {
            return Err("hook state busy".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = (|| {
        let mut state: HookState = decode(&directory.join("hook.json")).unwrap_or_default();
        if name == "SessionStart" {
            state.session = Some(session.into());
            state.cwd = event.get("cwd").and_then(Value::as_str).map(PathBuf::from);
            state.idle = true;
        }
        if state.session.as_deref() != Some(session) {
            return Ok(());
        }
        if matches!(name, "UserPromptSubmit" | "PreToolUse")
            && directory.join("fenced.json").exists()
        {
            println!(
                "{}",
                json!({"decision":"block","reason":"Execution handoff in progress; inspect agent-hop status"})
            );
            return Ok(());
        }
        match name {
            "UserPromptSubmit" | "PreToolUse" => state.idle = false,
            "Stop" => state.idle = true,
            "SubagentStart" => {
                if let Some(id) = event.get("agent_id").and_then(Value::as_str) {
                    state.active.insert(id.into());
                }
            }
            "SubagentStop" => {
                if let Some(id) = event.get("agent_id").and_then(Value::as_str) {
                    state.active.remove(id);
                }
            }
            _ => {}
        }
        atomic_json(&directory.join("hook.json"), &state)
    })();
    let _ = fs::remove_dir(lock);
    result?;
    if startup {
        let until = Instant::now() + Duration::from_secs(3);
        while !directory.join("startup-ready.json").exists() {
            if Instant::now() > until {
                return Err("managed supervisor did not acknowledge SessionStart".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Ok(())
}

fn claude_command(state: &State, directory: &Path) -> Result<Process, String> {
    let exe = std::env::current_exe().map_err(error)?;
    let command = format!(
        "{} __handoff hook --id {}",
        quote(exe.to_str().ok_or("executable path is not UTF-8")?),
        quote(&state.id)
    );
    let mut hooks = serde_json::Map::new();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "Stop",
        "SubagentStart",
        "SubagentStop",
    ] {
        hooks.insert(
            event.into(),
            json!([{"hooks":[{"type":"command","command":command,"timeout":5}]}]),
        );
    }
    let settings = directory.join("hooks.json");
    atomic_json(&settings, &json!({"hooks":hooks}))?;
    let mut process = Process::new("claude");
    process
        .arg("--settings")
        .arg(settings)
        .env("CLAUDE_CONFIG_DIR", &state.agent_home)
        .current_dir(&state.workspace);
    if let Some(session) = &state.session {
        process.args(["--resume", session]);
    }
    Ok(process)
}

fn serve_claude(directory: &Path, state: &mut State, prepared: bool) -> Result<(), String> {
    let pane = state.pane.as_deref().ok_or("pane missing")?.to_owned();
    for marker in ["hook.json", "startup-ready.json"] {
        if directory.join(marker).exists() {
            fs::rename(
                directory.join(marker),
                directory.join(format!("previous-{marker}")),
            )
            .map_err(error)?;
        }
    }
    if prepared {
        tmux(&["select-pane", "-d", "-t", &pane])?;
        let auth = Process::new("claude")
            .args(["auth", "status", "--json"])
            .env("CLAUDE_CONFIG_DIR", &state.agent_home)
            .output()
            .map_err(error)?;
        let auth_state: Value = serde_json::from_slice(&auth.stdout)
            .map_err(|_| "destination Claude auth status unavailable")?;
        if !auth.status.success()
            || auth_state.get("loggedIn").and_then(Value::as_bool) != Some(true)
        {
            return Err(
                "destination Claude is not authenticated; run claude auth login on that host"
                    .into(),
            );
        }
    }
    // A prepared Claude UI is fenced at its synchronous SessionStart boundary.
    if prepared {
        atomic_json(&directory.join("fenced.json"), &json!(true))?;
    }
    let mut child = child::AgentChild::spawn(&mut claude_command(state, directory)?)?;
    let until = Instant::now() + Duration::from_secs(90);
    loop {
        if prepared && directory.join("abort.json").exists() {
            let owned = child::descendants(child.id())?;
            child.terminate()?;
            child::require_gone(&owned)?;
            state.phase = "aborted".into();
            save(directory, state)?;
            return Ok(());
        }
        if child.try_wait().map_err(error)?.is_some() {
            return Err("Claude exited before SessionStart; inspect hook configuration".into());
        }
        let hook: HookState = decode(&directory.join("hook.json")).unwrap_or_default();
        if let Some(session) = hook.session {
            state.session = Some(session);
            break;
        }
        if Instant::now() > until {
            return Err("Claude SessionStart not received; source execution retained".into());
        }
        std::thread::sleep(POLL);
    }
    let baseline = child::descendants(child.id())?;
    atomic_json(&directory.join("startup-ready.json"), &json!(true))?;
    if prepared {
        tmux(&[
            "select-pane",
            "-d",
            "-t",
            state.pane.as_deref().ok_or("pane missing")?,
        ])?;
        state.phase = "ready".into();
        save(directory, state)?;
        loop {
            if directory.join("abort.json").exists() {
                child.terminate()?;
                child::require_gone(&baseline)?;
                state.phase = "aborted".into();
                save(directory, state)?;
                return Ok(());
            }
            if child.try_wait().map_err(error)?.is_some() {
                return Err("prepared Claude exited before activation".into());
            }
            let hook: HookState = decode(&directory.join("hook.json"))?;
            if !hook.idle || !hook.active.is_empty() || hook.session != state.session {
                return Err("prepared Claude lost its idle session boundary".into());
            }
            if directory.join("activate.json").exists() {
                break;
            }
            std::thread::sleep(POLL);
        }
        state.ownership_committed = true;
        save(directory, state)?;
        fs::rename(
            directory.join("fenced.json"),
            directory.join("unfenced.json"),
        )
        .map_err(error)?;
        tmux(&[
            "select-pane",
            "-e",
            "-t",
            state.pane.as_deref().ok_or("pane missing")?,
        ])?;
    }
    state.phase = "running".into();
    save(directory, state)?;
    tmux(&["select-pane", "-e", "-t", &pane])?;
    loop {
        if child.try_wait().map_err(error)?.is_some() {
            state.phase = "closed".into();
            save(directory, state)?;
            return Ok(());
        }
        if let Some(request) = take_request(directory, state)? {
            state.phase = "queued".into();
            save(directory, state)?;
            loop {
                let hook: HookState = decode(&directory.join("hook.json"))?;
                if hook.idle
                    && hook.active.is_empty()
                    && child::descendants(child.id())?.is_subset(&baseline)
                    || cancellation_requested(directory, state)?
                {
                    break;
                }
                if child.try_wait().map_err(error)?.is_some() {
                    return Err("Claude exited while move queued".into());
                }
                std::thread::sleep(POLL);
            }
            if cancelled(directory, state)? {
                continue;
            }
            let pane = state.pane.clone().ok_or("pane missing")?;
            tmux(&["select-pane", "-d", "-t", &pane])?;
            atomic_json(&directory.join("fenced.json"), &json!(true))?;
            child.signal(libc::SIGSTOP)?;
            let hook: HookState = decode(&directory.join("hook.json"))?;
            if !hook.idle
                || !hook.active.is_empty()
                || !child::descendants(child.id())?.is_subset(&baseline)
            {
                child.signal(libc::SIGCONT)?;
                fs::rename(
                    directory.join("fenced.json"),
                    directory.join("unfenced.json"),
                )
                .map_err(error)?;
                tmux(&["select-pane", "-e", "-t", &pane])?;
                atomic_json(&directory.join("request.json"), &request)?;
                continue;
            }
            state.session = hook.session;
            if let Some(cwd) = hook.cwd {
                state.workspace = cwd;
            }
            if let Err(failure) = prepare(directory, state, &request) {
                child.signal(libc::SIGCONT)?;
                fs::rename(
                    directory.join("fenced.json"),
                    directory.join("unfenced.json"),
                )
                .map_err(error)?;
                tmux(&["select-pane", "-e", "-t", &pane])?;
                state.phase = "running".into();
                state.error = Some(failure);
                state.destination = None;
                state.destination_run = None;
                save(directory, state)?;
                continue;
            }
            child.terminate()?;
            child::require_gone(&baseline)?;
            state.phase = "source-stopped".into();
            save(directory, state)?;
            return commit(directory, state, &request);
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_keeps_ownership_lease_after_supervisor_file_drops() {
        let directory = tempfile::tempdir().unwrap();
        let owner = lease(directory.path()).unwrap();
        let mut child = Process::new("sleep").arg("0.2").spawn().unwrap();
        drop(owner);
        assert!(lease(directory.path()).is_err());
        assert!(child.wait().unwrap().success());
        assert!(lease(directory.path()).is_ok());
    }
    #[test]
    fn concurrent_activate_and_abort_can_publish_only_one_decision() {
        for _ in 0..20 {
            let directory = tempfile::tempdir().unwrap();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
            let workers = ["activate", "abort"].map(|operation| {
                let path = directory.path().to_path_buf();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let Ok(_lock) = decision_lease(&path, operation) else {
                        return false;
                    };
                    // Widen the exact old race: both contenders are already launched.
                    std::thread::yield_now();
                    atomic_json(&path.join(format!("{operation}.json")), &json!(true)).unwrap();
                    true
                })
            });
            barrier.wait();
            let accepted = workers
                .into_iter()
                .map(|worker| usize::from(worker.join().unwrap()))
                .sum::<usize>();
            assert_eq!(accepted, 1);
            assert_ne!(
                directory.path().join("activate.json").exists(),
                directory.path().join("abort.json").exists()
            );
            let losing = if directory.path().join("activate.json").exists() {
                "abort"
            } else {
                "activate"
            };
            assert!(decision_lease(directory.path(), losing).is_err());
        }
    }
}
