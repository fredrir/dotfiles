#![forbid(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::CommandFactory;
use hostkit::Host;

mod catalog;
pub mod cli;
mod favorites;
mod inbound;
mod interactive;
mod lineage;
mod machine;
mod manifest;
mod plan;
mod preferences;
mod preview;
mod remote;
mod report;
mod session;
mod transfer;
mod tui;

use cli::{Agent, Cli, ColorMode, Command};
use remote::Remote;
use tui::{Origin, PickerAction, PickerOutcome};

#[derive(Clone, Copy)]
pub(crate) struct TransferOptions {
    pub(crate) dry_run: bool,
    pub(crate) no_connect: bool,
    pub(crate) color: ColorMode,
}

pub fn run(cli: Cli) -> Result<(), String> {
    if let Some(Command::Machine(machine)) = cli.command {
        return machine::run(machine.request);
    }
    if cli.list {
        return interactive::list();
    }
    let options = TransferOptions {
        dry_run: cli.dry_run,
        no_connect: cli.no_connect,
        color: cli.color,
    };
    if let Some(agent) = cli.agent {
        return send_local_session(agent, cli.session_id.as_deref(), options);
    }
    if !tui::capable() {
        let mut command = Cli::command();
        command
            .print_help()
            .map_err(|error| format!("could not print help: {error}"))?;
        std::io::stdout()
            .write_all(b"\n")
            .map_err(|error| format!("could not print help: {error}"))?;
        return Ok(());
    }
    let default_action = if options.dry_run {
        PickerAction::DryRun
    } else if options.no_connect {
        PickerAction::CopyOnly
    } else {
        PickerAction::HopAndOpen
    };
    match interactive::browse(options.color, default_action)? {
        PickerOutcome::Cancelled(_) => Ok(()),
        PickerOutcome::Picked(picked) => {
            let options = TransferOptions {
                dry_run: picked.action == PickerAction::DryRun,
                no_connect: picked.action == PickerAction::CopyOnly,
                color: options.color,
            };
            match picked.session.origin {
                Origin::Local => {
                    send_local_session(picked.session.agent, Some(&picked.session.id), options)
                }
                Origin::Remote => {
                    inbound::receive(picked.session.agent, &picked.session.id, options)
                }
            }
        }
    }
}

