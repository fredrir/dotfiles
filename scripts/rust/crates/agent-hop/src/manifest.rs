use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::cli::Agent;
use crate::lineage::TransformedLineage;
use crate::session::SessionId;

const SCHEMA_VERSION: u64 = 2;
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct HostMapping {
    pub(crate) source_host: String,
    pub(crate) destination_host: String,
    pub(crate) source_home: PathBuf,
    pub(crate) destination_home: PathBuf,
    pub(crate) source_workspace: PathBuf,
    pub(crate) destination_workspace: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManifestArtifact {
    pub(crate) session_id: String,
    pub(crate) parent_id: Option<String>,
    pub(crate) source_path: PathBuf,
    pub(crate) destination_path: PathBuf,
    pub(crate) source_sha256: String,
    pub(crate) destination_sha256: String,
    pub(crate) source_bytes: u64,
    pub(crate) destination_bytes: u64,
    pub(crate) source_history_offset: Option<u64>,
    pub(crate) destination_history_offset: Option<u64>,
    pub(crate) source_history_ordinal: Option<u64>,
    pub(crate) destination_history_ordinal: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct TransferManifest {
    pub(crate) schema_version: u64,
    pub(crate) transfer_id: String,
    pub(crate) created_at_ms: u64,
    pub(crate) state: String,
    pub(crate) agent: String,
    pub(crate) parent_id: String,
    pub(crate) child_id: Option<String>,
    pub(crate) mapping: HostMapping,
    pub(crate) artifacts: Vec<ManifestArtifact>,
}

impl TransferManifest {
    pub(crate) fn installed(
        source_host: &str,
        destination_host: &str,
        source_home: &Path,
        destination_home: &Path,
        lineage: &TransformedLineage,
    ) -> Result<Self, String> {
        let selected = lineage
            .artifacts
            .last()
            .ok_or_else(|| "cannot manifest an empty lineage".to_string())?;
        let now = now_ms()?;
        let parent_id = selected.source.session_id.clone();
        let prefix = parent_id.chars().take(12).collect::<String>();
        let transfer_id = format!("{}-{}-{prefix}", now_ns()?, std::process::id());
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            transfer_id,
            created_at_ms: now,
            state: "installed".to_string(),
            agent: lineage.agent.name().to_string(),
            parent_id,
            child_id: None,
            mapping: HostMapping {
                source_host: source_host.to_string(),
                destination_host: destination_host.to_string(),
                source_home: source_home.to_path_buf(),
                destination_home: destination_home.to_path_buf(),
                source_workspace: selected.source.workspace.clone(),
                destination_workspace: selected.destination.workspace.clone(),
            },
            artifacts: lineage
                .artifacts
                .iter()
                .map(|artifact| ManifestArtifact {
                    session_id: artifact.source.session_id.clone(),
                    parent_id: artifact
                        .source
                        .history_base
                        .as_ref()
                        .map(|base| base.thread_id.clone()),
                    source_path: artifact.source.transcript.clone(),
                    destination_path: artifact.destination.transcript.clone(),
                    source_sha256: artifact.source.sha256.clone(),
                    destination_sha256: artifact.destination.sha256.clone(),
                    source_bytes: artifact.source.bytes,
                    destination_bytes: artifact.destination.bytes,
                    source_history_offset: artifact
                        .source
                        .history_base
                        .as_ref()
                        .map(|base| base.end_byte_offset),
                    destination_history_offset: artifact
                        .destination
                        .history_base
                        .as_ref()
                        .map(|base| base.end_byte_offset),
                    source_history_ordinal: artifact
                        .source
                        .history_base
                        .as_ref()
                        .map(|base| base.end_ordinal_exclusive),
                    destination_history_ordinal: artifact
                        .destination
                        .history_base
                        .as_ref()
                        .map(|base| base.end_ordinal_exclusive),
                })
                .collect(),
        })
    }

    pub(crate) fn launched(&self, child_id: String) -> Result<Self, String> {
        let mut manifest = self.clone();
        manifest.created_at_ms = now_ms()?;
        manifest.state = "launched".to_string();
        manifest.child_id = Some(child_id);
        Ok(manifest)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, String> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| format!("could not encode transfer manifest: {error}"))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err("transfer manifest is unreasonably large".to_string());
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_MANIFEST_BYTES {
            return Err("transfer manifest has an invalid size".to_string());
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid transfer manifest: {error}"))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION
            || self.transfer_id.is_empty()
            || self.transfer_id.len() > 256
            || self.parent_id.is_empty()
            || !matches!(self.agent.as_str(), "codex" | "claude")
            || self.artifacts.is_empty()
            || self.artifacts.len() > crate::remote::MAX_LINEAGE_ARTIFACTS
            || !matches!(
                (self.state.as_str(), self.child_id.as_ref()),
                ("installed", None) | ("launched", Some(_))
            )
            || !valid_host(&self.mapping.source_host)
            || !valid_host(&self.mapping.destination_host)
            || !self.mapping.source_home.is_absolute()
            || !self.mapping.destination_home.is_absolute()
            || !self.mapping.source_workspace.is_absolute()
            || !self.mapping.destination_workspace.is_absolute()
            || self
                .artifacts
                .last()
                .is_none_or(|artifact| artifact.session_id != self.parent_id)
        {
            return Err("transfer manifest is internally inconsistent".to_string());
        }
        SessionId::new(&self.parent_id)?;
        if let Some(child_id) = self.child_id.as_deref() {
            SessionId::new(child_id)?;
        }
        for (index, artifact) in self.artifacts.iter().enumerate() {
            SessionId::new(&artifact.session_id)?;
            if let Some(parent_id) = artifact.parent_id.as_deref() {
                SessionId::new(parent_id)?;
            }
            let expected_parent = index
                .checked_sub(1)
                .map(|parent| self.artifacts[parent].session_id.as_str());
            if artifact.parent_id.as_deref() != expected_parent
                || !artifact.source_path.is_absolute()
                || !artifact.destination_path.is_absolute()
                || artifact.source_bytes == 0
                || artifact.destination_bytes == 0
                || !valid_sha256(&artifact.source_sha256)
                || !valid_sha256(&artifact.destination_sha256)
                || artifact.source_history_offset.is_some() != artifact.parent_id.is_some()
                || artifact.destination_history_offset.is_some() != artifact.parent_id.is_some()
                || artifact.source_history_ordinal.is_some() != artifact.parent_id.is_some()
                || artifact.destination_history_ordinal.is_some() != artifact.parent_id.is_some()
            {
                return Err("transfer manifest artifact lineage is inconsistent".to_string());
            }
        }
        Ok(())
    }
}

