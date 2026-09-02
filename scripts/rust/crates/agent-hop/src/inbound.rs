use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path};
use std::process::{Command, Stdio};

use crate::TransferOptions;
use crate::cli::Agent;
use crate::plan;
use crate::remote::{MAX_REMOTE_SESSIONS, Remote, RemoteSession};
use crate::transfer::Snapshot;
use hostkit::Host;
use tempfile::{NamedTempFile, TempDir};

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
    let (snapshot, session) = pull_snapshot(remote, &remote_home, agent, session_id)?;
    let destination = plan::destination(
        session.agent,
        &session.id,
        &session.transcript,
        &session.workspace,
        &remote_home,
        &home,
        session.companion.is_some(),
    )?;
    local_preflight(&destination.workspace, session.agent)?;

    let route_peer = peer.name().to_string();
    let route = std::thread::spawn(move || hostkit::ssh::resolved(&route_peer));
    let style = options.color.style();
    let source_workspace = plan::display(&session.workspace, &remote_home);
    let destination_workspace = plan::display(&destination.workspace, &home);
    let source_transcript = plan::display(&session.transcript, &remote_home);
    let destination_transcript = plan::display(&destination.transcript, &home);
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

    let bytes = fs::metadata(snapshot.path())
        .map_err(|error| format!("could not inspect the session snapshot: {error}"))?
        .len();
    let reused = verify_local_destination(snapshot.path(), &destination.transcript)?;
    if options.dry_run {
        if reused {
            println!("{}", crate::report::reused(&style));
        }
        println!("{}", crate::report::dry_run(&style));
        return Ok(());
    }

    let destination_directory = destination
        .transcript
        .parent()
        .ok_or_else(|| "the destination transcript has no parent directory".to_string())?;
    ensure_directory_tree(&home, destination_directory)?;
    if !reused {
        install_new_file(snapshot.path(), &destination.transcript)?;
    }

    if let Some(companion) = destination.companion.as_deref() {
        install_companion(remote, &remote_home, &session, &home, companion).map_err(|error| {
            format!(
                "{error}; the transcript is present at {}, but attachments were not installed",
                destination.transcript.display()
            )
        })?;
    }
    let attachment_count = crate::count_companion(destination.companion.as_deref())?;
    if reused {
        println!("{}", crate::report::reused(&style));
        if attachment_count > 0 {
            println!("{}", crate::report::attachments(&style, attachment_count));
        }
    } else {
        println!("{}", crate::report::copied(&style, bytes, attachment_count));
    }

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
    launch_local(&destination.workspace, session.agent, &session.id)
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

fn pull_snapshot(
    remote: Remote,
    remote_home: &Path,
    agent: Agent,
    session_id: &str,
) -> Result<(Snapshot, RemoteSession), String> {
    let mut last_error = String::new();
    for _ in 0..3 {
        let session = match revalidate(remote, remote_home, agent, session_id) {
            Ok(session) => session,
            Err(error) => {
                last_error = error;
                continue;
            }
        };
        let temporary = NamedTempFile::new()
            .map_err(|error| format!("could not create a temporary snapshot: {error}"))?;
        match remote
            .pull_transcript(remote_home, &session, temporary.path())
            .and_then(|()| Snapshot::from_temporary(temporary))
            .and_then(|snapshot| {
                crate::session::validate_snapshot_identity(
                    snapshot.path(),
                    session.agent,
                    &crate::session::SessionId::new(&session.id)?,
                    &session.workspace,
                )?;
                Ok(snapshot)
            }) {
            Ok(snapshot) => return Ok((snapshot, session)),
            Err(error) => last_error = error,
        }
    }
    Err(format!(
        "could not pull a valid snapshot from {} after 3 attempts: {last_error}",
        remote.peer().name()
    ))
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
    let mut input = File::open(source)
        .map_err(|error| format!("could not open {}: {error}", source.display()))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(destination).map_err(|error| {
        format!(
            "could not create destination session {}: {error}",
            destination.display()
        )
    })?;
    let result = std::io::copy(&mut input, &mut output)
        .and_then(|_| output.flush())
        .and_then(|_| output.sync_all());
    if let Err(error) = result {
        drop(output);
        let _ = fs::remove_file(destination);
        return Err(format!(
            "could not install destination session {}: {error}",
            destination.display()
        ));
    }
    Ok(())
}

