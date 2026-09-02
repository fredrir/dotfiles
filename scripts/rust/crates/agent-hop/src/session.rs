use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::cli::Agent;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct SessionId(String);

impl SessionId {
    pub(crate) fn new(value: &str) -> Result<Self, String> {
        let path = Path::new(value);
        let mut components = path.components();
        let safe_component = matches!(components.next(), Some(Component::Normal(component)) if component == OsStr::new(value))
            && components.next().is_none();
        if value.is_empty()
            || value == "."
            || value == ".."
            || value.contains('/')
            || value.contains('\\')
            || value.contains('\0')
            || value.chars().any(char::is_control)
            || !safe_component
        {
            return Err(format!("invalid session ID: {value:?}"));
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Session {
    pub(crate) agent: Agent,
    pub(crate) id: SessionId,
    pub(crate) transcript: PathBuf,
    pub(crate) companion: Option<PathBuf>,
    pub(crate) workspace: PathBuf,
}

#[derive(Default)]
pub(crate) struct Scan {
    pub(crate) regular: Vec<PathBuf>,
    pub(crate) unsafe_entries: Vec<PathBuf>,
    pub(crate) errors: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct Candidate {
    pub(crate) id: SessionId,
    pub(crate) transcript: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) modified: SystemTime,
}

pub(crate) fn discover(
    home: &Path,
    current_cwd: &Path,
    agent: Agent,
    requested: Option<&str>,
) -> Result<Session, String> {
    let scan = match agent {
        Agent::Codex => scan_codex(home)?,
        Agent::Claude => scan_claude(home)?,
    };
    match requested {
        Some(value) => discover_requested(home, agent, scan, SessionId::new(value)?),
        None => discover_latest(home, current_cwd, agent, scan),
    }
}

pub(crate) fn workspace_relative(home: &Path, cwd: &Path) -> Result<PathBuf, String> {
    if !home.is_absolute() || !cwd.is_absolute() {
        return Err(format!(
            "the session workspace must be absolute: {}",
            cwd.display()
        ));
    }
    if !is_normal_absolute(home) || !is_normal_absolute(cwd) {
        return Err(format!(
            "the session workspace is not a normalized path: {}",
            cwd.display()
        ));
    }
    cwd.strip_prefix(home).map(Path::to_path_buf).map_err(|_| {
        format!(
            "the session workspace is outside your home directory: {}",
            cwd.display()
        )
    })
}

pub(crate) fn claude_project_key(destination_cwd: &Path) -> Result<String, String> {
    if !destination_cwd.is_absolute() || !is_normal_absolute(destination_cwd) {
        return Err(format!(
            "the destination workspace is not an absolute normalized path: {}",
            destination_cwd.display()
        ));
    }
    let value = destination_cwd.to_str().ok_or_else(|| {
        format!(
            "the destination workspace is not valid UTF-8: {}",
            destination_cwd.display()
        )
    })?;
    Ok(value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect())
}

fn is_normal_absolute(path: &Path) -> bool {
    path.components()
        .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
}

fn discover_requested(
    home: &Path,
    agent: Agent,
    scan: Scan,
    requested: SessionId,
) -> Result<Session, String> {
    let unsafe_match = scan
        .unsafe_entries
        .iter()
        .find(|path| path_matches(agent, path, &requested));
    if let Some(path) = unsafe_match {
        return Err(format!(
            "session {} resolves to an unsafe non-regular file: {}",
            requested,
            path.display()
        ));
    }
    let mut matches = scan
        .regular
        .into_iter()
        .filter(|path| path_matches(agent, path, &requested))
        .collect::<Vec<_>>();
    matches.sort();
    match matches.len() {
        0 => return Err(not_found(agent, &requested)),
        1 => {}
        count => {
            return Err(format!(
                "session {} is ambiguous: found {count} transcript files",
                requested
            ));
        }
    }
    let transcript = matches.pop().unwrap();
    let candidate = parse_candidate(agent, transcript, Some(&requested)).map_err(|error| {
        format!(
            "{} session {} is not resumable: {error}",
            agent_label(agent),
            requested
        )
    })?;
    if candidate.id != requested {
        return Err(format!(
            "{} session {} is not resumable: transcript metadata names session {}",
            agent_label(agent),
            requested,
            candidate.id
        ));
    }
    materialize(home, agent, candidate)
}

fn discover_latest(
    home: &Path,
    current_cwd: &Path,
    agent: Agent,
    scan: Scan,
) -> Result<Session, String> {
    let mut candidates = scan
        .regular
        .into_iter()
        .filter_map(|path| parse_candidate(agent, path, None).ok())
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.transcript.cmp(&right.transcript));
    let mut best: Option<Candidate> = None;
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.workspace == current_cwd)
    {
        let replace = best
            .as_ref()
            .is_none_or(|selected| candidate.modified > selected.modified);
        if replace {
            best = Some(candidate.clone());
        }
    }
    let best = best.ok_or_else(|| {
        format!(
            "no {} session found for {}",
            agent_label(agent),
            current_cwd.display()
        )
    })?;
    let duplicate_count = candidates
        .iter()
        .filter(|candidate| candidate.id == best.id)
        .count();
    if duplicate_count > 1 {
        return Err(format!(
            "session {} is ambiguous: found {duplicate_count} transcript files",
            best.id
        ));
    }
    materialize(home, agent, best)
}

pub(crate) fn parse_candidate(
    agent: Agent,
    transcript: PathBuf,
    expected: Option<&SessionId>,
) -> Result<Candidate, String> {
    let (id, workspace) = match agent {
        Agent::Codex => parse_codex(&transcript)?.ok_or_else(|| {
            "the first record is not a user-authored Codex CLI session".to_owned()
        })?,
        Agent::Claude => {
            let filename_id = claude_filename_id(&transcript)?;
            if expected.is_some_and(|expected| expected != &filename_id) {
                return Err("the filename does not match the requested session ID".to_owned());
            }
            let workspace = parse_claude(&transcript, &filename_id)?;
            (filename_id, workspace)
        }
    };
    if !path_matches(agent, &transcript, &id) {
        return Err("the filename does not match the session metadata ID".to_owned());
    }
    let metadata = fs::symlink_metadata(&transcript)
        .map_err(|error| format!("could not inspect {}: {error}", transcript.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "the transcript is not a regular file: {}",
            transcript.display()
        ));
    }
    let modified = metadata
        .modified()
        .map_err(|error| format!("could not read the transcript modification time: {error}"))?;
    Ok(Candidate {
        id,
        transcript,
        workspace,
        modified,
    })
}

