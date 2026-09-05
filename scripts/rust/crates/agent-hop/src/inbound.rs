use std::fs::{self, File};
use std::io::{self, BufReader, IsTerminal, Read};
use std::path::{Component, Path};
use std::process::{Command, Stdio};

use crate::TransferOptions;
use crate::cli::Agent;
use crate::plan;
use crate::remote::{MAX_REMOTE_SESSIONS, Remote, RemoteSession};
use crate::transfer::Snapshot;
use hostkit::Host;
use tempfile::{NamedTempFile, TempDir};
use workstation::Style;
use workstation::path::home_relative_in;

pub(crate) fn receive(
    agent: Agent,
    session_id: &str,
    options: TransferOptions,
) -> Result<(), String> {
    let this = Host::this()?;
    let peer = this.peer();
    let remote = Remote::new(peer);
    let home = crate::local_home()?;
    let remote_home = remote.home()?;
    let (mut lineage, session) = pull_lineage(remote, &remote_home, &home, agent, session_id)?;
    for artifact in &mut lineage.artifacts {
        let mapped = plan::destination(
            session.agent,
            &artifact.source.session_id,
            &artifact.source.transcript,
            &artifact.source.workspace,
            &remote_home,
            &home,
            false,
        )?;
        artifact.destination.transcript = mapped.transcript;
    }
    let destination = plan::destination(
        session.agent,
        &session.id,
        &session.transcript,
        &session.workspace,
        &remote_home,
        &home,
        session.companion.is_some(),
    )?;
    let selected = lineage
        .artifacts
        .last()
        .ok_or_else(|| "remote session lineage is empty".to_string())?;
    if selected.destination.transcript != destination.transcript
        || selected.destination.workspace != destination.workspace
    {
        return Err("lineage transformer produced an inconsistent local destination".to_string());
    }
    local_preflight(&destination.workspace, session.agent)?;

    let route_peer = peer.name().to_string();
    let route = std::thread::spawn(move || hostkit::ssh::resolved(&route_peer));
    let style = Style::for_mode(options.color, io::stdout().is_terminal());
    let source_workspace = home_relative_in(&session.workspace, &remote_home);
    let destination_workspace = home_relative_in(&destination.workspace, &home);
    let source_transcript = home_relative_in(&session.transcript, &remote_home);
    let destination_transcript = home_relative_in(&destination.transcript, &home);
    let view = crate::report::View {
        this: peer.name(),
        peer: this.name(),
        route: route.join().ok().flatten(),
        agent: session.agent,
        session_id: &session.id,
        source_workspace: &source_workspace,
        destination_workspace: &destination_workspace,
        source_transcript: &source_transcript,
        destination_transcript: &destination_transcript,
    };
    println!("{}", crate::report::header(&style, &view));
    println!();
    for line in crate::report::details(&style, &view) {
        println!("{line}");
    }
    println!();

    if !options.dry_run
        && !options.no_connect
        && let Some(child_id) = crate::manifest::latest_child(
            &home,
            peer.name(),
            this.name(),
            session.agent,
            &session.id,
            &selected.source.sha256,
        )?
        && local_child_is_resumable(
            &home,
            session.agent,
            &child_id,
            &destination.workspace,
            &selected.destination,
        )?
        && crate::offer_resume(this.name(), session.agent, &child_id)?
    {
        println!(
            "Resuming existing child session {child_id} on {}.",
            this.name()
        );
        return resume_local(&destination.workspace, session.agent, &child_id);
    }

    let bytes = lineage
        .artifacts
        .iter()
        .map(|artifact| artifact.destination.bytes)
        .sum();
    let reused = verify_local_lineage(&lineage)?;
    if options.dry_run {
        if reused == lineage.artifacts.len() {
            println!("{}", crate::report::reused(&style));
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
        println!("{}", crate::report::dry_run(&style));
        return Ok(());
    }

    for artifact in &lineage.artifacts {
        let destination_directory = artifact
            .destination
            .transcript
            .parent()
            .ok_or_else(|| "a destination transcript has no parent directory".to_string())?;
        ensure_directory_tree(&home, destination_directory)?;
        if !verify_local_destination(artifact.snapshot.path(), &artifact.destination.transcript)? {
            install_new_file(artifact.snapshot.path(), &artifact.destination.transcript)?;
        }
    }

    if let Some(companion) = destination.companion.as_deref() {
        install_companion(remote, &remote_home, &session, &home, companion).map_err(|error| {
            format!(
                "{error}; the transcript is present at {}, but attachments were not installed",
                destination.transcript.display()
            )
        })?;
    }
    validate_local_install(&home, session.agent, &session.id, &lineage)?;
    let attachment_count = crate::count_companion(destination.companion.as_deref())?;
    if reused == lineage.artifacts.len() {
        println!("{}", crate::report::reused(&style));
        if attachment_count > 0 {
            println!("{}", crate::report::attachments(&style, attachment_count));
        }
    } else {
        println!("{}", crate::report::copied(&style, bytes, attachment_count));
    }

    let manifest = crate::manifest::TransferManifest::installed(
        peer.name(),
        this.name(),
        &remote_home,
        &home,
        &lineage,
    )?;
    let manifest_path = crate::manifest::record(&home, &manifest)?;
    remote.record_manifest(&manifest)?;
    println!("Transfer manifest: {}", manifest_path.display());

    if options.no_connect {
        println!(
            "{}",
            crate::report::copied_without_connect(&style, this.name())
        );
        return Ok(());
    }
    println!(
        "{}",
        crate::report::launching(&style, session.agent, this.name())
    );
    let before = local_session_ids(&home, session.agent, &destination.workspace);
    launch_local(&destination.workspace, session.agent, &session.id)?;
    let after = local_session_ids(&home, session.agent, &destination.workspace);
    let mut created = after
        .difference(&before)
        .filter(|id| id.as_str() != session.id)
        .cloned()
        .collect::<Vec<_>>();
    created.sort();
    if created.len() == 1
        && local_child_is_resumable(
            &home,
            session.agent,
            &created[0],
            &destination.workspace,
            &selected.destination,
        )?
    {
        let child_id = created.pop().unwrap();
        let launched = manifest.launched(child_id.clone())?;
        let path = crate::manifest::record(&home, &launched)?;
        remote.record_manifest(&launched)?;
        println!("Captured child session {child_id} in {}", path.display());
    } else {
        eprintln!("warning: the agent exited, but no unique child session ID could be identified");
    }
    Ok(())
}

fn local_child_is_resumable(
    home: &Path,
    agent: Agent,
    child_id: &str,
    workspace: &Path,
    expected_parent: &crate::lineage::ArtifactDescriptor,
) -> Result<bool, String> {
    if !local_session_ids(home, agent, workspace).contains(child_id) {
        return Ok(false);
    }
    let session = crate::session::discover(home, home, agent, Some(child_id))?;
    if session.workspace != workspace {
        return Ok(false);
    }
    let child_lineage = crate::lineage::Lineage::discover(home, &session)?;
    let Some(child) = child_lineage.artifacts.last() else {
        return Ok(false);
    };
    if agent == Agent::Claude {
        return Ok(true);
    }
    let directly_forked = child
        .descriptor
        .history_base
        .as_ref()
        .is_some_and(|base| base.thread_id == expected_parent.session_id);
    let immutable_parent_matches = child_lineage.artifacts.iter().any(|artifact| {
        artifact.descriptor.session_id == expected_parent.session_id
            && artifact.descriptor.transcript == expected_parent.transcript
            && artifact.descriptor.workspace == expected_parent.workspace
            && artifact.descriptor.history_base == expected_parent.history_base
            && artifact.descriptor.bytes == expected_parent.bytes
            && artifact.descriptor.sha256 == expected_parent.sha256
    });
    Ok(directly_forked && immutable_parent_matches)
}

fn revalidate(
    remote: Remote,
    remote_home: &Path,
    agent: Agent,
    session_id: &str,
) -> Result<RemoteSession, String> {
    let found = remote.catalog(remote_home, None, MAX_REMOTE_SESSIONS)?;
    let mut matches = found
        .sessions
        .into_iter()
        .filter(|session| session.agent == agent && session.id == session_id);
    let session = matches.next().ok_or_else(|| {
        format!(
            "the selected {} session no longer exists on {}",
            agent.name(),
            remote.peer().name()
        )
    })?;
    if matches.next().is_some() {
        return Err(format!(
            "the selected session is ambiguous on {}",
            remote.peer().name()
        ));
    }
    Ok(session)
}

fn pull_lineage(
    remote: Remote,
    remote_home: &Path,
    local_home: &Path,
    agent: Agent,
    session_id: &str,
) -> Result<(crate::lineage::TransformedLineage, RemoteSession), String> {
    let mut last_error = String::new();
    for _ in 0..3 {
        let session = match revalidate(remote, remote_home, agent, session_id) {
            Ok(session) => session,
            Err(error) => {
                last_error = error;
                continue;
            }
        };
        let described = match remote.lineage(remote_home, agent, session_id) {
            Ok(lineage) => lineage,
            Err(error) => {
                last_error = error;
                continue;
            }
        };
        if described.artifacts.last().is_none_or(|artifact| {
            artifact.transcript != session.transcript || artifact.workspace != session.workspace
        }) {
            last_error = "selected session changed during lineage discovery".to_string();
            continue;
        }
        let mut snapshots = Vec::with_capacity(described.artifacts.len());
        let mut failed = None;
        for artifact in &described.artifacts {
            let temporary = NamedTempFile::new()
                .map_err(|error| format!("could not create a temporary snapshot: {error}"))?;
            match remote
                .pull_artifact(remote_home, agent, artifact, temporary.path())
                .and_then(|()| Snapshot::from_temporary(temporary))
            {
                Ok(snapshot) => snapshots.push(snapshot),
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failed {
            last_error = error;
            continue;
        }
        match crate::lineage::Lineage::from_snapshots(agent, described.artifacts, snapshots)
            .and_then(|lineage| lineage.transform(remote_home, local_home))
        {
            Ok(lineage) => return Ok((lineage, session)),
            Err(error) => last_error = error,
        }
    }
    Err(format!(
        "could not pull a valid snapshot from {} after 3 attempts: {last_error}",
        remote.peer().name()
    ))
}

fn verify_local_lineage(lineage: &crate::lineage::TransformedLineage) -> Result<usize, String> {
    let mut reused = 0;
    for artifact in &lineage.artifacts {
        if verify_local_destination(artifact.snapshot.path(), &artifact.destination.transcript)? {
            reused += 1;
        }
    }
    Ok(reused)
}

fn validate_local_install(
    home: &Path,
    agent: Agent,
    selected_id: &str,
    expected: &crate::lineage::TransformedLineage,
) -> Result<(), String> {
    let session = crate::session::discover(home, home, agent, Some(selected_id))?;
    let actual = crate::lineage::Lineage::discover(home, &session)?.descriptors();
    if actual.len() != expected.artifacts.len() {
        return Err(
            "local destination did not retain the complete transferred lineage".to_string(),
        );
    }
    for (actual, expected) in actual.iter().zip(&expected.artifacts) {
        if actual.session_id != expected.destination.session_id
            || actual.transcript != expected.destination.transcript
            || actual.workspace != expected.destination.workspace
            || actual.history_base != expected.destination.history_base
            || actual.bytes != expected.destination.bytes
            || actual.sha256 != expected.destination.sha256
        {
            return Err(format!(
                "local rollout {} does not match the immutable transferred object",
                expected.destination.session_id
            ));
        }
    }
    Ok(())
}

fn local_session_ids(
    home: &Path,
    agent: Agent,
    workspace: &Path,
) -> std::collections::BTreeSet<String> {
    crate::catalog::scan(home, home)
        .sessions
        .into_iter()
        .filter(|entry| entry.session.agent == agent && entry.session.workspace == workspace)
        .map(|entry| entry.session.id.as_str().to_string())
        .collect()
}

fn install_companion(
    remote: Remote,
    remote_home: &Path,
    session: &RemoteSession,
    home: &Path,
    destination: &Path,
) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "the attachment destination has no parent directory".to_string())?;
    ensure_directory_tree(home, parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".agent-hop-attachments-")
        .tempdir_in(parent)
        .map_err(|error| format!("could not stage session attachments: {error}"))?;
    let payload = staging.path().join("payload");
    fs::create_dir(&payload)
        .map_err(|error| format!("could not stage session attachments: {error}"))?;
    remote.pull_companion(remote_home, session, &payload)?;
    validate_companion_tree(&payload)?;
    commit_companion(staging, &payload, destination)
}

fn validate_companion_tree(root: &Path) -> Result<(), String> {
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| format!("could not inspect staged attachments: {error}"))?
        {
            let entry = entry.map_err(|error| format!("could not inspect attachments: {error}"))?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("could not inspect attachments: {error}"))?;
            if metadata.file_type().is_dir() {
                directories.push(entry.path());
            } else if !metadata.file_type().is_file() {
                return Err(format!(
                    "remote attachments contain an unsafe entry: {}",
                    entry.path().display()
                ));
            }
        }
    }
    Ok(())
}