fn send_local_session(
    agent: Agent,
    session_id: Option<&str>,
    options: TransferOptions,
) -> Result<(), String> {
    let this = Host::this()?;
    let peer = this.peer();
    let home = local_home()?;
    let current_cwd = physical_current_directory()?;
    let session = session::discover(&home, &current_cwd, agent, session_id)?;

    let route_peer = peer.name().to_owned();
    let route = std::thread::spawn(move || hostkit::ssh::resolved(&route_peer));
    let remote = Remote::new(peer);
    let remote_home = remote.home()?;
    let mut lineage =
        lineage::Lineage::discover(&home, &session)?.transform(&home, &remote_home)?;
    for artifact in &mut lineage.artifacts {
        let mapped = plan::destination(
            session.agent,
            &artifact.source.session_id,
            &artifact.source.transcript,
            &artifact.source.workspace,
            &home,
            &remote_home,
            false,
        )?;
        artifact.destination.transcript = mapped.transcript;
    }
    let selected = lineage
        .artifacts
        .last()
        .ok_or_else(|| "session lineage is empty".to_string())?;
    let destination = plan::destination(
        session.agent,
        session.id.as_str(),
        &session.transcript,
        &session.workspace,
        &home,
        &remote_home,
        session.companion.is_some(),
    )?;
    if selected.destination.transcript != destination.transcript
        || selected.destination.workspace != destination.workspace
    {
        return Err("lineage transformer produced an inconsistent destination".to_string());
    }
    remote.preflight(&destination.workspace, session.agent)?;

    let style = options.color.style();
    let source_workspace = plan::display(&session.workspace, &home);
    let destination_workspace = plan::display(&destination.workspace, &remote_home);
    let source_transcript = plan::display(&session.transcript, &home);
    let destination_transcript = plan::display(&destination.transcript, &remote_home);
    let view = report::View {
        this: this.name(),
        peer: peer.name(),
        route: route.join().ok().flatten(),
        agent: session.agent,
        session_id: session.id.as_str(),
        source_workspace: &source_workspace,
        destination_workspace: &destination_workspace,
        source_transcript: &source_transcript,
        destination_transcript: &destination_transcript,
    };
    println!("{}", report::header(&style, &view));
    println!();
    for line in report::details(&style, &view) {
        println!("{line}");
    }
    println!();

    if !options.dry_run
        && !options.no_connect
        && let Some(child_id) = manifest::latest_child(
            &home,
            this.name(),
            peer.name(),
            session.agent,
            session.id.as_str(),
            &selected.source.sha256,
        )?
        && remote_child_is_resumable(
            remote,
            &remote_home,
            session.agent,
            &child_id,
            &destination.workspace,
            &selected.destination,
        )?
        && offer_resume(peer.name(), session.agent, &child_id)?
    {
        println!(
            "Resuming existing child session {child_id} on {}.",
            peer.name()
        );
        return remote.resume(&destination.workspace, session.agent, &child_id);
    }

    let attachment_count = count_companion(session.companion.as_deref())?;
    if options.dry_run {
        let reused = verify_remote_lineage(remote, &lineage)?;
        if reused == lineage.artifacts.len() {
            println!("{}", report::reused(&style));
        }
        println!(
            "Validated {} immutable history object{}.",
            lineage.artifacts.len(),
            if lineage.artifacts.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        println!("{}", report::dry_run(&style));
        return Ok(());
    }

    let bytes = lineage
        .artifacts
        .iter()
        .map(|artifact| artifact.destination.bytes)
        .sum();
    let mut reused = 0usize;
    for artifact in &lineage.artifacts {
        let already_present = verify_destination(
            remote,
            artifact.snapshot.path(),
            &artifact.destination.transcript,
        )?;
        if already_present {
            reused += 1;
        } else {
            remote.install_artifact(
                session.agent,
                &artifact.destination,
                artifact.snapshot.path(),
            )?;
        }
    }
    if let (Some(source), Some(destination)) = (
        session.companion.as_deref(),
        destination.companion.as_deref(),
    ) {
        transfer::copy_companion(peer, source, destination)?;
    }
    validate_remote_install(
        remote,
        &remote_home,
        session.agent,
        session.id.as_str(),
        &lineage,
    )?;
    if reused == lineage.artifacts.len() {
        println!("{}", report::reused(&style));
        if attachment_count > 0 {
            println!("{}", report::attachments(&style, attachment_count));
        }
    } else {
        println!("{}", report::copied(&style, bytes, attachment_count));
    }

    let manifest = manifest::TransferManifest::installed(
        this.name(),
        peer.name(),
        &home,
        &remote_home,
        &lineage,
    )?;
    let manifest_path = manifest::record(&home, &manifest)?;
    remote.record_manifest(&manifest)?;
    println!("Transfer manifest: {}", manifest_path.display());

    if options.no_connect {
        println!("{}", report::copied_without_connect(&style, peer.name()));
        return Ok(());
    }
    println!("{}", report::launching(&style, session.agent, peer.name()));
    let before = remote.catalog(&remote_home, None, remote::MAX_REMOTE_SESSIONS)?;
    remote.launch(&destination.workspace, session.agent, session.id.as_str())?;
    if let Some(child_id) = detect_created_child(
        before,
        remote.catalog(&remote_home, None, remote::MAX_REMOTE_SESSIONS)?,
        session.agent,
        &destination.workspace,
        session.id.as_str(),
    ) && remote_child_is_resumable(
        remote,
        &remote_home,
        session.agent,
        &child_id,
        &destination.workspace,
        &selected.destination,
    )? {
        let launched = manifest.launched(child_id.clone())?;
        let path = manifest::record(&home, &launched)?;
        remote.record_manifest(&launched)?;
        println!("Captured child session {child_id} in {}", path.display());
    } else {
        eprintln!("warning: the agent exited, but no unique child session ID could be identified");
    }
    Ok(())
}

fn remote_child_is_resumable(
    remote: Remote,
    remote_home: &Path,
    agent: Agent,
    child_id: &str,
    workspace: &Path,
    expected_parent: &lineage::ArtifactDescriptor,
) -> Result<bool, String> {
    let exists = remote
        .catalog(remote_home, None, remote::MAX_REMOTE_SESSIONS)?
        .sessions
        .iter()
        .any(|candidate| {
            candidate.agent == agent && candidate.id == child_id && candidate.workspace == workspace
        });
    if !exists {
        return Ok(false);
    }
    let child_lineage = remote.lineage(remote_home, agent, child_id)?;
    let Some(child) = child_lineage.artifacts.last() else {
        return Ok(false);
    };
    if child.workspace != workspace {
        return Ok(false);
    }
    if agent == Agent::Claude {
        return Ok(true);
    }
    let directly_forked = child
        .history_base
        .as_ref()
        .is_some_and(|base| base.thread_id == expected_parent.session_id);
    let immutable_parent_matches = child_lineage.artifacts.iter().any(|artifact| {
        artifact.session_id == expected_parent.session_id
            && artifact.transcript == expected_parent.transcript
            && artifact.workspace == expected_parent.workspace
            && artifact.history_base == expected_parent.history_base
            && artifact.bytes == expected_parent.bytes
            && artifact.sha256 == expected_parent.sha256
    });
    Ok(directly_forked && immutable_parent_matches)
}

pub(crate) fn offer_resume(host: &str, agent: Agent, child_id: &str) -> Result<bool, String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        println!(
            "Previous hop created {agent} child {child_id} on {host}; run that session ID to resume it.",
            agent = agent.name()
        );
        return Ok(false);
    }
    print!(
        "A previous hop created {agent} child {child_id} on {host}. Resume it? [Y/n] ",
        agent = agent.name()
    );
    std::io::stdout()
        .flush()
        .map_err(|error| format!("could not show resume prompt: {error}"))?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("could not read resume choice: {error}"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    ))
}