fn launch_local(workspace: &Path, agent: Agent, session_id: &str) -> Result<(), String> {
    let mut command = Command::new(agent.name());
    command.current_dir(workspace);
    match agent {
        Agent::Codex => {
            command.arg("fork").arg(session_id).arg("-C").arg(workspace);
        }
        Agent::Claude => {
            command
                .arg("--resume")
                .arg(session_id)
                .arg("--fork-session");
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
mod tests {
    use super::*;

    #[test]
    fn equal_file_comparison_streams_content() {
        let directory = tempfile::tempdir().unwrap();
        let left = directory.path().join("left");
        let right = directory.path().join("right");
        fs::write(&left, b"same").unwrap();
        fs::write(&right, b"same").unwrap();
        assert!(files_equal(&left, &right).unwrap());
        fs::write(&right, b"different").unwrap();
        assert!(!files_equal(&left, &right).unwrap());
    }

    #[test]
    fn safe_directory_creation_rejects_a_file_in_the_path() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        fs::create_dir(&home).unwrap();
        fs::write(home.join("blocked"), "file").unwrap();
        let error = ensure_directory_tree(&home, &home.join("blocked/child")).unwrap_err();
        assert!(error.contains("unsafe non-directory"));
    }

    #[test]
    fn new_transcripts_are_installed_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, "one").unwrap();
        install_new_file(&source, &destination).unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), "one");
        fs::write(&source, "two").unwrap();
        assert!(install_new_file(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "one");
    }

    #[test]
    fn companion_commit_is_atomic_when_destination_is_new() {
        let directory = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir_in(directory.path()).unwrap();
        let payload = staging.path().join("payload");
        fs::create_dir(&payload).unwrap();
        fs::write(payload.join("attachment.txt"), "content").unwrap();
        let destination = directory.path().join("session");
        commit_companion(staging, &payload, &destination).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("attachment.txt")).unwrap(),
            "content"
        );
    }

    #[test]
    fn existing_companion_is_accepted_only_when_exactly_identical() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("session");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("attachment.txt"), "local").unwrap();

        let identical_staging = tempfile::tempdir_in(directory.path()).unwrap();
        let identical = identical_staging.path().join("payload");
        fs::create_dir(&identical).unwrap();
        fs::write(identical.join("attachment.txt"), "local").unwrap();
        commit_companion(identical_staging, &identical, &destination).unwrap();

        let staging = tempfile::tempdir_in(directory.path()).unwrap();
        let payload = staging.path().join("payload");
        fs::create_dir(&payload).unwrap();
        fs::write(payload.join("attachment.txt"), "remote").unwrap();
        let error = commit_companion(staging, &payload, &destination).unwrap_err();
        assert!(error.contains("different contents"));
        assert_eq!(
            fs::read_to_string(destination.join("attachment.txt")).unwrap(),
            "local"
        );
    }

    #[test]
    fn existing_companion_with_stale_extra_files_is_not_modified() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("session");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("attachment.txt"), "same").unwrap();
        fs::write(destination.join("stale.txt"), "keep").unwrap();

        let staging = tempfile::tempdir_in(directory.path()).unwrap();
        let payload = staging.path().join("payload");
        fs::create_dir(&payload).unwrap();
        fs::write(payload.join("attachment.txt"), "same").unwrap();

        let error = commit_companion(staging, &payload, &destination).unwrap_err();
        assert!(error.contains("different contents"));
        assert_eq!(
            fs::read_to_string(destination.join("attachment.txt")).unwrap(),
            "same"
        );
        assert_eq!(
            fs::read_to_string(destination.join("stale.txt")).unwrap(),
            "keep"
        );
    }
}
