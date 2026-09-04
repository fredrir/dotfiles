use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::NamedTempFile;

use crate::cli::Agent;
use crate::session::{self, Session, SessionId};
use crate::transfer::Snapshot;

const MAX_ANCESTORS: usize = 128;
const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Boundary {
    destination_offset: u64,
    next_ordinal: u64,
}

type BoundaryMap = BTreeMap<u64, Boundary>;
type SnapshotTransform = (Snapshot, BoundaryMap, Option<HistoryBase>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Projection {
    byte_offset: u64,
    next_ordinal: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct HistoryBase {
    pub(crate) thread_id: String,
    pub(crate) end_ordinal_exclusive: u64,
    pub(crate) end_byte_offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArtifactDescriptor {
    pub(crate) session_id: String,
    pub(crate) workspace: PathBuf,
    pub(crate) transcript: PathBuf,
    pub(crate) history_base: Option<HistoryBase>,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

pub(crate) struct Artifact {
    pub(crate) descriptor: ArtifactDescriptor,
    pub(crate) snapshot: Snapshot,
}

pub(crate) struct Lineage {
    pub(crate) agent: Agent,
    /// Root first, selected session last.
    pub(crate) artifacts: Vec<Artifact>,
}

pub(crate) struct TransformedArtifact {
    pub(crate) source: ArtifactDescriptor,
    pub(crate) destination: ArtifactDescriptor,
    pub(crate) snapshot: Snapshot,
}

pub(crate) struct TransformedLineage {
    pub(crate) agent: Agent,
    pub(crate) artifacts: Vec<TransformedArtifact>,
}

impl Lineage {
    pub(crate) fn discover(home: &Path, session: &Session) -> Result<Self, String> {
        match session.agent {
            Agent::Claude => {
                let artifact =
                    snapshot_artifact(Agent::Claude, &session.transcript, Some(&session.id))?;
                Ok(Self {
                    agent: session.agent,
                    artifacts: vec![artifact],
                })
            }
            Agent::Codex => discover_codex(home, session),
        }
    }

    pub(crate) fn descriptors(&self) -> Vec<ArtifactDescriptor> {
        self.artifacts
            .iter()
            .map(|artifact| artifact.descriptor.clone())
            .collect()
    }

    pub(crate) fn from_snapshots(
        agent: Agent,
        descriptors: Vec<ArtifactDescriptor>,
        snapshots: Vec<Snapshot>,
    ) -> Result<Self, String> {
        if descriptors.is_empty() || descriptors.len() != snapshots.len() {
            return Err("remote lineage response is incomplete".to_string());
        }
        let mut artifacts = Vec::with_capacity(descriptors.len());
        for (descriptor, snapshot) in descriptors.into_iter().zip(snapshots) {
            if snapshot.size()? != descriptor.bytes || snapshot.sha256()? != descriptor.sha256 {
                return Err(format!(
                    "immutable rollout {} changed between lineage discovery and export",
                    descriptor.session_id
                ));
            }
            let parsed = match agent {
                Agent::Codex => read_codex_metadata(snapshot.path())?,
                Agent::Claude => {
                    let id = SessionId::new(&descriptor.session_id)?;
                    read_claude_metadata(snapshot.path(), &id)?
                }
            };
            if parsed.session_id != descriptor.session_id
                || parsed.workspace != descriptor.workspace
                || parsed.history_base != descriptor.history_base
            {
                return Err(format!(
                    "immutable rollout {} metadata changed during export",
                    descriptor.session_id
                ));
            }
            artifacts.push(Artifact {
                descriptor,
                snapshot,
            });
        }
        if agent == Agent::Codex {
            validate_ancestry(&artifacts)?;
        } else if artifacts.len() != 1 {
            return Err("Claude sessions cannot contain Codex history_base ancestry".to_string());
        }
        Ok(Self { agent, artifacts })
    }

    pub(crate) fn transform(
        self,
        source_home: &Path,
        destination_home: &Path,
    ) -> Result<TransformedLineage, String> {
        let mut transformed = Vec::with_capacity(self.artifacts.len());
        let mut parent_boundaries: Option<BoundaryMap> = None;
        for artifact in self.artifacts {
            let source = artifact.descriptor;
            let destination_workspace =
                rebase_path(&source.workspace, source_home, destination_home)?;
            let (snapshot, boundaries, destination_base) = transform_snapshot(
                artifact.snapshot,
                self.agent,
                source_home,
                destination_home,
                parent_boundaries.as_ref(),
            )?;
            let destination = ArtifactDescriptor {
                session_id: source.session_id.clone(),
                workspace: destination_workspace,
                transcript: PathBuf::new(),
                history_base: destination_base,
                bytes: snapshot.size()?,
                sha256: snapshot.sha256()?,
            };
            transformed.push(TransformedArtifact {
                source,
                destination,
                snapshot,
            });
            parent_boundaries = Some(boundaries);
        }
        Ok(TransformedLineage {
            agent: self.agent,
            artifacts: transformed,
        })
    }
}

pub(crate) fn find_artifact(home: &Path, agent: Agent, id: &str) -> Result<Artifact, String> {
    let id = SessionId::new(id)?;
    match agent {
        Agent::Claude => {
            let session = session::discover(home, home, agent, Some(id.as_str()))?;
            snapshot_artifact(agent, &session.transcript, Some(&id))
        }
        Agent::Codex => {
            let scan = session::scan_codex_lineage(home)?;
            if let Some(path) = scan
                .unsafe_entries
                .iter()
                .find(|path| session::path_matches(Agent::Codex, path, &id))
            {
                return Err(format!(
                    "Codex rollout {} is an unsafe non-regular file: {}",
                    id,
                    path.display()
                ));
            }
            let matches = scan
                .regular
                .into_iter()
                .filter(|path| session::path_matches(Agent::Codex, path, &id))
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(format!(
                    "Codex rollout {} is {}",
                    id,
                    if matches.is_empty() {
                        "missing".to_string()
                    } else {
                        format!("ambiguous ({} files)", matches.len())
                    }
                ));
            }
            snapshot_artifact(agent, &matches[0], Some(&id))
        }
    }
}

fn discover_codex(home: &Path, session: &Session) -> Result<Lineage, String> {
    let scan = session::scan_codex_lineage(home)?;
    let regular = scan.regular;
    let unsafe_entries = scan.unsafe_entries;

    let mut reverse = Vec::new();
    let mut next_id = session.id.as_str().to_owned();
    let mut selected_path = Some(session.transcript.clone());
    let mut visited = BTreeSet::new();
    for _ in 0..MAX_ANCESTORS {
        if !visited.insert(next_id.clone()) {
            return Err(format!(
                "Codex history_base ancestry contains a cycle at session {next_id}"
            ));
        }
        let path = if let Some(path) = selected_path.take() {
            path
        } else {
            let expected = SessionId::new(&next_id)?;
            if let Some(path) = unsafe_entries
                .iter()
                .find(|path| session::path_matches(Agent::Codex, path, &expected))
            {
                return Err(format!(
                    "cannot transfer Codex session {}: required history_base ancestor {} is an unsafe non-regular file at {}",
                    session.id,
                    next_id,
                    path.display()
                ));
            }
            let matches = regular
                .iter()
                .filter(|path| session::path_matches(Agent::Codex, path, &expected))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                return Err(format!(
                    "cannot transfer Codex session {}: required history_base ancestor {} is missing",
                    session.id, next_id
                ));
            }
            if matches.len() != 1 {
                return Err(format!(
                    "cannot transfer Codex session {}: required history_base ancestor {} is ambiguous ({} files)",
                    session.id,
                    next_id,
                    matches.len()
                ));
            }
            (*matches[0]).clone()
        };
        let expected = SessionId::new(&next_id)?;
        let artifact = snapshot_artifact(Agent::Codex, &path, Some(&expected)).map_err(|error| {
            format!(
                "cannot transfer Codex session {}: required history_base object {} is invalid: {error}",
                session.id, next_id
            )
        })?;
        next_id = match artifact.descriptor.history_base.as_ref() {
            Some(base) => base.thread_id.clone(),
            None => {
                reverse.push(artifact);
                break;
            }
        };
        reverse.push(artifact);
    }
    if reverse
        .last()
        .is_some_and(|artifact| artifact.descriptor.history_base.is_some())
    {
        return Err(format!(
            "cannot transfer Codex session {}: history_base ancestry exceeds {MAX_ANCESTORS} objects",
            session.id
        ));
    }
    reverse.reverse();
    for artifact in &reverse {
        session::workspace_relative(home, &artifact.descriptor.workspace).map_err(|error| {
            format!(
                "cannot transfer Codex ancestor {}: {error}",
                artifact.descriptor.session_id
            )
        })?;
    }
    validate_ancestry(&reverse)?;
    validate_projection_offsets(home, &reverse)?;
    Ok(Lineage {
        agent: Agent::Codex,
        artifacts: reverse,
    })
}

fn snapshot_artifact(
    agent: Agent,
    path: &Path,
    expected: Option<&SessionId>,
) -> Result<Artifact, String> {
    let snapshot = Snapshot::create(path)?;
    let mut descriptor = match agent {
        Agent::Codex => read_codex_metadata(snapshot.path())?,
        Agent::Claude => {
            read_claude_metadata(snapshot.path(), expected.ok_or("missing Claude ID")?)?
        }
    };
    if let Some(expected) = expected
        && descriptor.session_id != expected.as_str()
    {
        return Err(format!(
            "transcript {} names session {}, not {}",
            path.display(),
            descriptor.session_id,
            expected
        ));
    }
    descriptor.transcript = path.to_path_buf();
    descriptor.bytes = snapshot.size()?;
    descriptor.sha256 = snapshot.sha256()?;
    Ok(Artifact {
        descriptor,
        snapshot,
    })
}

fn read_codex_metadata(path: &Path) -> Result<ArtifactDescriptor, String> {
    let file =
        File::open(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let bytes = reader
        .by_ref()
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes == 0 {
        return Err(format!("Codex rollout is empty: {}", path.display()));
    }
    if bytes > MAX_RECORD_BYTES {
        return Err("Codex rollout metadata record is unreasonably large".to_string());
    }
    let record: Value = serde_json::from_slice(&line)
        .map_err(|error| format!("Codex rollout metadata is invalid JSON: {error}"))?;
    if record.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err(format!(
            "Codex rollout has no leading session_meta record: {}",
            path.display()
        ));
    }
    let payload = record
        .get("payload")
        .and_then(Value::as_object)
        .ok_or_else(|| "Codex rollout metadata has no payload".to_string())?;
    let session_id = payload
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex rollout metadata has no string ID".to_string())?;
    SessionId::new(session_id)?;
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex rollout metadata has no string cwd".to_string())?;
    let history_base = payload
        .get("history_base")
        .filter(|value| !value.is_null())
        .map(parse_history_base)
        .transpose()?;
    Ok(ArtifactDescriptor {
        session_id: session_id.to_owned(),
        workspace: PathBuf::from(cwd),
        transcript: path.to_path_buf(),
        history_base,
        bytes: 0,
        sha256: String::new(),
    })
}

fn parse_history_base(value: &Value) -> Result<HistoryBase, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Codex history_base is not an object".to_string())?;
    let thread_id = object
        .get("thread_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Codex history_base has no string thread_id".to_string())?;
    SessionId::new(thread_id)?;
    let end_ordinal_exclusive = object
        .get("end_ordinal_exclusive")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Codex history_base has no nonnegative end_ordinal_exclusive".to_string())?;
    let end_byte_offset = object
        .get("end_byte_offset")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Codex history_base has no nonnegative end_byte_offset".to_string())?;
    Ok(HistoryBase {
        thread_id: thread_id.to_owned(),
        end_ordinal_exclusive,
        end_byte_offset,
    })
}

fn read_claude_metadata(path: &Path, expected: &SessionId) -> Result<ArtifactDescriptor, String> {
    let workspace = claude_workspace(path)?;
    session::validate_snapshot_identity(path, Agent::Claude, expected, &workspace)?;
    Ok(ArtifactDescriptor {
        session_id: expected.as_str().to_owned(),
        workspace,
        transcript: path.to_path_buf(),
        history_base: None,
        bytes: 0,
        sha256: String::new(),
    })
}

fn claude_workspace(path: &Path) -> Result<PathBuf, String> {
    let file =
        File::open(path).map_err(|error| format!("could not read Claude transcript: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not read Claude transcript: {error}"))?
            == 0
        {
            break;
        }
        let record: Value = serde_json::from_slice(&line)
            .map_err(|error| format!("Claude transcript contains invalid JSON: {error}"))?;
        if record.get("isSidechain") != Some(&Value::Bool(true))
            && let Some(cwd) = record.get("cwd").and_then(Value::as_str)
        {
            return Ok(PathBuf::from(cwd));
        }
    }
    Err("Claude transcript has no workspace".to_string())
}

