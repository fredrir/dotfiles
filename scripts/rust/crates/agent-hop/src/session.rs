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

#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub(crate) id: SessionId,
    pub(crate) transcript: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) modified: SystemTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CandidateFailureKind {
    Invalid,
    Unsafe,
    Storage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CandidateFailure {
    pub(crate) kind: CandidateFailureKind,
    pub(crate) message: String,
}

impl CandidateFailure {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: CandidateFailureKind::Invalid,
            message: message.into(),
        }
    }

    fn unsafe_entry(message: impl Into<String>) -> Self {
        Self {
            kind: CandidateFailureKind::Unsafe,
            message: message.into(),
        }
    }

    fn storage(message: impl Into<String>) -> Self {
        Self {
            kind: CandidateFailureKind::Storage,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MaterializeFailureKind {
    Invalid,
    Unsafe,
    Storage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MaterializeFailure {
    pub(crate) kind: MaterializeFailureKind,
    pub(crate) message: String,
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
    parse_candidate_classified(agent, transcript, expected).map_err(|failure| failure.message)
}

fn parse_candidate_classified(
    agent: Agent,
    transcript: PathBuf,
    expected: Option<&SessionId>,
) -> Result<Candidate, CandidateFailure> {
    let (id, workspace) = match agent {
        Agent::Codex => parse_codex(&transcript)?.ok_or_else(|| {
            CandidateFailure::invalid("the first record is not a user-authored Codex CLI session")
        })?,
        Agent::Claude => {
            let filename_id = claude_filename_id(&transcript).map_err(CandidateFailure::invalid)?;
            if expected.is_some_and(|expected| expected != &filename_id) {
                return Err(CandidateFailure::invalid(
                    "the filename does not match the requested session ID",
                ));
            }
            let workspace = parse_claude(&transcript, &filename_id)?;
            (filename_id, workspace)
        }
    };
    if !path_matches(agent, &transcript, &id) {
        return Err(CandidateFailure::invalid(
            "the filename does not match the session metadata ID",
        ));
    }
    let metadata = fs::symlink_metadata(&transcript).map_err(|error| {
        CandidateFailure::storage(format!(
            "could not inspect {}: {error}",
            transcript.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(CandidateFailure::unsafe_entry(format!(
            "the transcript is not a regular file: {}",
            transcript.display()
        )));
    }
    let modified = metadata.modified().map_err(|error| {
        CandidateFailure::storage(format!(
            "could not read the transcript modification time: {error}"
        ))
    })?;
    Ok(Candidate {
        id,
        transcript,
        workspace,
        modified,
    })
}

pub(crate) fn parse_candidate_for_catalog(
    agent: Agent,
    transcript: PathBuf,
) -> Result<Candidate, CandidateFailure> {
    parse_candidate_classified(agent, transcript, None)
}

fn parse_codex(path: &Path) -> Result<Option<(SessionId, PathBuf)>, CandidateFailure> {
    let file = File::open(path).map_err(|error| {
        CandidateFailure::storage(format!("could not open {}: {error}", path.display()))
    })?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let bytes = reader.read_until(b'\n', &mut line).map_err(|error| {
        CandidateFailure::storage(format!("could not read {}: {error}", path.display()))
    })?;
    if bytes == 0 {
        return Err(CandidateFailure::invalid("the transcript is empty"));
    }
    let record: Value = serde_json::from_slice(&line).map_err(|error| {
        CandidateFailure::invalid(format!(
            "the first transcript record is invalid JSON: {error}"
        ))
    })?;
    if record.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Ok(None);
    }
    let Some(payload) = record.get("payload") else {
        return Err(CandidateFailure::invalid(
            "the Codex session metadata has no payload",
        ));
    };
    if payload.get("thread_source").and_then(Value::as_str) != Some("user")
        || payload.get("source").and_then(Value::as_str) != Some("cli")
    {
        return Ok(None);
    }
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CandidateFailure::invalid("the Codex session metadata has no string ID"))?;
    let cwd = payload.get("cwd").and_then(Value::as_str).ok_or_else(|| {
        CandidateFailure::invalid("the Codex session metadata has no string workspace")
    })?;
    Ok(Some((
        SessionId::new(id).map_err(CandidateFailure::invalid)?,
        PathBuf::from(cwd),
    )))
}

fn parse_claude(path: &Path, filename_id: &SessionId) -> Result<PathBuf, CandidateFailure> {
    let file = File::open(path).map_err(|error| {
        CandidateFailure::storage(format!("could not open {}: {error}", path.display()))
    })?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let bytes = reader.read_until(b'\n', &mut line).map_err(|error| {
            CandidateFailure::storage(format!("could not read {}: {error}", path.display()))
        })?;
        if bytes == 0 {
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record: Value = serde_json::from_slice(&line).map_err(|error| {
            CandidateFailure::invalid(format!("a transcript record is invalid JSON: {error}"))
        })?;
        if let Some(value) = record.get("sessionId") {
            let value = value.as_str().ok_or_else(|| {
                CandidateFailure::invalid("Claude sessionId metadata is not a string")
            })?;
            let record_id = SessionId::new(value).map_err(CandidateFailure::invalid)?;
            if &record_id != filename_id {
                return Err(CandidateFailure::invalid(format!(
                    "transcript metadata names session {record_id}, not {filename_id}"
                )));
            }
        }
        let Some(cwd) = record.get("cwd") else {
            continue;
        };
        if cwd.is_null() || record.get("isSidechain") == Some(&Value::Bool(true)) {
            continue;
        }
        let cwd = cwd.as_str().ok_or_else(|| {
            CandidateFailure::invalid("Claude workspace metadata is not a string")
        })?;
        return Ok(PathBuf::from(cwd));
    }
    Err(CandidateFailure::invalid(
        "the Claude Code session has no workspace",
    ))
}

pub(crate) fn validate_snapshot_identity(
    path: &Path,
    agent: Agent,
    expected_id: &SessionId,
    expected_workspace: &Path,
) -> Result<(), String> {
    let (id, workspace) = match agent {
        Agent::Codex => parse_codex(path)
            .map_err(|failure| failure.message)?
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
    materialize_for_catalog(home, agent, candidate).map_err(|failure| failure.message)
}

pub(crate) fn materialize_for_catalog(
    home: &Path,
    agent: Agent,
    candidate: Candidate,
) -> Result<Session, MaterializeFailure> {
    workspace_relative(home, &candidate.workspace).map_err(|message| MaterializeFailure {
        kind: MaterializeFailureKind::Invalid,
        message,
    })?;
    let metadata =
        fs::symlink_metadata(&candidate.transcript).map_err(|error| MaterializeFailure {
            kind: MaterializeFailureKind::Storage,
            message: format!(
                "could not inspect {}: {error}",
                candidate.transcript.display()
            ),
        })?;
    if !metadata.file_type().is_file() {
        return Err(MaterializeFailure {
            kind: MaterializeFailureKind::Unsafe,
            message: format!(
                "the transcript is not a regular file: {}",
                candidate.transcript.display()
            ),
        });
    }
    let companion = if agent == Agent::Claude {
        let path = candidate.transcript.with_extension("");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => Some(path),
            Ok(_) => {
                return Err(MaterializeFailure {
                    kind: MaterializeFailureKind::Unsafe,
                    message: format!(
                        "the Claude session companion is not a safe directory: {}",
                        path.display()
                    ),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(MaterializeFailure {
                    kind: MaterializeFailureKind::Storage,
                    message: format!(
                        "could not inspect Claude session companion {}: {error}",
                        path.display()
                    ),
                });
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

/// History bases can point at an archived backing rollout even though archived
/// sessions are intentionally omitted from the interactive catalog.
pub(crate) fn scan_codex_lineage(home: &Path) -> Result<Scan, String> {
    let mut scan = scan_codex(home)?;
    let root = home.join(".codex/archived_sessions");
    match fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(scan),
        Err(error) => {
            return Err(format!(
                "Codex archived session store is unavailable at {}: {error}",
                root.display()
            ));
        }
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "Codex archived session store is not a safe directory: {}",
                root.display()
            ));
        }
    }
    let mut directories = vec![root];
    for _ in 0..4 {
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
                let path = entry.path();
                match entry.file_type() {
                    Ok(file_type) if file_type.is_dir() => next.push(path),
                    Ok(file_type) if path.extension() == Some(OsStr::new("jsonl")) => {
                        if file_type.is_file() {
                            scan.regular.push(path);
                        } else {
                            scan.unsafe_entries.push(path);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => scan
                        .errors
                        .push(format!("could not inspect {}: {error}", path.display())),
                }
            }
        }
        directories = next;
    }
    scan.regular.sort();
    scan.regular.dedup();
    scan.unsafe_entries.sort();
    scan.unsafe_entries.dedup();
    Ok(scan)
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
#[path = "../tests/unit/session_tests.rs"]
mod tests;
