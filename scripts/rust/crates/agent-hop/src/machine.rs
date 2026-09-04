use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::catalog;
use crate::cli::MachineRequest;
use crate::preview::{self, PreviewLimits, PreviewRole};
use crate::remote::{
    MACHINE_PROTOCOL_VERSION, MAX_REMOTE_PREVIEW_CHARS, MAX_REMOTE_SESSIONS, MAX_REMOTE_WARNINGS,
    RemoteCatalog, RemoteLineage, RemotePreview, RemotePreviewMessage, RemotePreviewRole,
    RemoteSession, encode_catalog_response, encode_lineage_response, encode_machine_error,
    encode_preview_response,
};

pub(crate) fn run(request: MachineRequest) -> Result<(), String> {
    match &request {
        MachineRequest::Export {
            protocol,
            agent,
            session,
            sha256,
            bytes,
        } => return export_response(*protocol, *agent, session, sha256, *bytes),
        MachineRequest::ExportCompanion {
            protocol,
            agent,
            session,
            workspace,
        } => return export_companion_response(*protocol, *agent, session, workspace),
        MachineRequest::Import {
            protocol,
            agent,
            session,
            destination,
            sha256,
            bytes,
        } => return import_response(*protocol, *agent, session, destination, sha256, *bytes),
        MachineRequest::RecordManifest { protocol } => {
            return record_manifest_response(*protocol);
        }
        _ => {}
    }
    let kind = match &request {
        MachineRequest::Catalog { .. } => "catalog",
        MachineRequest::Preview { .. } => "preview",
        MachineRequest::Lineage { .. } => "lineage",
        MachineRequest::Export { .. }
        | MachineRequest::ExportCompanion { .. }
        | MachineRequest::Import { .. }
        | MachineRequest::RecordManifest { .. } => unreachable!(),
    };
    let response = match request {
        MachineRequest::Catalog {
            protocol,
            workspace,
            limit,
        } => catalog_response(protocol, workspace.as_deref(), limit),
        MachineRequest::Preview {
            protocol,
            agent,
            session,
            max_chars,
        } => preview_response(protocol, agent, &session, max_chars),
        MachineRequest::Lineage {
            protocol,
            agent,
            session,
        } => lineage_response(protocol, agent, &session),
        MachineRequest::Export { .. }
        | MachineRequest::ExportCompanion { .. }
        | MachineRequest::Import { .. }
        | MachineRequest::RecordManifest { .. } => unreachable!(),
    };
    println!(
        "{}",
        response.unwrap_or_else(|error| encode_machine_error(kind, &error))
    );
    Ok(())
}

fn record_manifest_response(protocol: u64) -> Result<(), String> {
    require_protocol(protocol)?;
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take((crate::manifest::MAX_MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not receive transfer manifest: {error}"))?;
    let manifest = crate::manifest::TransferManifest::decode(&bytes)?;
    let home = crate::local_home()?;
    crate::manifest::record(&home, &manifest)?;
    Ok(())
}

fn import_response(
    protocol: u64,
    agent: crate::cli::Agent,
    session_id: &str,
    destination: &Path,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<(), String> {
    require_protocol(protocol)?;
    crate::session::SessionId::new(session_id)?;
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid immutable rollout SHA-256".to_string());
    }
    let home = crate::local_home()?;
    validate_import_destination(&home, agent, session_id, destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| "rollout destination has no parent directory".to_string())?;
    ensure_directory_tree(&home, parent)?;
    let reused = crate::transfer::install_immutable_stream(
        std::io::stdin().lock(),
        destination,
        expected_bytes,
        expected_sha256,
    )?;
    println!("{}", if reused { "reused" } else { "installed" });
    Ok(())
}

fn validate_import_destination(
    home: &Path,
    agent: crate::cli::Agent,
    session_id: &str,
    destination: &Path,
) -> Result<(), String> {
    let roots = match agent {
        crate::cli::Agent::Codex => vec![
            home.join(".codex/sessions"),
            home.join(".codex/archived_sessions"),
        ],
        crate::cli::Agent::Claude => vec![home.join(".claude/projects")],
    };
    if !destination.is_absolute()
        || destination.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
        || !roots.iter().any(|root| destination.starts_with(root))
    {
        return Err(format!(
            "rollout destination is outside the {} session store: {}",
            agent.name(),
            destination.display()
        ));
    }
    let filename = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "rollout destination has no UTF-8 filename".to_string())?;
    let expected = format!("{session_id}.jsonl");
    let valid = match agent {
        crate::cli::Agent::Codex => {
            filename == expected || filename.ends_with(&format!("-{expected}"))
        }
        crate::cli::Agent::Claude => filename == expected,
    };
    if !valid {
        return Err("rollout destination filename does not match its session ID".to_string());
    }
    Ok(())
}

fn ensure_directory_tree(home: &Path, target: &Path) -> Result<(), String> {
    let relative = target
        .strip_prefix(home)
        .map_err(|_| "rollout destination is outside the home directory".to_string())?;
    let mut current = home.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err("rollout destination is not normalized".to_string());
        };
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "unsafe destination component: {}",
                    current.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .map_err(|error| format!("could not create {}: {error}", current.display()))?;
            }
            Err(error) => {
                return Err(format!("could not inspect {}: {error}", current.display()));
            }
        }
    }
    Ok(())
}