fn commit_companion(staging: TempDir, payload: &Path, destination: &Path) -> Result<(), String> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::rename(payload, destination).map_err(|error| {
                format!(
                    "could not install session attachments at {}: {error}",
                    destination.display()
                )
            })?;
            drop(staging);
            Ok(())
        }
        Ok(metadata) if metadata.file_type().is_dir() => {
            if companion_trees_equal(payload, destination)? {
                Ok(())
            } else {
                Err(format!(
                    "session attachments already exist with different contents: {}",
                    destination.display()
                ))
            }
        }
        Ok(_) => Err(format!(
            "attachment destination is not a safe directory: {}",
            destination.display()
        )),
        Err(error) => Err(format!(
            "could not inspect attachment destination {}: {error}",
            destination.display()
        )),
    }
}

fn companion_trees_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let mut left_entries = fs::read_dir(left)
        .map_err(|error| format!("could not inspect staged attachments: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not inspect staged attachments: {error}"))?;
    let mut right_entries = fs::read_dir(right)
        .map_err(|error| format!("could not inspect existing attachments: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not inspect existing attachments: {error}"))?;
    left_entries.sort_by_key(fs::DirEntry::file_name);
    right_entries.sort_by_key(fs::DirEntry::file_name);
    let left_names = left_entries
        .iter()
        .map(fs::DirEntry::file_name)
        .collect::<Vec<_>>();
    let right_names = right_entries
        .iter()
        .map(fs::DirEntry::file_name)
        .collect::<Vec<_>>();
    if left_names != right_names {
        return Ok(false);
    }
    for (left_entry, right_entry) in left_entries.iter().zip(&right_entries) {
        let left_path = left_entry.path();
        let right_path = right_entry.path();
        let left_metadata = fs::symlink_metadata(&left_path)
            .map_err(|error| format!("could not inspect staged attachments: {error}"))?;
        let right_metadata = fs::symlink_metadata(&right_path)
            .map_err(|error| format!("could not inspect existing attachments: {error}"))?;
        if left_metadata.file_type().is_dir() && right_metadata.file_type().is_dir() {
            if !companion_trees_equal(&left_path, &right_path)? {
                return Ok(false);
            }
        } else if left_metadata.file_type().is_file() && right_metadata.file_type().is_file() {
            if !files_equal(&left_path, &right_path)? {
                return Ok(false);
            }
        } else if !left_metadata.file_type().is_file() && !left_metadata.file_type().is_dir()
            || !right_metadata.file_type().is_file() && !right_metadata.file_type().is_dir()
        {
            return Err("session attachments contain an unsafe entry".to_string());
        } else {
            return Ok(false);
        }
    }
    Ok(true)
}

