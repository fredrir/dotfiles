use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::catalog;
use crate::cli::MachineRequest;
use crate::preview::{self, PreviewLimits, PreviewRole};
use crate::remote::{
    MACHINE_PROTOCOL_VERSION, MAX_REMOTE_PREVIEW_CHARS, MAX_REMOTE_SESSIONS, MAX_REMOTE_WARNINGS,
    RemoteCatalog, RemotePreview, RemotePreviewMessage, RemotePreviewRole, RemoteSession,
    encode_catalog_response, encode_machine_error, encode_preview_response,
};

pub(crate) fn run(request: MachineRequest) -> Result<(), String> {
    match &request {
        MachineRequest::Export {
            protocol,
            agent,
            session,
        } => return export_response(*protocol, *agent, session),
        MachineRequest::ExportCompanion {
            protocol,
            agent,
            session,
            workspace,
        } => return export_companion_response(*protocol, *agent, session, workspace),
        _ => {}
    }
    let kind = match &request {
        MachineRequest::Catalog { .. } => "catalog",
        MachineRequest::Preview { .. } => "preview",
        MachineRequest::Export { .. } | MachineRequest::ExportCompanion { .. } => unreachable!(),
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
        MachineRequest::Export { .. } | MachineRequest::ExportCompanion { .. } => unreachable!(),
    };
    println!(
        "{}",
        response.unwrap_or_else(|error| encode_machine_error(kind, &error))
    );
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
) -> Result<(), String> {
    require_protocol(protocol)?;
    let home = crate::local_home()?;
    let session = crate::session::discover(&home, &home, agent, Some(session_id))?;
    let snapshot = crate::transfer::Snapshot::create(&session.transcript)?;
    crate::session::validate_snapshot_identity(
        snapshot.path(),
        session.agent,
        &session.id,
        &session.workspace,
    )?;
    let mut input = std::fs::File::open(snapshot.path())
        .map_err(|error| format!("could not open session export: {error}"))?;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    std::io::copy(&mut input, &mut output)
        .map_err(|error| format!("could not write session export: {error}"))?;
    std::io::Write::flush(&mut output)
        .map_err(|error| format!("could not finish session export: {error}"))
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
            crate::catalog::SourceState::Available { .. } => None,
            crate::catalog::SourceState::Disabled(error) => Some(format!(
                "{} session store unavailable: {error}",
                source.agent.name()
            )),
        })
        .chain(found.diagnostics.iter().map(|item| item.message.clone()))
        .map(|warning| preview::sanitize(&warning))
        .filter(|warning| !warning.is_empty())
        .collect::<Vec<_>>();
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
mod tests {
    use super::*;

    #[test]
    fn only_the_current_protocol_is_accepted() {
        assert!(require_protocol(MACHINE_PROTOCOL_VERSION).is_ok());
        assert!(require_protocol(MACHINE_PROTOCOL_VERSION + 1).is_err());
    }
}