fn parse_codex(path: &Path) -> Result<Option<(SessionId, PathBuf)>, String> {
    let file =
        File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let bytes = reader
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes == 0 {
        return Err("the transcript is empty".to_owned());
    }
    let record: Value = serde_json::from_slice(&line)
        .map_err(|error| format!("the first transcript record is invalid JSON: {error}"))?;
    if record.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Ok(None);
    }
    let Some(payload) = record.get("payload") else {
        return Err("the Codex session metadata has no payload".to_owned());
    };
    if payload.get("thread_source").and_then(Value::as_str) != Some("user")
        || payload.get("source").and_then(Value::as_str) != Some("cli")
    {
        return Ok(None);
    }
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "the Codex session metadata has no string ID".to_owned())?;
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| "the Codex session metadata has no string workspace".to_owned())?;
    Ok(Some((SessionId::new(id)?, PathBuf::from(cwd))))
}

fn parse_claude(path: &Path, filename_id: &SessionId) -> Result<PathBuf, String> {
    let file =
        File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        if bytes == 0 {
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record: Value = serde_json::from_slice(&line)
            .map_err(|error| format!("a transcript record is invalid JSON: {error}"))?;
        if let Some(value) = record.get("sessionId") {
            let value = value
                .as_str()
                .ok_or_else(|| "Claude sessionId metadata is not a string".to_owned())?;
            let record_id = SessionId::new(value)?;
            if &record_id != filename_id {
                return Err(format!(
                    "transcript metadata names session {record_id}, not {filename_id}"
                ));
            }
        }
        let Some(cwd) = record.get("cwd") else {
            continue;
        };
        if cwd.is_null() || record.get("isSidechain") == Some(&Value::Bool(true)) {
            continue;
        }
        let cwd = cwd
            .as_str()
            .ok_or_else(|| "Claude workspace metadata is not a string".to_owned())?;
        return Ok(PathBuf::from(cwd));
    }
    Err("the Claude Code session has no workspace".to_owned())
}