/// Each manifest is a new immutable object; updates create a new state record with
/// the same transfer ID instead of rewriting the installed record.
pub(crate) fn record(home: &Path, manifest: &TransferManifest) -> Result<PathBuf, String> {
    manifest.validate()?;
    let directory = manifest_directory(home);
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "could not create transfer manifest directory {}: {error}",
            directory.display()
        )
    })?;
    let child = manifest.child_id.as_deref().unwrap_or("pending");
    let filename = format!(
        "{}-{}-{}.json",
        manifest.created_at_ms,
        sanitize_component(&manifest.transfer_id),
        sanitize_component(child)
    );
    let destination = directory.join(filename);
    let mut temporary = NamedTempFile::new_in(&directory)
        .map_err(|error| format!("could not stage transfer manifest: {error}"))?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), manifest)
        .map_err(|error| format!("could not encode transfer manifest: {error}"))?;
    use std::io::Write;
    temporary
        .write_all(b"\n")
        .map_err(|error| format!("could not finish transfer manifest: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("could not sync transfer manifest: {error}"))?;
    match temporary.persist_noclobber(&destination) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !crate::transfer::files_equal(error.file.path(), &destination)? {
                return Err(format!(
                    "immutable transfer manifest already exists with different contents: {}",
                    destination.display()
                ));
            }
        }
        Err(error) => {
            return Err(format!(
                "could not install transfer manifest: {}",
                error.error
            ));
        }
    }
    Ok(destination)
}