fn local_preflight(workspace: &Path, agent: Agent) -> Result<(), String> {
    if !workspace.is_dir() {
        return Err(format!(
            "destination workspace does not exist: {}",
            workspace.display()
        ));
    }
    let available = Command::new(agent.name())
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !available {
        return Err(format!(
            "{} command is not available on this workstation",
            agent.name()
        ));
    }
    Ok(())
}

fn verify_local_destination(source: &Path, destination: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "could not inspect {}: {error}",
                destination.display()
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(format!(
            "destination session is not a safe regular file: {}",
            destination.display()
        ));
    }
    if files_equal(source, destination)? {
        Ok(true)
    } else {
        Err(format!(
            "session already exists with different contents on this workstation: {}",
            destination.display()
        ))
    }
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let left_file =
        File::open(left).map_err(|error| format!("could not open {}: {error}", left.display()))?;
    let right_file = File::open(right)
        .map_err(|error| format!("could not open {}: {error}", right.display()))?;
    let mut left = BufReader::new(left_file);
    let mut right = BufReader::new(right_file);
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_count = left
            .read(&mut left_buffer)
            .map_err(|error| format!("could not compare session: {error}"))?;
        let right_count = right
            .read(&mut right_buffer)
            .map_err(|error| format!("could not compare session: {error}"))?;
        if left_count != right_count {
            return Ok(false);
        }
        if left_count == 0 {
            return Ok(true);
        }
        if left_buffer[..left_count] != right_buffer[..right_count] {
            return Ok(false);
        }
    }
}