fn validate_ancestry(artifacts: &[Artifact]) -> Result<(), String> {
    if artifacts.is_empty() {
        return Err("Codex lineage is empty".to_string());
    }
    if let Some(base) = artifacts[0].descriptor.history_base.as_ref() {
        return Err(format!(
            "Codex lineage is incomplete: root candidate {} still requires history_base ancestor {}",
            artifacts[0].descriptor.session_id, base.thread_id
        ));
    }
    let mut ids = BTreeSet::new();
    for artifact in artifacts {
        if !ids.insert(artifact.descriptor.session_id.as_str()) {
            return Err(format!(
                "Codex lineage contains duplicate session {}",
                artifact.descriptor.session_id
            ));
        }
    }
    for pair in artifacts.windows(2) {
        let parent = &pair[0].descriptor;
        let child = &pair[1].descriptor;
        let base = child.history_base.as_ref().ok_or_else(|| {
            format!(
                "Codex lineage is disconnected between {} and {}",
                parent.session_id, child.session_id
            )
        })?;
        if base.thread_id != parent.session_id {
            return Err(format!(
                "Codex session {} names history_base {}, but resolved ancestor is {}",
                child.session_id, base.thread_id, parent.session_id
            ));
        }
        if parent.bytes < base.end_byte_offset {
            return Err(format!(
                "refusing to launch Codex session {}: history_base needs byte offset {} from parent {}, but its immutable snapshot is only {} bytes",
                child.session_id, base.end_byte_offset, parent.session_id, parent.bytes
            ));
        }
        let local_ordinal =
            record_boundary_index(pair[0].snapshot.path(), base.end_byte_offset).map_err(
                |error| {
                    format!(
                        "refusing to launch Codex session {}: history_base offset {} in parent {} {error}",
                        child.session_id, base.end_byte_offset, parent.session_id
                    )
                },
            )?;
        let expected_ordinal = artifact_start_ordinal(parent)
            .checked_add(local_ordinal as u64)
            .ok_or_else(|| "Codex history ordinal overflowed u64".to_string())?;
        if expected_ordinal != base.end_ordinal_exclusive {
            return Err(format!(
                "refusing to launch Codex session {}: history_base byte offset {} resolves to ordinal {}, not declared ordinal {} in parent {}",
                child.session_id,
                base.end_byte_offset,
                expected_ordinal,
                base.end_ordinal_exclusive,
                parent.session_id
            ));
        }
    }
    Ok(())
}