fn export_companion_response(
    protocol: u64,
    agent: crate::cli::Agent,
    session_id: &str,
    expected_workspace: &Path,
) -> Result<(), String> {
    require_protocol(protocol)?;
    if agent != crate::cli::Agent::Claude {
        return Err("only Claude sessions have companion attachments".to_string());
    }
    let home = crate::local_home()?;
    crate::session::workspace_relative(&home, expected_workspace)?;
    let session = crate::session::discover(&home, &home, agent, Some(session_id))?;
    if session.workspace != expected_workspace {
        return Err("the session workspace changed before attachment export".to_string());
    }
    let companion = session
        .companion
        .as_deref()
        .ok_or_else(|| "the selected session has no companion attachments".to_string())?;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    crate::remote::write_companion_export(companion, &mut output)?;
    std::io::Write::flush(&mut output)
        .map_err(|error| format!("could not finish attachment export: {error}"))
}

fn export_response(
    protocol: u64,
    agent: crate::cli::Agent,
    session_id: &str,
    expected_sha256: &str,
    expected_bytes: u64,
) -> Result<(), String> {
    require_protocol(protocol)?;
    let home = crate::local_home()?;
    let artifact = crate::lineage::find_artifact(&home, agent, session_id)?;
    let snapshot = artifact.snapshot;
    if snapshot.size()? != expected_bytes || snapshot.sha256()? != expected_sha256 {
        return Err(format!(
            "immutable rollout {session_id} changed after lineage discovery; retry the hop"
        ));
    }
    let mut input = std::fs::File::open(snapshot.path())
        .map_err(|error| format!("could not open session export: {error}"))?;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    std::io::copy(&mut input, &mut output)
        .map_err(|error| format!("could not write session export: {error}"))?;
    std::io::Write::flush(&mut output)
        .map_err(|error| format!("could not finish session export: {error}"))
}

fn lineage_response(
    protocol: u64,
    agent: crate::cli::Agent,
    session_id: &str,
) -> Result<String, String> {
    require_protocol(protocol)?;
    let home = crate::local_home()?;
    let session = crate::session::discover(&home, &home, agent, Some(session_id))?;
    let lineage = crate::lineage::Lineage::discover(&home, &session)?;
    encode_lineage_response(&RemoteLineage {
        agent,
        selected_id: session_id.to_string(),
        artifacts: lineage.descriptors(),
    })
}

