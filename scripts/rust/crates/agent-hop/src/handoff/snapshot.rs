use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use hostkit::shell::quote;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tempfile::NamedTempFile;

use super::{State, error};
use crate::cli::Agent;
use crate::lineage::{Lineage, TransformedLineage};

const MAGIC: &[u8] = b"agent-hop-workspace/1\n";
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_HEADER: u64 = 16 * 1024 * 1024;
const MAX_FILES: usize = 100_000;

#[derive(Deserialize, Serialize)]
struct Entry {
    path: PathBuf,
    bytes: u64,
    sha256: String,
    mode: u32,
    link: bool,
}
#[derive(Deserialize, Serialize)]
struct Header {
    state: State,
    entries: Vec<Entry>,
    branch: Option<String>,
    origin: Option<String>,
    source_workspace: PathBuf,
}

struct Payload {
    entry: Entry,
    source: PathBuf,
}

pub(super) fn create(
    state: &State,
    id: &str,
    destination_home: &Path,
) -> Result<NamedTempFile, String> {
    create_in(state, id, destination_home, &crate::local_home()?)
}

fn create_in(
    state: &State,
    id: &str,
    destination_home: &Path,
    source_home: &Path,
) -> Result<NamedTempFile, String> {
    super::valid_id(id)?;
    let root = git(&state.workspace, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim());
    if root == source_home || root.parent().is_none() {
        return Err("refusing a home/root workspace snapshot".into());
    }
    let index = git_bytes(&root, &["ls-files", "--stage", "-z"])?;
    if index
        .split(|c| *c == 0)
        .any(|line| line.starts_with(b"160000 "))
    {
        return Err("workspace has submodules; their independent execution state cannot be handed off automatically".into());
    }
    if !git_bytes(&root, &["ls-files", "--unmerged", "-z"])?.is_empty() {
        return Err("workspace has unresolved Git merges".into());
    }
    let source_files = files(&root)?;
    let temporary = tempfile::tempdir().map_err(error)?;
    let bundle = temporary.path().join("repository.bundle");
    command(
        Command::new("git")
            .current_dir(&root)
            .args(["bundle", "create"])
            .arg(&bundle)
            .arg("HEAD"),
    )?;
    let patch = temporary.path().join("index.patch");
    fs::write(
        &patch,
        git_bytes(
            &root,
            &[
                "diff",
                "--cached",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
            ],
        )?,
    )
    .map_err(error)?;
    let destination_root = destination_home
        .join(".local/state/agent-hop/runs")
        .join(id);
    let destination_workspace = destination_root.join("workspace");
    let cwd = destination_workspace.join(state.workspace.strip_prefix(&root).map_err(error)?);
    let agent = match state.agent.as_str() {
        "codex" => Agent::Codex,
        "claude" => Agent::Claude,
        _ => return Err("invalid agent".into()),
    };
    if state.agent_home.file_name().and_then(|s| s.to_str())
        != Some(match agent {
            Agent::Codex => ".codex",
            Agent::Claude => ".claude",
        })
    {
        return Err("managed transfer requires an agent home named .codex or .claude".into());
    }
    let store_home = state.agent_home.parent().ok_or("agent home missing")?;
    let wanted = crate::session::SessionId::new(
        state
            .session
            .as_deref()
            .ok_or("managed session ID missing")?,
    )?;
    let scan = match agent {
        Agent::Codex => crate::session::scan_codex(store_home)?,
        Agent::Claude => crate::session::scan_claude(store_home)?,
    };
    if scan
        .unsafe_entries
        .iter()
        .any(|path| crate::session::path_matches(agent, path, &wanted))
    {
        return Err("managed transcript is unsafe".into());
    }
    let matches = scan
        .regular
        .into_iter()
        .filter(|path| crate::session::path_matches(agent, path, &wanted))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err("managed transcript missing or ambiguous".into());
    }
    let candidate = if agent == Agent::Codex {
        let artifact = crate::lineage::find_artifact(store_home, agent, wanted.as_str())?;
        crate::session::Candidate {
            id: wanted.clone(),
            transcript: matches[0].clone(),
            workspace: artifact.descriptor.workspace,
            modified: fs::metadata(&matches[0])
                .map_err(error)?
                .modified()
                .map_err(error)?,
        }
    } else {
        crate::session::parse_candidate(agent, matches[0].clone(), Some(&wanted))?
    };
    let session = crate::session::materialize(source_home, agent, candidate)?;
    if session.workspace != state.workspace {
        return Err(
            "agent workspace changed; restart the managed launcher in the current workspace".into(),
        );
    }
    let lineage = Lineage::discover(store_home, &session)?;
    let mapped = lineage.transform(source_home, destination_home)?;
    // Rebase structural workspace paths only; conversation text stays byte-for-byte semantic content.
    let descriptors = mapped
        .artifacts
        .iter()
        .map(|a| a.destination.clone())
        .collect();
    let original = mapped
        .artifacts
        .iter()
        .map(|a| a.source.clone())
        .collect::<Vec<_>>();
    let snapshots = mapped.artifacts.into_iter().map(|a| a.snapshot).collect();
    let mapped_root = crate::lineage::rebase_path(&root, source_home, destination_home)?;
    let mut mapped = Lineage::from_snapshots(agent, descriptors, snapshots)?
        .transform(&mapped_root, &destination_workspace)?;
    for (artifact, source) in mapped.artifacts.iter_mut().zip(original) {
        artifact.source = source;
    }
    let mut payload = vec![
        regular(&bundle, PathBuf::from("repository.bundle"))?,
        regular(&patch, PathBuf::from("index.patch"))?,
    ];
    for file in &source_files {
        source_parents(&root, file)?;
        let source = root.join(file);
        let metadata = fs::symlink_metadata(&source).map_err(error)?;
        if metadata.is_symlink() {
            let target = fs::read_link(&source).map_err(error)?;
            safe_link(file, &target)?;
            resolved_link(&root, file)?;
            let path = temporary.path().join(format!("link-{}", payload.len()));
            fs::write(&path, target.as_os_str().as_bytes()).map_err(error)?;
            let mut item = regular(&path, PathBuf::from("workspace").join(file))?;
            item.entry.link = true;
            payload.push(item);
        } else if metadata.is_file() {
            let mut input = open_source(&root, file)?;
            let before = input.metadata().map_err(error)?;
            let path = temporary.path().join(format!("file-{}", payload.len()));
            let mut output = File::create(&path).map_err(error)?;
            std::io::copy(&mut input, &mut output).map_err(error)?;
            let after = input.metadata().map_err(error)?;
            if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
                return Err("workspace file changed during snapshot".into());
            }
            let mut item = regular(&path, PathBuf::from("workspace").join(file))?;
            item.entry.mode = before.permissions().mode() & 0o777;
            payload.push(item);
        } else {
            return Err(format!("unsupported workspace entry: {}", file.display()));
        }
    }
    let agent_store = PathBuf::from("home").join(if agent == Agent::Codex {
        ".codex"
    } else {
        ".claude"
    });
    transcript_payload(&mapped, agent, &agent_store, &cwd, &mut payload)?;
    if let Some(companion) = &session.companion {
        let relative = agent_store
            .join("projects")
            .join(crate::session::claude_project_key(&cwd)?)
            .join(session.id.as_str());
        companion_payload(companion, &relative, &mut payload)?;
    }
    if payload.len() > MAX_FILES || payload.iter().map(|p| p.entry.bytes).sum::<u64>() > MAX_BYTES {
        return Err("workspace snapshot exceeds 2 GiB or 100000 files".into());
    }
    let destination_state = State {
        protocol: super::PROTOCOL,
        id: id.into(),
        agent: state.agent.clone(),
        session: state.session.clone(),
        workspace: cwd,
        agent_home: destination_root.join(&agent_store),
        phase: "starting".into(),
        pane: None,
        destination: None,
        destination_run: None,
        error: None,
        supervisor: None,
        ownership_committed: false,
        goal: state.goal.clone(),
        goal_resume_pending: false,
        goal_history: state.goal_history.clone(),
        validated_snapshot: None,
    };
    let branch = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().into());
    let origin = git(&root, &["remote", "get-url", "origin"])
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.contains("://") || !s.split("://").nth(1).unwrap_or("").contains('@'));
    let header = Header {
        state: destination_state,
        entries: payload
            .iter()
            .map(|p| Entry {
                path: p.entry.path.clone(),
                bytes: p.entry.bytes,
                sha256: p.entry.sha256.clone(),
                mode: p.entry.mode,
                link: p.entry.link,
            })
            .collect(),
        branch,
        origin,
        source_workspace: state.workspace.clone(),
    };
    let header = serde_json::to_vec(&header).map_err(error)?;
    if header.len() as u64 > MAX_HEADER {
        return Err("workspace manifest too large".into());
    }
    let mut output = NamedTempFile::new().map_err(error)?;
    output.write_all(MAGIC).map_err(error)?;
    output
        .write_all(&(header.len() as u64).to_be_bytes())
        .map_err(error)?;
    output.write_all(&header).map_err(error)?;
    for item in &payload {
        let mut input = File::open(&item.source).map_err(error)?;
        std::io::copy(&mut input, &mut output).map_err(error)?;
        if crate::transfer::sha256_file(&item.source)? != item.entry.sha256 {
            return Err(format!(
                "workspace changed during snapshot: {}",
                item.source.display()
            ));
        }
    }
    if files(&root)? != source_files || git_bytes(&root, &["ls-files", "--stage", "-z"])? != index {
        return Err("workspace changed during snapshot; source retained".into());
    }
    output.as_file_mut().sync_all().map_err(error)?;
    output.seek(SeekFrom::Start(0)).map_err(error)?;
    Ok(output)
}