pub(crate) fn validate_snapshot_identity(
    path: &Path,
    agent: Agent,
    expected_id: &SessionId,
    expected_workspace: &Path,
) -> Result<(), String> {
    let (id, workspace) = match agent {
        Agent::Codex => parse_codex(path)?
            .ok_or_else(|| "the exported transcript is not a user Codex CLI session".to_string())?,
        Agent::Claude => parse_claude_snapshot(path, expected_id)?,
    };
    if &id != expected_id {
        return Err(format!(
            "the exported transcript names session {id}, not {expected_id}"
        ));
    }
    if workspace != expected_workspace {
        return Err("the exported transcript workspace changed during transfer".to_string());
    }
    Ok(())
}

fn parse_claude_snapshot(
    path: &Path,
    expected_id: &SessionId,
) -> Result<(SessionId, PathBuf), String> {
    let file =
        File::open(path).map_err(|error| format!("could not open exported transcript: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut workspace = None;
    let mut saw_id = false;
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not read exported transcript: {error}"))?
            == 0
        {
            break;
        }
        let record: Value = serde_json::from_slice(&line)
            .map_err(|error| format!("an exported transcript record is invalid: {error}"))?;
        if let Some(value) = record.get("sessionId") {
            let id = value
                .as_str()
                .ok_or_else(|| "exported Claude sessionId is not a string".to_string())?;
            if SessionId::new(id)? != *expected_id {
                return Err("the exported Claude transcript has a different session ID".to_string());
            }
            saw_id = true;
        }
        if workspace.is_none()
            && record.get("isSidechain") != Some(&Value::Bool(true))
            && let Some(cwd) = record.get("cwd").and_then(Value::as_str)
        {
            workspace = Some(PathBuf::from(cwd));
        }
    }
    if !saw_id {
        return Err("the exported Claude transcript has no session ID".to_string());
    }
    Ok((
        expected_id.clone(),
        workspace.ok_or_else(|| "the exported Claude transcript has no workspace".to_string())?,
    ))
}

pub(crate) fn materialize(
    home: &Path,
    agent: Agent,
    candidate: Candidate,
) -> Result<Session, String> {
    workspace_relative(home, &candidate.workspace)?;
    let metadata = fs::symlink_metadata(&candidate.transcript).map_err(|error| {
        format!(
            "could not inspect {}: {error}",
            candidate.transcript.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "the transcript is not a regular file: {}",
            candidate.transcript.display()
        ));
    }
    let companion = if agent == Agent::Claude {
        let path = candidate.transcript.with_extension("");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => Some(path),
            Ok(_) => {
                return Err(format!(
                    "the Claude session companion is not a safe directory: {}",
                    path.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "could not inspect Claude session companion {}: {error}",
                    path.display()
                ));
            }
        }
    } else {
        None
    };
    Ok(Session {
        agent,
        id: candidate.id,
        transcript: candidate.transcript,
        companion,
        workspace: candidate.workspace,
    })
}