pub(crate) fn latest_child(
    home: &Path,
    source_host: &str,
    destination_host: &str,
    agent: Agent,
    parent_id: &str,
    source_sha256: &str,
) -> Result<Option<String>, String> {
    let directory = manifest_directory(home);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read transfer manifests: {error}")),
    };
    let mut best: Option<(u64, String)> = None;
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read transfer manifests: {error}"))?;
        if !entry
            .file_type()
            .map_err(|error| format!("could not inspect transfer manifest: {error}"))?
            .is_file()
        {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| format!("could not inspect transfer manifest: {error}"))?;
        if metadata.len() > MAX_MANIFEST_BYTES as u64 {
            continue;
        }
        let bytes = fs::read(entry.path())
            .map_err(|error| format!("could not read transfer manifest: {error}"))?;
        let Ok(manifest) = TransferManifest::decode(&bytes) else {
            continue;
        };
        let selected_hash = manifest
            .artifacts
            .last()
            .map(|artifact| artifact.source_sha256.as_str());
        if manifest.schema_version == SCHEMA_VERSION
            && manifest.agent == agent.name()
            && manifest.parent_id == parent_id
            && manifest.mapping.source_host == source_host
            && manifest.mapping.destination_host == destination_host
            && selected_hash == Some(source_sha256)
            && let Some(child) = manifest.child_id
            && best
                .as_ref()
                .is_none_or(|(time, _)| manifest.created_at_ms > *time)
        {
            best = Some((manifest.created_at_ms, child));
        }
    }
    Ok(best.map(|(_, child)| child))
}

fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn manifest_directory(home: &Path) -> PathBuf {
    home.join(".local/state/agent-hop/transfers")
}

fn now_ms() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?
        .as_millis()
        .min(u128::from(u64::MAX)) as u64)
}

fn now_ns() -> Result<u128, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?
        .as_nanos())
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_updates_are_separate_immutable_files() {
        let home = tempfile::tempdir().unwrap();
        let installed = TransferManifest {
            schema_version: SCHEMA_VERSION,
            transfer_id: "transfer".to_string(),
            created_at_ms: 1,
            state: "installed".to_string(),
            agent: "codex".to_string(),
            parent_id: "parent".to_string(),
            child_id: None,
            mapping: HostMapping {
                source_host: "macie".to_string(),
                destination_host: "archie".to_string(),
                source_home: "/Users/f".into(),
                destination_home: "/home/f".into(),
                source_workspace: "/Users/f/work".into(),
                destination_workspace: "/home/f/work".into(),
            },
            artifacts: vec![ManifestArtifact {
                session_id: "parent".to_string(),
                parent_id: None,
                source_path: "/Users/f/source.jsonl".into(),
                destination_path: "/home/f/destination.jsonl".into(),
                source_sha256: "a".repeat(64),
                destination_sha256: "b".repeat(64),
                source_bytes: 10,
                destination_bytes: 11,
                source_history_offset: None,
                destination_history_offset: None,
                source_history_ordinal: None,
                destination_history_ordinal: None,
            }],
        };
        let first = record(home.path(), &installed).unwrap();
        let launched = installed.launched("child".to_string()).unwrap();
        let second = record(home.path(), &launched).unwrap();
        assert_ne!(first, second);
        assert!(first.exists());
        assert!(second.exists());
        assert_eq!(
            latest_child(
                home.path(),
                "macie",
                "archie",
                Agent::Codex,
                "parent",
                &"a".repeat(64)
            )
            .unwrap()
            .as_deref(),
            Some("child")
        );
    }
}