fn transcript_payload(
    lineage: &TransformedLineage,
    agent: Agent,
    store: &Path,
    workspace: &Path,
    payload: &mut Vec<Payload>,
) -> Result<(), String> {
    for artifact in &lineage.artifacts {
        let relative = if agent == Agent::Codex {
            let filename = artifact
                .source
                .transcript
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("transcript filename missing")?;
            let date = filename
                .strip_prefix("rollout-")
                .and_then(|s| s.get(..10))
                .ok_or("invalid Codex rollout filename")?;
            if date.len() != 10
                || date.as_bytes()[4] != b'-'
                || date.as_bytes()[7] != b'-'
                || !date
                    .bytes()
                    .enumerate()
                    .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
            {
                return Err("invalid Codex rollout date".into());
            }
            store
                .join("sessions")
                .join(&date[..4])
                .join(&date[5..7])
                .join(&date[8..10])
                .join(filename)
        } else {
            store
                .join("projects")
                .join(crate::session::claude_project_key(workspace)?)
                .join(format!("{}.jsonl", artifact.source.session_id))
        };
        payload.push(regular(artifact.snapshot.path(), relative)?);
    }
    Ok(())
}
fn companion_payload(
    source: &Path,
    relative: &Path,
    payload: &mut Vec<Payload>,
) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(error)? {
        let entry = entry.map_err(error)?;
        let path = entry.path();
        let target = relative.join(entry.file_name());
        let kind = entry.file_type().map_err(error)?;
        if kind.is_dir() {
            companion_payload(&path, &target, payload)?;
        } else if kind.is_file() {
            payload.push(regular(&path, target)?);
        } else {
            return Err("agent attachment contains a symlink or special file".into());
        }
    }
    Ok(())
}

