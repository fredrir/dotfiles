#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use hostkit::Host;

pub mod cli;
mod plan;
mod remote;
mod report;
mod session;
mod transfer;

use cli::Cli;
use remote::Remote;
use transfer::Snapshot;

pub fn run(cli: Cli) -> Result<(), String> {
    let agent = cli
        .agent
        .ok_or_else(|| "choose codex or claude".to_string())?;
    let this = Host::this()?;
    let peer = this.peer();
    let home = local_home()?;
    let current_cwd = physical_current_directory()?;
    let session = session::discover(&home, &current_cwd, agent, cli.session_id.as_deref())?;

    let route_peer = peer.name().to_owned();
    let route = std::thread::spawn(move || hostkit::ssh::resolved(&route_peer));
    let remote = Remote::new(peer);
    let remote_home = remote.home()?;
    let destination = plan::destination(
        session.agent,
        session.id.as_str(),
        &session.transcript,
        &session.workspace,
        &home,
        &remote_home,
        session.companion.is_some(),
    )?;
    remote.preflight(&destination.workspace, session.agent)?;

    let style = cli.color.style();
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

    let attachment_count = count_companion(session.companion.as_deref())?;
    if cli.dry_run {
        let reused = verify_destination(remote, &session.transcript, &destination.transcript)?;
        if reused {
            println!("{}", report::reused(&style));
        }
        println!("{}", report::dry_run(&style));
        return Ok(());
    }

    let snapshot = Snapshot::create(&session.transcript)?;
    let bytes = fs::metadata(snapshot.path())
        .map_err(|error| format!("could not inspect the session snapshot: {error}"))?
        .len();
    let reused = verify_destination(remote, snapshot.path(), &destination.transcript)?;
    let destination_directory = destination
        .transcript
        .parent()
        .ok_or_else(|| "the destination transcript has no parent directory".to_string())?;
    remote.mkdir(destination_directory)?;
    if !reused {
        transfer::copy_transcript(peer, snapshot.path(), &destination.transcript)?;
    }
    if let (Some(source), Some(destination)) = (
        session.companion.as_deref(),
        destination.companion.as_deref(),
    ) {
        transfer::copy_companion(peer, source, destination)?;
    }
    if reused {
        println!("{}", report::reused(&style));
        if attachment_count > 0 {
            println!("{}", report::attachments(&style, attachment_count));
        }
    } else {
        println!("{}", report::copied(&style, bytes, attachment_count));
    }

    if cli.no_connect {
        println!("{}", report::copied_without_connect(&style, peer.name()));
        return Ok(());
    }
    println!("{}", report::launching(&style, session.agent, peer.name()));
    remote.launch(&destination.workspace, session.agent, session.id.as_str())
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
}