fn verify_remote_lineage(
    remote: Remote,
    lineage: &lineage::TransformedLineage,
) -> Result<usize, String> {
    let mut reused = 0;
    for artifact in &lineage.artifacts {
        if verify_destination(
            remote,
            artifact.snapshot.path(),
            &artifact.destination.transcript,
        )? {
            reused += 1;
        }
    }
    Ok(reused)
}

fn validate_remote_install(
    remote: Remote,
    remote_home: &Path,
    agent: Agent,
    selected_id: &str,
    expected: &lineage::TransformedLineage,
) -> Result<(), String> {
    let actual = remote.lineage(remote_home, agent, selected_id)?;
    if actual.artifacts.len() != expected.artifacts.len() {
        return Err("destination did not retain the complete transferred lineage".to_string());
    }
    for (actual, expected) in actual.artifacts.iter().zip(&expected.artifacts) {
        if actual.session_id != expected.destination.session_id
            || actual.transcript != expected.destination.transcript
            || actual.workspace != expected.destination.workspace
            || actual.history_base != expected.destination.history_base
            || actual.bytes != expected.destination.bytes
            || actual.sha256 != expected.destination.sha256
        {
            return Err(format!(
                "destination rollout {} does not match the immutable transferred object",
                expected.destination.session_id
            ));
        }
    }
    Ok(())
}

fn detect_created_child(
    before: remote::RemoteCatalog,
    after: remote::RemoteCatalog,
    agent: Agent,
    workspace: &Path,
    parent_id: &str,
) -> Option<String> {
    use std::collections::HashSet;
    let existing = before
        .sessions
        .into_iter()
        .map(|session| (session.agent, session.id))
        .collect::<HashSet<_>>();
    let mut created = after
        .sessions
        .into_iter()
        .filter(|session| {
            session.agent == agent
                && session.workspace == workspace
                && session.id != parent_id
                && !existing.contains(&(session.agent, session.id.clone()))
        })
        .map(|session| session.id);
    let child = created.next()?;
    if created.next().is_some() {
        None
    } else {
        Some(child)
    }
}

fn verify_destination(remote: Remote, source: &Path, destination: &Path) -> Result<bool, String> {
    if !remote.exists(destination)? {
        return Ok(false);
    }
    if remote.file_matches(source, destination)? {
        Ok(true)
    } else {
        Err(format!(
            "session already exists with different contents on {}: {}",
            remote.peer().name(),
            destination.display()
        ))
    }
}

fn local_home() -> Result<PathBuf, String> {
    let value = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("HOME is not an absolute path".to_string());
    }
    fs::canonicalize(&path)
        .map_err(|error| format!("could not resolve {}: {error}", path.display()))
}

fn physical_current_directory() -> Result<PathBuf, String> {
    let path =
        std::env::current_dir().map_err(|error| format!("this directory is gone: {error}"))?;
    fs::canonicalize(&path).map_err(|error| {
        format!(
            "could not resolve current directory {}: {error}",
            path.display()
        )
    })
}

fn count_companion(path: Option<&Path>) -> Result<usize, String> {
    let Some(path) = path else {
        return Ok(0);
    };
    let mut count = 0;
    let mut directories = vec![path.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("could not read {}: {error}", directory.display()))?
        {
            let entry = entry
                .map_err(|error| format!("could not read {}: {error}", directory.display()))?;
            let file_type = entry.file_type().map_err(|error| {
                format!("could not inspect {}: {error}", entry.path().display())
            })?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_session(id: &str, agent: Agent, workspace: &Path) -> remote::RemoteSession {
        remote::RemoteSession {
            agent,
            id: id.to_string(),
            title: String::new(),
            project: String::new(),
            workspace: workspace.to_path_buf(),
            transcript: PathBuf::from(format!("/tmp/{id}.jsonl")),
            companion: None,
            modified_ms: 0,
        }
    }

    #[test]
    fn companion_count_includes_nested_files_without_following_symlinks() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("companion");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("one"), "one").unwrap();
        fs::write(root.join("nested/two"), "two").unwrap();
        assert_eq!(count_companion(Some(&root)).unwrap(), 2);
        assert_eq!(count_companion(None).unwrap(), 0);
    }

    #[test]
    fn child_capture_requires_one_new_session_in_the_destination_workspace() {
        let workspace = Path::new("/home/f/project");
        let before = remote::RemoteCatalog {
            sessions: vec![remote_session("parent", Agent::Codex, workspace)],
            warnings: Vec::new(),
        };
        let after = remote::RemoteCatalog {
            sessions: vec![
                remote_session("parent", Agent::Codex, workspace),
                remote_session("child", Agent::Codex, workspace),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            detect_created_child(before.clone(), after, Agent::Codex, workspace, "parent"),
            Some("child".to_string())
        );

        let ambiguous = remote::RemoteCatalog {
            sessions: vec![
                remote_session("parent", Agent::Codex, workspace),
                remote_session("child-one", Agent::Codex, workspace),
                remote_session("child-two", Agent::Codex, workspace),
            ],
            warnings: Vec::new(),
        };
        assert_eq!(
            detect_created_child(before, ambiguous, Agent::Codex, workspace, "parent"),
            None
        );
    }
}