pub(super) fn receive(id: &str) -> Result<(), String> {
    let directory = super::new_run(id)?;
    let result = receive_stream(&directory, id, &mut std::io::stdin().lock());
    if let Err(ref error) = result {
        super::atomic_json(
            &directory.join("receive-error.json"),
            &json!({"error":error}),
        )?;
    }
    let state = result?;
    link_host_config(&state)?;
    super::save(&directory, &state)?;
    let exe = std::env::current_exe().map_err(error)?;
    let launch = format!(
        "exec {} __handoff serve --id {}",
        quote(exe.to_str().ok_or("executable path is not UTF-8")?),
        quote(id)
    );
    // This session belongs to the destination tmux server, independently of SSH and the source host.
    super::tmux(&[
        "new-session",
        "-d",
        "-s",
        &format!("ah-{id}"),
        "-c",
        state.workspace.to_str().ok_or("workspace is not UTF-8")?,
        &launch,
    ])?;
    println!(
        "{}",
        json!({"protocol":super::PROTOCOL,"phase":"starting","id":id})
    );
    Ok(())
}

fn receive_stream(directory: &Path, id: &str, input: &mut impl Read) -> Result<State, String> {
    let mut magic = vec![0; MAGIC.len()];
    input.read_exact(&mut magic).map_err(error)?;
    if magic != MAGIC {
        return Err("invalid workspace stream".into());
    }
    let mut length = [0; 8];
    input.read_exact(&mut length).map_err(error)?;
    let length = u64::from_be_bytes(length);
    if length > MAX_HEADER {
        return Err("workspace manifest too large".into());
    }
    let mut header = vec![0; length as usize];
    input.read_exact(&mut header).map_err(error)?;
    let header: Header = serde_json::from_slice(&header).map_err(error)?;
    if header.state.id != id
        || header.state.protocol != super::PROTOCOL
        || header.entries.len() > MAX_FILES
    {
        return Err("invalid workspace manifest".into());
    }
    if header
        .state
        .workspace
        .strip_prefix(directory.join("workspace"))
        .is_err()
        || !matches!(header.state.agent.as_str(), "codex" | "claude")
        || !normal_absolute(&header.state.workspace)
    {
        return Err("invalid destination workspace".into());
    }
    let expected_home = directory
        .join("home")
        .join(format!(".{}", header.state.agent));
    if header.state.agent_home != expected_home {
        return Err("invalid destination agent home".into());
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for entry in &header.entries {
        safe_relative(&entry.path)?;
        if !seen.insert(entry.path.clone()) {
            return Err("duplicate workspace entry".into());
        }
        total = total
            .checked_add(entry.bytes)
            .ok_or("workspace size overflow")?;
        if total > MAX_BYTES {
            return Err("workspace snapshot exceeds 2 GiB".into());
        }
        if entry.path != Path::new("repository.bundle")
            && entry.path != Path::new("index.patch")
            && !entry.path.starts_with("workspace")
            && !entry.path.starts_with("home/.codex/sessions")
            && !entry.path.starts_with("home/.claude/projects")
        {
            return Err("unexpected workspace entry".into());
        }
        if entry.path.components().any(|p| p.as_os_str() == ".git") {
            return Err("workspace payload contains Git metadata".into());
        }
        let path = directory.join(&entry.path);
        safe_parents(directory, path.parent().ok_or("payload parent missing")?)?;
        let mut file =
            NamedTempFile::new_in(path.parent().ok_or("payload parent missing")?).map_err(error)?;
        let copied = std::io::copy(&mut input.take(entry.bytes), &mut file).map_err(error)?;
        if copied != entry.bytes || crate::transfer::sha256_file(file.path())? != entry.sha256 {
            return Err("workspace payload hash/size mismatch".into());
        }
        if entry.link {
            let bytes = fs::read(file.path()).map_err(error)?;
            let target = PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
            let relative = entry
                .path
                .strip_prefix("workspace")
                .map_err(|_| "symlink outside workspace")?;
            safe_link(relative, &target)?;
            symlink(target, &path).map_err(error)?;
        } else {
            fs::set_permissions(
                file.path(),
                fs::Permissions::from_mode(if entry.mode & 0o111 != 0 {
                    0o700
                } else {
                    0o600
                }),
            )
            .map_err(error)?;
            file.persist_noclobber(&path).map_err(error)?;
        }
    }
    let mut trailing = [0; 1];
    if input.read(&mut trailing).map_err(error)? != 0 {
        return Err("trailing workspace data".into());
    }
    let workspace = directory.join("workspace");
    fs::create_dir_all(&workspace).map_err(error)?;
    let git_stage = directory.join("git-stage");
    command(
        Command::new("git")
            .args(["clone", "--no-checkout", "--"])
            .arg(directory.join("repository.bundle"))
            .arg(&git_stage),
    )?;
    fs::rename(git_stage.join(".git"), workspace.join(".git")).map_err(error)?;
    command(
        Command::new("git")
            .current_dir(&workspace)
            .args(["read-tree", "HEAD"]),
    )?;
    if let Some(branch) = header.branch {
        command(Command::new("git").current_dir(&workspace).args([
            "check-ref-format",
            "--branch",
            &branch,
        ]))?;
        command(Command::new("git").current_dir(&workspace).args([
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{branch}"),
        ]))?;
        // Restore the branch ref at the bundled commit without touching working files.
        let bundle_head = git(
            &workspace,
            &[
                "bundle",
                "list-heads",
                directory
                    .join("repository.bundle")
                    .to_str()
                    .ok_or("bundle path is not UTF-8")?,
            ],
        )?;
        let commit = bundle_head
            .split_whitespace()
            .next()
            .ok_or("bundle has no HEAD")?;
        command(
            Command::new("git")
                .current_dir(&workspace)
                .args(["update-ref", "HEAD", commit]),
        )?;
    }
    if fs::metadata(directory.join("index.patch"))
        .map_err(error)?
        .len()
        > 0
    {
        command(
            Command::new("git")
                .current_dir(&workspace)
                .args(["apply", "--cached", "--binary", "--whitespace=nowarn"])
                .arg(directory.join("index.patch")),
        )?;
    }
    command(
        Command::new("git")
            .current_dir(&workspace)
            .args(["remote", "remove", "origin"]),
    )?;
    if let Some(origin) = header.origin {
        command(
            Command::new("git")
                .current_dir(&workspace)
                .args(["remote", "add", "origin", &origin]),
        )?;
    }
    if !header.state.workspace.is_dir() {
        return Err("session directory missing from snapshot".into());
    }
    for entry in &header.entries {
        if entry.link {
            resolved_link(
                &workspace,
                entry.path.strip_prefix("workspace").map_err(error)?,
            )?;
        }
    }
    let mut state = header.state;
    state.phase = "starting".into();
    state.pane = None;
    state.destination = None;
    state.destination_run = None;
    state.error = None;
    state.supervisor = None;
    state.ownership_committed = false;
    state.goal_resume_pending = false;
    state.validated_snapshot = Some(fs::canonicalize(&workspace).map_err(error)?);
    Ok(state)
}

fn link_host_config(state: &State) -> Result<(), String> {
    fs::create_dir_all(&state.agent_home).map_err(error)?;
    let host = crate::local_home()?.join(format!(".{}", state.agent));
    let files: &[&str] = if state.agent == "codex" {
        &[
            "config.toml",
            "auth.json",
            "hooks.json",
            "rules",
            "skills",
            "plugins",
            "AGENTS.md",
        ]
    } else {
        &[
            "settings.json",
            ".credentials.json",
            "CLAUDE.md",
            "skills",
            "plugins",
            "commands",
            "agents",
        ]
    };
    // References point only to this destination user's existing settings/auth; nothing secret crosses SSH.
    for name in files {
        let source = host.join(name);
        if source.exists() {
            symlink(source, state.agent_home.join(name)).map_err(error)?;
        }
    }
    Ok(())
}

fn regular(source: &Path, path: PathBuf) -> Result<Payload, String> {
    let metadata = fs::metadata(source).map_err(error)?;
    Ok(Payload {
        entry: Entry {
            path,
            bytes: metadata.len(),
            sha256: crate::transfer::sha256_file(source)?,
            mode: metadata.permissions().mode() & 0o777,
            link: false,
        },
        source: source.into(),
    })
}
fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = git_bytes(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?
    .split(|c| *c == 0)
    .filter(|s| !s.is_empty())
    .map(|s| PathBuf::from(std::ffi::OsStr::from_bytes(s)))
    .filter(|p| fs::symlink_metadata(root.join(p)).is_ok())
    .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    for file in &files {
        safe_relative(file)?;
        source_parents(root, file)?;
    }
    Ok(files)
}
fn source_parents(root: &Path, relative: &Path) -> Result<(), String> {
    let mut path = root.to_path_buf();
    if let Some(parent) = relative.parent() {
        for part in parent.components() {
            path.push(part);
            let meta = fs::symlink_metadata(&path).map_err(error)?;
            if !meta.is_dir() || meta.is_symlink() {
                return Err(format!(
                    "workspace path has a symlink parent: {}",
                    relative.display()
                ));
            }
        }
    }
    Ok(())
}
fn resolved_link(root: &Path, relative: &Path) -> Result<(), String> {
    let resolved = fs::canonicalize(root.join(relative))
        .map_err(|e| format!("workspace symlink target unavailable: {e}"))?;
    if !resolved.starts_with(root) {
        return Err(format!(
            "workspace symlink escapes repository: {}",
            relative.display()
        ));
    }
    Ok(())
}

#[allow(unsafe_code)]
fn open_source(root: &Path, relative: &Path) -> Result<File, String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    safe_relative(relative)?;
    let path = CString::new(root.as_os_str().as_bytes()).map_err(error)?;
    let root_fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(error(std::io::Error::last_os_error()));
    }
    let mut directory = unsafe { OwnedFd::from_raw_fd(root_fd) };
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let name = CString::new(component.as_os_str().as_bytes()).map_err(error)?;
        let last = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if last { 0 } else { libc::O_DIRECTORY };
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(format!(
                "unsafe workspace read {}: {}",
                relative.display(),
                std::io::Error::last_os_error()
            ));
        }
        directory = unsafe { OwnedFd::from_raw_fd(descriptor) };
    }
    let file = File::from(directory);
    if !file.metadata().map_err(error)?.is_file() {
        return Err("workspace entry is not a regular file".into());
    }
    Ok(file)
}
fn safe_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(format!("unsafe workspace path: {}", path.display()));
    }
    Ok(())
}
fn normal_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
fn safe_link(path: &Path, target: &Path) -> Result<(), String> {
    if target.is_absolute() {
        return Err(format!("absolute workspace symlink: {}", path.display()));
    }
    let mut depth = path.parent().map(|p| p.components().count()).unwrap_or(0);
    for component in target.components() {
        match component {
            Component::ParentDir if depth == 0 => {
                return Err(format!(
                    "workspace symlink escapes repository: {}",
                    path.display()
                ));
            }
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return Err("unsafe symlink".into()),
        }
    }
    Ok(())
}
fn safe_parents(root: &Path, parent: &Path) -> Result<(), String> {
    let mut path = root.to_path_buf();
    for part in parent.strip_prefix(root).map_err(error)?.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {}
            Ok(_) => return Err("payload parent is not a safe directory".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(error)?
            }
            Err(e) => return Err(error(e)),
        }
    }
    Ok(())
}
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    String::from_utf8(git_bytes(root, args)?).map_err(error)
}
fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    command(Command::new("git").current_dir(root).args(args))
}
fn command(command: &mut Command) -> Result<Vec<u8>, String> {
    let result = command
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", "/dev/null")
        .stdin(Stdio::null())
        .output()
        .map_err(error)?;
    if !result.status.success() {
        return Err(format!(
            "Git snapshot: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(result.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, State, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let home = base.join("source");
        let destination = base.join("destination");
        let workspace = home.join("project with spaces");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&destination).unwrap();
        git(&workspace, &["init", "-b", "feature/handoff"]).unwrap();
        git(&workspace, &["config", "user.name", "Test"]).unwrap();
        git(
            &workspace,
            &["config", "user.email", "test@example.invalid"],
        )
        .unwrap();
        fs::write(workspace.join("tracked"), "base\n").unwrap();
        fs::write(workspace.join("deleted"), "delete me\n").unwrap();
        fs::write(workspace.join(".gitignore"), ".env\n").unwrap();
        git(&workspace, &["add", "."]).unwrap();
        git(&workspace, &["commit", "-m", "initial"]).unwrap();
        fs::write(workspace.join("tracked"), "staged\n").unwrap();
        git(&workspace, &["add", "tracked"]).unwrap();
        fs::write(workspace.join("tracked"), "working\n").unwrap();
        fs::remove_file(workspace.join("deleted")).unwrap();
        fs::write(workspace.join("untracked"), "new\n").unwrap();
        fs::write(workspace.join(".env"), "do not transfer").unwrap();
        symlink("tracked", workspace.join("link")).unwrap();
        let agent_home = home.join(".claude");
        let path = agent_home
            .join("projects")
            .join(crate::session::claude_project_key(&workspace).unwrap())
            .join("test-session.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path,format!("{}\n",json!({"sessionId":"test-session","cwd":workspace,"type":"user","message":{"role":"user","content":"hello"}}))).unwrap();
        let state = State {
            protocol: super::super::PROTOCOL,
            id: "source-run".into(),
            agent: "claude".into(),
            session: Some("test-session".into()),
            workspace,
            agent_home,
            phase: "running".into(),
            pane: None,
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
        (temp, state, home, destination)
    }
    #[test]
    fn snapshot_roundtrip_preserves_dirty_index_deletions_untracked_and_symlinks() {
        let (_temp, state, home, destination) = fixture();
        let mut archive = create_in(&state, "destination-run", &destination, &home).unwrap();
        let directory = destination.join(".local/state/agent-hop/runs/destination-run");
        fs::create_dir_all(&directory).unwrap();
        let received =
            receive_stream(&directory, "destination-run", archive.as_file_mut()).unwrap();
        let workspace = &received.workspace;
        assert_eq!(
            fs::read_to_string(workspace.join("tracked")).unwrap(),
            "working\n"
        );
        assert_eq!(git(workspace, &["show", ":tracked"]).unwrap(), "staged\n");
        assert!(!workspace.join("deleted").exists());
        assert!(workspace.join("untracked").exists());
        assert!(!workspace.join(".env").exists());
        assert_eq!(
            fs::read_link(workspace.join("link")).unwrap(),
            PathBuf::from("tracked")
        );
        assert_eq!(
            git(workspace, &["branch", "--show-current"])
                .unwrap()
                .trim(),
            "feature/handoff"
        );
        let scan = crate::session::scan_claude(received.agent_home.parent().unwrap()).unwrap();
        assert_eq!(scan.regular.len(), 1);
        assert!(
            fs::read_to_string(&scan.regular[0])
                .unwrap()
                .contains(workspace.to_str().unwrap())
        );
        // A second move from the isolated destination must use its private transcript store.
        let next = destination.join("next-host");
        fs::create_dir_all(&next).unwrap();
        create_in(&received, "next-run", &next, &destination).unwrap();
    }
    #[test]
    fn corrupt_snapshot_never_overwrites_destination_files() {
        let (_temp, state, home, destination) = fixture();
        let mut archive = create_in(&state, "destination-run", &destination, &home).unwrap();
        let directory = destination.join(".local/state/agent-hop/runs/destination-run");
        fs::create_dir_all(directory.join("workspace")).unwrap();
        fs::write(directory.join("workspace/tracked"), "existing").unwrap();
        assert!(receive_stream(&directory, "destination-run", archive.as_file_mut()).is_err());
        assert_eq!(
            fs::read_to_string(directory.join("workspace/tracked")).unwrap(),
            "existing"
        );
    }
    #[test]
    fn links_cannot_escape_the_workspace() {
        assert!(safe_link(Path::new("lib/link"), Path::new("../src")).is_ok());
        assert!(safe_link(Path::new("link"), Path::new("../outside")).is_err());
        assert!(safe_link(Path::new("lib/link"), Path::new("/etc/passwd")).is_err());
    }
    #[test]
    fn source_parent_symlink_cannot_export_an_outside_file() {
        let (_temp, state, home, destination) = fixture();
        fs::create_dir(state.workspace.join("dir")).unwrap();
        fs::write(state.workspace.join("dir/file"), "tracked").unwrap();
        git(&state.workspace, &["add", "dir/file"]).unwrap();
        fs::rename(state.workspace.join("dir"), home.join("outside")).unwrap();
        symlink(home.join("outside"), state.workspace.join("dir")).unwrap();
        fs::write(state.workspace.join(".git/info/exclude"), "dir\n").unwrap();
        fs::write(home.join("outside/file"), "private canary").unwrap();
        assert!(create_in(&state, "destination-run", &destination, &home).is_err());
        assert!(open_source(&state.workspace, Path::new("dir/file")).is_err());
    }
    #[test]
    fn chained_symlinks_cannot_escape_after_dotdot_resolution() {
        let (_temp, state, home, destination) = fixture();
        fs::write(home.join("outside"), "private canary").unwrap();
        symlink(".", state.workspace.join("a")).unwrap();
        symlink("a/../outside", state.workspace.join("chain")).unwrap();
        assert!(safe_link(Path::new("chain"), Path::new("a/../outside")).is_ok());
        assert!(resolved_link(&state.workspace, Path::new("chain")).is_err());
        assert!(create_in(&state, "destination-run", &destination, &home).is_err());
    }
    #[test]
    fn recovered_snapshot_trust_rejects_replaced_root_directory() {
        let (_temp, state, home, destination) = fixture();
        let mut archive = create_in(&state, "destination-run", &destination, &home).unwrap();
        let directory = destination.join(".local/state/agent-hop/runs/destination-run");
        fs::create_dir_all(&directory).unwrap();
        let received =
            receive_stream(&directory, "destination-run", archive.as_file_mut()).unwrap();
        assert!(super::super::validated_snapshot(&received, &directory).unwrap());
        fs::rename(
            directory.join("workspace"),
            directory.join("original-workspace"),
        )
        .unwrap();
        let outside = destination.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, directory.join("workspace")).unwrap();
        assert!(super::super::validated_snapshot(&received, &directory).is_err());
    }
    #[test]
    fn paths_must_be_normal_relative() {
        for path in ["/etc/passwd", "../x", "a/../../x", ""] {
            assert!(safe_relative(Path::new(path)).is_err());
        }
        assert!(safe_relative(Path::new("space here/файл")).is_ok());
    }
}