pub(crate) fn scan_codex(home: &Path) -> Result<Scan, String> {
    let root = home.join(".codex/sessions");
    require_safe_directory(&root, "Codex session store")?;
    let mut directories = vec![root];
    let mut scan = Scan::default();
    for _ in 0..3 {
        let mut next = Vec::new();
        for directory in directories {
            let entries = match sorted_entries(&directory) {
                Ok(entries) => entries,
                Err(error) => {
                    scan.errors.push(error);
                    continue;
                }
            };
            for entry in entries {
                match entry.file_type() {
                    Ok(file_type) if file_type.is_dir() => next.push(entry.path()),
                    Ok(_) => {}
                    Err(error) => scan.errors.push(format!(
                        "could not inspect {}: {error}",
                        entry.path().display()
                    )),
                }
            }
        }
        directories = next;
    }
    Ok(scan_jsonl_files(directories, scan))
}

pub(crate) fn scan_claude(home: &Path) -> Result<Scan, String> {
    let root = home.join(".claude/projects");
    require_safe_directory(&root, "Claude session store")?;
    let mut scan = Scan::default();
    let mut projects = Vec::new();
    match sorted_entries(&root) {
        Ok(entries) => {
            for entry in entries {
                match entry.file_type() {
                    Ok(file_type) if file_type.is_dir() => projects.push(entry.path()),
                    Ok(_) => {}
                    Err(error) => scan.errors.push(format!(
                        "could not inspect {}: {error}",
                        entry.path().display()
                    )),
                }
            }
        }
        Err(error) => scan.errors.push(error),
    }
    Ok(scan_jsonl_files(projects, scan))
}

fn require_safe_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("{label} is unavailable at {}: {error}", path.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "{label} is not a safe directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn sorted_entries(directory: &Path) -> Result<Vec<fs::DirEntry>, String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not read {}: {error}", directory.display()))?;
    entries.sort_by_key(fs::DirEntry::path);
    Ok(entries)
}

fn scan_jsonl_files(directories: Vec<PathBuf>, mut scan: Scan) -> Scan {
    for directory in directories {
        let entries = match sorted_entries(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                scan.errors.push(error);
                continue;
            }
        };
        for entry in entries {
            let path = entry.path();
            if path.extension() != Some(OsStr::new("jsonl")) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    scan.errors
                        .push(format!("could not inspect {}: {error}", path.display()));
                    continue;
                }
            };
            if file_type.is_file() {
                scan.regular.push(path);
            } else {
                scan.unsafe_entries.push(path);
            }
        }
    }
    scan.regular.sort();
    scan.unsafe_entries.sort();
    scan
}

pub(crate) fn path_matches(agent: Agent, path: &Path, id: &SessionId) -> bool {
    let Some(filename) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let Some(stem) = filename.strip_suffix(".jsonl") else {
        return false;
    };
    match agent {
        Agent::Codex => stem
            .strip_suffix(id.as_str())
            .is_some_and(|prefix| prefix.ends_with('-')),
        Agent::Claude => stem == id.as_str(),
    }
}

fn claude_filename_id(path: &Path) -> Result<SessionId, String> {
    let filename = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "the Claude transcript filename is not valid UTF-8".to_owned())?;
    let value = filename
        .strip_suffix(".jsonl")
        .ok_or_else(|| "the Claude transcript filename does not end in .jsonl".to_owned())?;
    SessionId::new(value)
}

fn not_found(agent: Agent, id: &SessionId) -> String {
    format!("{} session {id} was not found", agent_label(agent))
}

fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "Codex",
        Agent::Claude => "Claude Code",
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn home() -> (TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir_all(&home).unwrap();
        (temporary, home)
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn codex_path(home: &Path, day: &str, id: &str) -> PathBuf {
        home.join(format!(
            ".codex/sessions/2026/09/{day}/rollout-2026-09-{day}T00-00-00-{id}.jsonl"
        ))
    }

    fn codex_record(id: &str, cwd: &Path, thread_source: Value, source: Value) -> String {
        format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "ordinal": 0,
                "payload": {
                    "id": id,
                    "cwd": cwd,
                    "thread_source": thread_source,
                    "source": source,
                    "future_field": {"is": "ignored"}
                }
            })
        )
    }

    fn claude_path(home: &Path, project: &str, id: &str) -> PathBuf {
        home.join(format!(".claude/projects/{project}/{id}.jsonl"))
    }

    fn set_modified(path: &Path, seconds: u64) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn unreadable_or_missing_sibling_directories_do_not_hide_healthy_sessions() {
        let (temporary, home) = home();
        let healthy = temporary.path().join("healthy");
        fs::create_dir(&healthy).unwrap();
        let transcript = healthy.join("session.jsonl");
        fs::write(&transcript, "{}\n").unwrap();
        let scan = scan_jsonl_files(
            vec![temporary.path().join("missing"), healthy],
            Scan::default(),
        );
        assert_eq!(scan.regular, [transcript]);
        assert_eq!(scan.errors.len(), 1);
        drop(home);
    }

    #[test]
    fn session_ids_are_opaque_safe_path_components() {
        for value in [
            "01999999-1111-7222-8333-444444444444",
            "future.id",
            "id with spaces",
        ] {
            assert_eq!(SessionId::new(value).unwrap().as_str(), value);
        }
        for value in ["", ".", "..", "a/b", "a\\b", "a\0b", "a\nb"] {
            assert!(SessionId::new(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn workspace_mapping_is_component_safe() {
        let home = Path::new("/home/fredrir");
        assert_eq!(workspace_relative(home, home).unwrap(), Path::new(""));
        assert_eq!(
            workspace_relative(home, Path::new("/home/fredrir/src/project")).unwrap(),
            Path::new("src/project")
        );
        assert!(workspace_relative(home, Path::new("/home/fredrir-other")).is_err());
        assert!(workspace_relative(home, Path::new("/home/fredrir/../other")).is_err());
        assert!(workspace_relative(home, Path::new("relative")).is_err());
    }

    #[test]
    fn claude_project_keys_match_the_cli_encoding() {
        assert_eq!(
            claude_project_key(Path::new("/home/fredrir/src/my-project_2")).unwrap(),
            "-home-fredrir-src-my-project_2"
        );
        assert_eq!(
            claude_project_key(Path::new("/Users/fréd/project")).unwrap(),
            "-Users-fr-d-project"
        );
        assert!(claude_project_key(Path::new("relative/path")).is_err());
    }

    #[test]
    fn codex_latest_uses_only_user_cli_sessions() {
        let (_temporary, home) = home();
        let workspace = home.join("src/project");
        fs::create_dir_all(&workspace).unwrap();
        let cli_id = "01999999-1111-7222-8333-444444444441";
        let cli = codex_path(&home, "01", cli_id);
        write(
            &cli,
            &codex_record(cli_id, &workspace, json!("user"), json!("cli")),
        );
        let vscode_id = "01999999-1111-7222-8333-444444444442";
        let vscode = codex_path(&home, "02", vscode_id);
        write(
            &vscode,
            &codex_record(vscode_id, &workspace, json!("user"), json!("vscode")),
        );
        let child_id = "01999999-1111-7222-8333-444444444443";
        let child = codex_path(&home, "03", child_id);
        write(
            &child,
            &codex_record(
                child_id,
                &workspace,
                json!("subagent"),
                json!({"subagent": {"future": true}}),
            ),
        );
        set_modified(&cli, 1);
        set_modified(&vscode, 3);
        set_modified(&child, 4);
        let session = discover(&home, &workspace, Agent::Codex, None).unwrap();
        assert_eq!(session.id.as_str(), cli_id);
        assert_eq!(session.transcript, cli);
        assert_eq!(session.agent, Agent::Codex);
        assert!(session.companion.is_none());
    }

    #[test]
    fn latest_selection_uses_mtime_and_a_stable_path_tie_break() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        fs::create_dir_all(&workspace).unwrap();
        let first_id = "01999999-1111-7222-8333-444444444441";
        let second_id = "01999999-1111-7222-8333-444444444442";
        let first = codex_path(&home, "01", first_id);
        let second = codex_path(&home, "02", second_id);
        write(
            &first,
            &codex_record(first_id, &workspace, json!("user"), json!("cli")),
        );
        write(
            &second,
            &codex_record(second_id, &workspace, json!("user"), json!("cli")),
        );
        set_modified(&first, 7);
        set_modified(&second, 7);
        assert_eq!(
            discover(&home, &workspace, Agent::Codex, None)
                .unwrap()
                .id
                .as_str(),
            first_id
        );
        set_modified(&second, 8);
        assert_eq!(
            discover(&home, &workspace, Agent::Codex, None)
                .unwrap()
                .id
                .as_str(),
            second_id
        );
    }

    #[test]
    fn codex_requires_metadata_on_the_first_physical_line() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        let id = "01999999-1111-7222-8333-444444444441";
        let path = codex_path(&home, "01", id);
        write(
            &path,
            &format!(
                "\n{}",
                codex_record(id, &workspace, json!("user"), json!("cli"))
            ),
        );
        assert!(discover(&home, &workspace, Agent::Codex, Some(id)).is_err());
    }

    #[test]
    fn codex_explicit_discovery_refuses_metadata_and_filename_mismatch() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        let requested = "01999999-1111-7222-8333-444444444441";
        let other = "01999999-1111-7222-8333-444444444442";
        write(
            &codex_path(&home, "01", requested),
            &codex_record(other, &workspace, json!("user"), json!("cli")),
        );
        let error = discover(&home, &workspace, Agent::Codex, Some(requested)).unwrap_err();
        assert!(error.contains("not resumable"));
    }

    #[test]
    fn duplicate_explicit_codex_ids_are_refused() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        let id = "01999999-1111-7222-8333-444444444441";
        for day in ["01", "02"] {
            write(
                &codex_path(&home, day, id),
                &codex_record(id, &workspace, json!("user"), json!("cli")),
            );
        }
        let error = discover(&home, &workspace, Agent::Codex, Some(id)).unwrap_err();
        assert!(error.contains("ambiguous"));
    }

    #[test]
    fn malformed_unrelated_codex_files_do_not_hide_a_latest_session() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        let id = "01999999-1111-7222-8333-444444444441";
        write(&codex_path(&home, "01", "broken"), "not json\n");
        write(
            &codex_path(&home, "02", id),
            &codex_record(id, &workspace, json!("user"), json!("cli")),
        );
        assert_eq!(
            discover(&home, &workspace, Agent::Codex, None)
                .unwrap()
                .id
                .as_str(),
            id
        );
    }

    #[test]
    fn claude_uses_the_first_non_sidechain_workspace() {
        let (_temporary, home) = home();
        let first = home.join("first");
        let second = home.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        let path = claude_path(&home, "-home-first", id);
        let content = [
            json!({"type": "mode", "sessionId": id}),
            json!({"sessionId": id, "cwd": home.join("side"), "isSidechain": true}),
            json!({"sessionId": id, "cwd": first, "isSidechain": false}),
            json!({"sessionId": id, "cwd": second}),
        ]
        .into_iter()
        .map(|record| format!("{record}\n"))
        .collect::<String>();
        write(&path, &content);
        let session = discover(&home, &first, Agent::Claude, None).unwrap();
        assert_eq!(session.workspace, first);
        assert!(discover(&home, &second, Agent::Claude, None).is_err());
    }

    #[test]
    fn claude_accepts_missing_and_non_boolean_sidechain_markers() {
        let (_temporary, home) = home();
        for (project, marker) in [("missing", None), ("string", Some(json!("true")))] {
            let workspace = home.join(project);
            fs::create_dir_all(&workspace).unwrap();
            let id = format!("{project}-session");
            let mut record = json!({"sessionId": id, "cwd": workspace});
            if let Some(marker) = marker {
                record["isSidechain"] = marker;
            }
            write(&claude_path(&home, project, &id), &format!("{record}\n"));
            assert!(discover(&home, &workspace, Agent::Claude, Some(&id)).is_ok());
        }
    }

    #[test]
    fn claude_metadata_only_sessions_have_no_workspace() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        let id = "11111111-2222-4333-8444-555555555555";
        write(
            &claude_path(&home, "project", id),
            &format!("{}\n", json!({"type": "mode", "sessionId": id})),
        );
        let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
        assert!(error.contains("no workspace"));
    }

    #[test]
    fn claude_internal_session_id_must_match_the_filename() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        let id = "11111111-2222-4333-8444-555555555555";
        let other = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        write(
            &claude_path(&home, "project", id),
            &format!("{}\n", json!({"sessionId": other, "cwd": workspace})),
        );
        let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
        assert!(error.contains("not resumable"));
    }

    #[test]
    fn claude_companion_is_returned_and_nested_jsonl_is_not_discovered() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        fs::create_dir_all(&workspace).unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        let path = claude_path(&home, "project", id);
        write(
            &path,
            &format!("{}\n", json!({"sessionId": id, "cwd": workspace})),
        );
        let companion = path.with_extension("");
        write(
            &companion.join("subagents/nested.jsonl"),
            &format!("{}\n", json!({"cwd": home.join("wrong")})),
        );
        let session = discover(&home, &workspace, Agent::Claude, None).unwrap();
        assert_eq!(session.companion, Some(companion));
    }

    #[test]
    fn latest_duplicate_claude_ids_are_refused_across_projects() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        fs::create_dir_all(&workspace).unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        let content = format!("{}\n", json!({"sessionId": id, "cwd": workspace}));
        write(&claude_path(&home, "one", id), &content);
        write(&claude_path(&home, "two", id), &content);
        let error = discover(&home, &workspace, Agent::Claude, None).unwrap_err();
        assert!(error.contains("ambiguous"));
    }

    #[test]
    fn selected_workspace_must_be_absolute_and_below_home() {
        let (_temporary, home) = home();
        let id = "11111111-2222-4333-8444-555555555555";
        let path = claude_path(&home, "project", id);
        write(
            &path,
            &format!("{}\n", json!({"sessionId": id, "cwd": "/outside"})),
        );
        assert!(discover(&home, Path::new("/outside"), Agent::Claude, Some(id)).is_err());
        write(
            &path,
            &format!("{}\n", json!({"sessionId": id, "cwd": "relative"})),
        );
        assert!(discover(&home, Path::new("relative"), Agent::Claude, Some(id)).is_err());
    }

    #[test]
    fn missing_stores_are_reported() {
        let (_temporary, home) = home();
        let workspace = home.join("src");
        assert!(
            discover(&home, &workspace, Agent::Codex, None)
                .unwrap_err()
                .contains("unavailable")
        );
        assert!(
            discover(&home, &workspace, Agent::Claude, None)
                .unwrap_err()
                .contains("unavailable")
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_symlink_transcripts_are_refused() {
        use std::os::unix::fs::symlink;

        let (_temporary, home) = home();
        let workspace = home.join("src");
        let id = "11111111-2222-4333-8444-555555555555";
        let target = home.join("target.jsonl");
        write(
            &target,
            &format!("{}\n", json!({"sessionId": id, "cwd": workspace})),
        );
        let path = claude_path(&home, "project", id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(&target, &path).unwrap();
        let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
        assert!(error.contains("unsafe non-regular"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_companions_are_refused() {
        use std::os::unix::fs::symlink;

        let (_temporary, home) = home();
        let workspace = home.join("src");
        fs::create_dir_all(&workspace).unwrap();
        let id = "11111111-2222-4333-8444-555555555555";
        let path = claude_path(&home, "project", id);
        write(
            &path,
            &format!("{}\n", json!({"sessionId": id, "cwd": workspace})),
        );
        let target = home.join("attachments");
        fs::create_dir_all(&target).unwrap();
        symlink(&target, path.with_extension("")).unwrap();
        let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
        assert!(error.contains("not a safe directory"));
    }
}