fn ensure_directory_tree(home: &Path, target: &Path) -> Result<(), String> {
    let relative = target.strip_prefix(home).map_err(|_| {
        format!(
            "destination is outside the local home directory: {}",
            target.display()
        )
    })?;
    let mut current = home.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(format!(
                "destination is not a normalized path: {}",
                target.display()
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "destination contains an unsafe non-directory: {}",
                    current.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&current)
                .map_err(|error| format!("could not create {}: {error}", current.display()))?,
            Err(error) => {
                return Err(format!("could not inspect {}: {error}", current.display()));
            }
        }
    }
    Ok(())
}

fn install_new_file(source: &Path, destination: &Path) -> Result<(), String> {
    crate::transfer::install_immutable_file(source, destination)?;
    Ok(())
}

fn launch_local(workspace: &Path, agent: Agent, session_id: &str) -> Result<(), String> {
    connect_local(workspace, agent, session_id, true)
}

fn resume_local(workspace: &Path, agent: Agent, session_id: &str) -> Result<(), String> {
    connect_local(workspace, agent, session_id, false)
}

fn connect_local(
    workspace: &Path,
    agent: Agent,
    session_id: &str,
    fork: bool,
) -> Result<(), String> {
    let mut command = Command::new(agent.name());
    command.current_dir(workspace);
    match agent {
        Agent::Codex => {
            command
                .arg(if fork { "fork" } else { "resume" })
                .arg(session_id)
                .arg("-C")
                .arg(workspace);
        }
        Agent::Claude => {
            command.arg("--resume").arg(session_id);
            if fork {
                command.arg("--fork-session");
            }
        }
    }
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("could not start {}: {error}", agent.name()))?;
    if status.success() {
        Ok(())
    } else {
        Err(match status.code() {
            Some(code) => format!("local {} session exited with status {code}", agent.name()),
            None => format!("local {} session was interrupted", agent.name()),
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/inbound_tests.rs"]
mod tests;