fn catalog_response(
    protocol: u64,
    workspace: Option<&Path>,
    limit: usize,
) -> Result<String, String> {
    require_protocol(protocol)?;
    if !(1..=MAX_REMOTE_SESSIONS).contains(&limit) {
        return Err(format!(
            "catalog limit must be between 1 and {MAX_REMOTE_SESSIONS}"
        ));
    }
    let home = crate::local_home()?;
    let workspace = workspace.unwrap_or(&home);
    crate::session::workspace_relative(&home, workspace)?;
    let found = catalog::scan(&home, workspace);
    let mut warnings = found
        .sources
        .iter()
        .filter_map(|source| match &source.state {
            crate::catalog::SourceState::Available { .. } | crate::catalog::SourceState::Absent => {
                None
            }
            crate::catalog::SourceState::Disabled(error) => Some(format!(
                "{} session store unavailable: {error}",
                source.agent.name()
            )),
        })
        .map(|warning| preview::sanitize(&warning))
        .filter(|warning| !warning.is_empty())
        .collect::<Vec<_>>();
    for agent in [crate::cli::Agent::Codex, crate::cli::Agent::Claude] {
        if let Some(summary) = catalog::diagnostic_summary(&found.diagnostics, agent) {
            warnings.push(format!("{} session store: {summary}", agent.name()));
        }
    }
    warnings.sort();
    warnings.dedup();
    warnings.truncate(MAX_REMOTE_WARNINGS);
    let sessions = found
        .sessions
        .into_iter()
        .take(limit)
        .map(|entry| {
            let limits = PreviewLimits {
                head_bytes: 64 * 1024,
                tail_bytes: 0,
                max_records_per_window: 1_024,
                max_messages: 1,
                max_message_chars: 256,
                max_title_chars: 96,
            };
            let title = preview::load(&entry.session, limits)
                .map(|preview| preview.title)
                .unwrap_or_else(|_| "Untitled session".to_string());
            let modified_ms = entry
                .modified
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            RemoteSession {
                agent: entry.session.agent,
                id: entry.session.id.as_str().to_string(),
                title,
                project: entry.project,
                workspace: entry.session.workspace,
                transcript: entry.session.transcript,
                companion: entry.session.companion,
                modified_ms,
            }
        })
        .collect();
    encode_catalog_response(&RemoteCatalog { sessions, warnings })
}

fn preview_response(
    protocol: u64,
    agent: crate::cli::Agent,
    session_id: &str,
    max_chars: usize,
) -> Result<String, String> {
    require_protocol(protocol)?;
    if !(1..=MAX_REMOTE_PREVIEW_CHARS).contains(&max_chars) {
        return Err(format!(
            "preview size must be between 1 and {MAX_REMOTE_PREVIEW_CHARS} characters"
        ));
    }
    let home = crate::local_home()?;
    let session = crate::session::discover(&home, &home, agent, Some(session_id))?;
    let limits = PreviewLimits {
        max_message_chars: max_chars.min(4_000),
        ..PreviewLimits::default()
    };
    let preview = preview::load(&session, limits)?;
    let mut remaining = max_chars;
    let mut truncated = preview.truncated;
    let messages = preview
        .messages
        .into_iter()
        .filter_map(|message| {
            if remaining == 0 {
                truncated = true;
                return None;
            }
            let text = message.text.chars().take(remaining).collect::<String>();
            let used = text.chars().count();
            truncated |= used < message.text.chars().count();
            remaining = remaining.saturating_sub(used);
            (!text.is_empty()).then_some(RemotePreviewMessage {
                role: match message.role {
                    PreviewRole::User => RemotePreviewRole::User,
                    PreviewRole::Assistant => RemotePreviewRole::Assistant,
                },
                text,
            })
        })
        .collect();
    encode_preview_response(&RemotePreview {
        title: preview.title,
        messages,
        truncated: truncated || preview.skipped_records > 0,
        warning: (preview.skipped_records > 0).then(|| {
            format!(
                "Ignored {} malformed or incomplete transcript record{}",
                preview.skipped_records,
                if preview.skipped_records == 1 {
                    ""
                } else {
                    "s"
                }
            )
        }),
    })
}

fn require_protocol(protocol: u64) -> Result<(), String> {
    if protocol == MACHINE_PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(format!(
            "incompatible agent-hop protocol: requested {protocol}, available {MACHINE_PROTOCOL_VERSION}"
        ))
    }
}

#[cfg(test)]
#[path = "../tests/unit/machine_tests.rs"]
mod tests;