fn validate_projection_offsets(home: &Path, artifacts: &[Artifact]) -> Result<(), String> {
    if artifacts.is_empty() {
        return Ok(());
    }
    let database = home.join(".codex/thread_history_1.sqlite");
    if !database.exists() {
        return Ok(());
    }
    let output = Command::new("sqlite3")
        .args([
            "-readonly",
            "-separator",
            "\t",
            database
                .to_str()
                .ok_or_else(|| "Codex projection database path is not UTF-8".to_string())?,
            "SELECT thread_id, next_rollout_byte_offset, next_rollout_ordinal FROM thread_history_projection_state;",
        ])
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "sqlite3 is required to validate Codex rollout projections".to_string()
            } else {
                format!("could not inspect Codex rollout projections: {error}")
            }
        })?;
    if !output.status.success() {
        return Err(format!(
            "could not inspect Codex rollout projections: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut projections = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split('\t');
        let (Some(id), Some(offset), Some(ordinal), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err("Codex projection database returned malformed output".to_string());
        };
        let byte_offset = offset
            .parse::<u64>()
            .map_err(|_| "Codex projection database returned an invalid offset".to_string())?;
        let next_ordinal = ordinal
            .parse::<u64>()
            .map_err(|_| "Codex projection database returned an invalid ordinal".to_string())?;
        projections.insert(
            id.to_owned(),
            Projection {
                byte_offset,
                next_ordinal,
            },
        );
    }
    validate_projections(artifacts, &projections)
}

fn validate_projections(
    artifacts: &[Artifact],
    projections: &HashMap<String, Projection>,
) -> Result<(), String> {
    for artifact in artifacts {
        let descriptor = &artifact.descriptor;
        let Some(projection) = projections.get(descriptor.session_id.as_str()) else {
            continue;
        };
        if descriptor.bytes < projection.byte_offset {
            return Err(format!(
                "refusing to launch Codex session {}: indexed projection expects byte offset {}, but the immutable rollout snapshot is only {} bytes",
                descriptor.session_id, projection.byte_offset, descriptor.bytes
            ));
        }
        let local_ordinal = record_boundary_index(artifact.snapshot.path(), projection.byte_offset)
            .map_err(|error| {
                format!(
                    "refusing to launch Codex session {}: indexed projection byte offset {} {error}",
                    descriptor.session_id, projection.byte_offset
                )
            })?;
        let expected_ordinal = artifact_start_ordinal(descriptor)
            .checked_add(local_ordinal as u64)
            .ok_or_else(|| "Codex history ordinal overflowed u64".to_string())?;
        if projection.next_ordinal != expected_ordinal {
            return Err(format!(
                "refusing to launch Codex session {}: indexed projection expects ordinal {}, but byte offset {} resolves to ordinal {}",
                descriptor.session_id,
                projection.next_ordinal,
                projection.byte_offset,
                expected_ordinal
            ));
        }
    }
    Ok(())
}

fn artifact_start_ordinal(descriptor: &ArtifactDescriptor) -> u64 {
    descriptor
        .history_base
        .as_ref()
        .map_or(0, |base| base.end_ordinal_exclusive)
}

fn transform_snapshot(
    source: Snapshot,
    agent: Agent,
    source_home: &Path,
    destination_home: &Path,
    parent_boundaries: Option<&BoundaryMap>,
) -> Result<SnapshotTransform, String> {
    let input = File::open(source.path())
        .map_err(|error| format!("could not open rollout snapshot: {error}"))?;
    let mut reader = BufReader::new(input);
    let mut output = NamedTempFile::new()
        .map_err(|error| format!("could not create transformed rollout: {error}"))?;
    let mut source_offset = 0u64;
    let mut destination_offset = 0u64;
    let mut boundaries = BTreeMap::new();
    let mut line = Vec::new();
    let mut first = true;
    let mut record_index = 0u64;
    let mut paginated_start = None;
    let mut transformed_base = None;
    loop {
        line.clear();
        let count = reader
            .by_ref()
            .take((MAX_RECORD_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not transform rollout: {error}"))?;
        if count == 0 {
            break;
        }
        if count > MAX_RECORD_BYTES {
            return Err("rollout contains an unreasonably large record".to_string());
        }
        let (record_bytes, terminator): (&[u8], &[u8]) =
            if let Some(record) = line.strip_suffix(b"\r\n") {
                (record, b"\r\n")
            } else if let Some(record) = line.strip_suffix(b"\n") {
                (record, b"\n")
            } else {
                (&line, b"")
            };
        let mut value: Value = serde_json::from_slice(record_bytes)
            .map_err(|error| format!("could not transform invalid rollout JSON: {error}"))?;
        let before = value.clone();
        if agent == Agent::Codex
            && first
            && let Some(base_value) = value.pointer_mut("/payload/history_base")
            && !base_value.is_null()
        {
            let mut base = parse_history_base(base_value)?;
            let mapping = parent_boundaries.ok_or_else(|| {
                format!(
                    "history_base ancestor {} was not transferred before its child",
                    base.thread_id
                )
            })?;
            let boundary = mapping.get(&base.end_byte_offset).ok_or_else(|| {
                format!(
                    "history_base offset {} is not a JSONL record boundary in ancestor {}",
                    base.end_byte_offset, base.thread_id
                )
            })?;
            base.end_byte_offset = boundary.destination_offset;
            base.end_ordinal_exclusive = boundary.next_ordinal;
            if let Some(object) = base_value.as_object_mut() {
                object.insert(
                    "end_byte_offset".to_string(),
                    Value::Number(base.end_byte_offset.into()),
                );
                object.insert(
                    "end_ordinal_exclusive".to_string(),
                    Value::Number(base.end_ordinal_exclusive.into()),
                );
            }
            transformed_base = Some(base);
        }
        if agent == Agent::Codex && first {
            paginated_start = match value.get("ordinal") {
                Some(value) => {
                    value.as_u64().ok_or_else(|| {
                        "Codex rollout metadata has an invalid ordinal".to_string()
                    })?;
                    Some(
                        transformed_base
                            .as_ref()
                            .map_or(0, |base| base.end_ordinal_exclusive),
                    )
                }
                None if transformed_base.is_some() => {
                    return Err(
                        "Codex rollout with history_base has no paginated ordinal".to_string()
                    );
                }
                None => None,
            };
            boundaries.insert(
                0,
                Boundary {
                    destination_offset: 0,
                    next_ordinal: paginated_start.unwrap_or(0),
                },
            );
        }
        if let Some(start) = paginated_start {
            let ordinal = start
                .checked_add(record_index)
                .ok_or_else(|| "Codex history ordinal overflowed u64".to_string())?;
            value
                .as_object_mut()
                .ok_or_else(|| "Codex rollout record is not an object".to_string())?
                .insert("ordinal".to_string(), Value::Number(ordinal.into()));
        }
        transform_structural_paths(&mut value, agent, source_home, destination_home)?;
        if value == before {
            output
                .write_all(&line)
                .map_err(|error| format!("could not write transformed rollout: {error}"))?;
            destination_offset += count as u64;
        } else {
            serde_json::to_writer(output.as_file_mut(), &value)
                .map_err(|error| format!("could not write transformed rollout: {error}"))?;
            output
                .write_all(terminator)
                .map_err(|error| format!("could not write transformed rollout: {error}"))?;
            destination_offset = output
                .as_file()
                .metadata()
                .map_err(|error| format!("could not inspect transformed rollout: {error}"))?
                .len();
        }
        source_offset += count as u64;
        record_index = record_index
            .checked_add(1)
            .ok_or_else(|| "rollout record count overflowed u64".to_string())?;
        let next_ordinal = paginated_start
            .unwrap_or(0)
            .checked_add(record_index)
            .ok_or_else(|| "Codex history ordinal overflowed u64".to_string())?;
        boundaries.insert(
            source_offset,
            Boundary {
                destination_offset,
                next_ordinal,
            },
        );
        first = false;
    }
    output
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("could not rewind transformed rollout: {error}"))?;
    Ok((
        Snapshot::from_temporary(output)?,
        boundaries,
        transformed_base,
    ))
}

fn transform_structural_paths(
    record: &mut Value,
    agent: Agent,
    source_home: &Path,
    destination_home: &Path,
) -> Result<(), String> {
    match agent {
        Agent::Claude => transform_string_at(record, "/cwd", source_home, destination_home),
        Agent::Codex => {
            transform_string_at(record, "/payload/cwd", source_home, destination_home)?;
            transform_string_at(
                record,
                "/payload/thread_settings/cwd",
                source_home,
                destination_home,
            )?;
            if let Some(roots) = record
                .pointer_mut("/payload/workspace_roots")
                .and_then(Value::as_array_mut)
            {
                for root in roots {
                    transform_string_value(root, source_home, destination_home)?;
                }
            }
            if let Some(environments) = record
                .pointer_mut("/payload/state/environments/environments")
                .and_then(Value::as_object_mut)
            {
                for environment in environments.values_mut() {
                    if let Some(cwd) = environment.get_mut("cwd") {
                        transform_string_value(cwd, source_home, destination_home)?;
                    }
                }
            }
            Ok(())
        }
    }
}

fn transform_string_at(
    record: &mut Value,
    pointer: &str,
    source_home: &Path,
    destination_home: &Path,
) -> Result<(), String> {
    if let Some(value) = record.pointer_mut(pointer) {
        transform_string_value(value, source_home, destination_home)?;
    }
    Ok(())
}

fn transform_string_value(
    value: &mut Value,
    source_home: &Path,
    destination_home: &Path,
) -> Result<(), String> {
    let Some(path) = value.as_str() else {
        return Ok(());
    };
    let rebased = rebase_path(Path::new(path), source_home, destination_home)?;
    if rebased != Path::new(path) {
        *value = Value::String(
            rebased
                .to_str()
                .ok_or_else(|| format!("rebased path is not UTF-8: {}", rebased.display()))?
                .to_owned(),
        );
    }
    Ok(())
}

pub(crate) fn rebase_path(
    path: &Path,
    source_home: &Path,
    destination_home: &Path,
) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    match path.strip_prefix(source_home) {
        Ok(relative) => Ok(destination_home.join(relative)),
        Err(_) => Ok(path.to_path_buf()),
    }
}

fn record_boundary_index(path: &Path, offset: u64) -> Result<usize, String> {
    if offset == 0 {
        return Ok(0);
    }
    let file =
        File::open(path).map_err(|error| format!("could not read parent rollout: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut position = 0u64;
    let mut line = Vec::new();
    let mut ordinal = 0usize;
    while position < offset {
        line.clear();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not read parent rollout: {error}"))?;
        if count == 0 {
            break;
        }
        position += count as u64;
        ordinal += 1;
    }
    if position == offset {
        Ok(ordinal)
    } else {
        Err("does not land on a JSONL record boundary".to_string())
    }
}

#[cfg(test)]
#[path = "../tests/unit/lineage_tests.rs"]
mod tests;
