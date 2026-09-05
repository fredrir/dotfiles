use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hostkit::Host;
use hostkit::shell::{quote, quote_path};
use hostkit::ssh::{self, Session};
use serde_json::{Value, json};

use crate::cli::Agent;
use crate::lineage::{ArtifactDescriptor, HistoryBase};
use crate::manifest::TransferManifest;
use crate::preview::sanitize;
use crate::session::SessionId;
use tempfile::NamedTempFile;

const MACHINE_PROTOCOL: &str = "agent-hop-machine";
pub(crate) const MACHINE_PROTOCOL_VERSION: u64 = 2;
pub(crate) const MAX_REMOTE_SESSIONS: usize = 2_000;
pub(crate) const MAX_LINEAGE_ARTIFACTS: usize = 128;
pub(crate) const MAX_REMOTE_PREVIEW_CHARS: usize = 64 * 1024;
pub(crate) const MAX_REMOTE_WARNINGS: usize = 128;
const MAX_CATALOG_OUTPUT: usize = 4 * 1024 * 1024;
const MAX_PREVIEW_OUTPUT: usize = 256 * 1024;
const MAX_LINEAGE_OUTPUT: usize = 256 * 1024;
const MAX_ERROR_OUTPUT: usize = 16 * 1024;
const MAX_WIRE_PATH: usize = 16 * 1024;
const MAX_WIRE_PROJECT_CHARS: usize = 256;
const MAX_COMPANION_HEADER: usize = 32 * 1024;
const MAX_COMPANION_ENTRIES: usize = 100_000;
const MAX_WIRE_TITLE_CHARS: usize = 512;
const MAX_WIRE_MESSAGES: usize = 32;
const MACHINE_TIMEOUT: Duration = Duration::from_secs(20);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const COMPANION_STREAM_MAGIC: &[u8] = b"agent-hop-companion/1\n";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteCatalog {
    pub(crate) sessions: Vec<RemoteSession>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteSession {
    pub(crate) agent: Agent,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) workspace: PathBuf,
    /// Kept for transfer, and never intended for normal UI rendering.
    pub(crate) transcript: PathBuf,
    /// Kept for transfer, and never intended for normal UI rendering.
    pub(crate) companion: Option<PathBuf>,
    pub(crate) modified_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemoteLineage {
    pub(crate) agent: Agent,
    pub(crate) selected_id: String,
    pub(crate) artifacts: Vec<ArtifactDescriptor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemotePreview {
    pub(crate) title: String,
    pub(crate) messages: Vec<RemotePreviewMessage>,
    pub(crate) truncated: bool,
    pub(crate) warning: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RemotePreviewRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemotePreviewMessage {
    pub(crate) role: RemotePreviewRole,
    pub(crate) text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Remote {
    peer: Host,
}

impl Remote {
    pub fn new(peer: Host) -> Remote {
        Remote { peer }
    }

    pub fn peer(self) -> Host {
        self.peer
    }

    pub fn home(self) -> Result<PathBuf, String> {
        let output = self.output(ssh::HOME_SCRIPT)?;
        ssh::parse_home(self.peer.name(), &output.stdout)
    }

    pub(crate) fn home_noninteractive(self) -> Result<PathBuf, String> {
        let bytes = self.bounded_output(ssh::HOME_SCRIPT, MAX_WIRE_PATH)?;
        ssh::parse_home(self.peer.name(), &bytes)
    }

    /// Query the peer without allocating a TTY. The peer returns paths solely so a
    /// subsequently selected record can be copied; callers must not render them as
    /// preview content.
    pub(crate) fn catalog(
        self,
        remote_home: &Path,
        workspace: Option<&Path>,
        limit: usize,
    ) -> Result<RemoteCatalog, String> {
        if !(1..=MAX_REMOTE_SESSIONS).contains(&limit) {
            return Err(format!(
                "remote catalog limit must be between 1 and {MAX_REMOTE_SESSIONS}"
            ));
        }
        require_normal_absolute(remote_home, "the other machine's home directory")?;
        let mut arguments = vec![
            "__machine".to_owned(),
            "catalog".to_owned(),
            "--protocol".to_owned(),
            MACHINE_PROTOCOL_VERSION.to_string(),
            "--limit".to_owned(),
            limit.to_string(),
        ];
        if let Some(workspace) = workspace {
            let workspace = workspace.to_str().ok_or_else(|| {
                format!(
                    "remote workspace is not valid UTF-8: {}",
                    workspace.display()
                )
            })?;
            arguments.push("--workspace".to_owned());
            arguments.push(workspace.to_owned());
        }
        let bytes = self.machine(&arguments, MAX_CATALOG_OUTPUT)?;
        parse_catalog_response(&bytes, remote_home, limit)
    }

    pub(crate) fn preview(
        self,
        agent: Agent,
        session_id: &str,
        max_chars: usize,
    ) -> Result<RemotePreview, String> {
        SessionId::new(session_id)?;
        if !(1..=MAX_REMOTE_PREVIEW_CHARS).contains(&max_chars) {
            return Err(format!(
                "remote preview size must be between 1 and {MAX_REMOTE_PREVIEW_CHARS} characters"
            ));
        }
        let arguments = [
            "__machine".to_owned(),
            "preview".to_owned(),
            "--protocol".to_owned(),
            MACHINE_PROTOCOL_VERSION.to_string(),
            "--agent".to_owned(),
            agent.name().to_owned(),
            "--session".to_owned(),
            session_id.to_owned(),
            "--max-chars".to_owned(),
            max_chars.to_string(),
        ];
        let bytes = self.machine(&arguments, MAX_PREVIEW_OUTPUT)?;
        parse_preview_response(&bytes, max_chars)
    }

    pub(crate) fn lineage(
        self,
        remote_home: &Path,
        agent: Agent,
        session_id: &str,
    ) -> Result<RemoteLineage, String> {
        SessionId::new(session_id)?;
        let arguments = [
            "__machine".to_owned(),
            "lineage".to_owned(),
            "--protocol".to_owned(),
            MACHINE_PROTOCOL_VERSION.to_string(),
            "--agent".to_owned(),
            agent.name().to_owned(),
            "--session".to_owned(),
            session_id.to_owned(),
        ];
        let bytes = self.machine(&arguments, MAX_LINEAGE_OUTPUT)?;
        parse_lineage_response(&bytes, remote_home, agent, session_id)
    }

    pub(crate) fn pull_artifact(
        self,
        remote_home: &Path,
        agent: Agent,
        artifact: &ArtifactDescriptor,
        destination: &Path,
    ) -> Result<(), String> {
        SessionId::new(&artifact.session_id)?;
        validate_transcript_name(agent, &artifact.session_id, &artifact.transcript)?;
        validate_remote_source(
            remote_home,
            agent,
            &artifact.transcript,
            RemoteSourceKind::Transcript,
        )?;
        let arguments = [
            "__machine".to_owned(),
            "export".to_owned(),
            "--protocol".to_owned(),
            MACHINE_PROTOCOL_VERSION.to_string(),
            "--agent".to_owned(),
            agent.name().to_owned(),
            "--session".to_owned(),
            artifact.session_id.clone(),
            "--sha256".to_owned(),
            artifact.sha256.clone(),
            "--bytes".to_owned(),
            artifact.bytes.to_string(),
        ];
        let output = File::options()
            .write(true)
            .truncate(true)
            .open(destination)
            .map_err(|error| format!("could not open session export destination: {error}"))?;
        let mut command = Command::new("ssh");
        command
            .args(machine_ssh_arguments(
                self.peer,
                &machine_script(&arguments),
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::piped());
        let output = redirected_output_with_timeout(command, TRANSFER_TIMEOUT)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(captured_stderr_error(
                self.peer,
                &output.stderr,
                output.status,
            ))
        }
    }

    pub(crate) fn install_artifact(
        self,
        agent: Agent,
        artifact: &ArtifactDescriptor,
        source: &Path,
    ) -> Result<(), String> {
        SessionId::new(&artifact.session_id)?;
        let destination = artifact
            .transcript
            .to_str()
            .ok_or_else(|| "rollout destination is not UTF-8".to_string())?;
        let arguments = [
            "__machine".to_owned(),
            "import".to_owned(),
            "--protocol".to_owned(),
            MACHINE_PROTOCOL_VERSION.to_string(),
            "--agent".to_owned(),
            agent.name().to_owned(),
            "--session".to_owned(),
            artifact.session_id.clone(),
            "--destination".to_owned(),
            destination.to_owned(),
            "--sha256".to_owned(),
            artifact.sha256.clone(),
            "--bytes".to_owned(),
            artifact.bytes.to_string(),
        ];
        let input = File::open(source)
            .map_err(|error| format!("could not open rollout snapshot: {error}"))?;
        let mut command = Command::new("ssh");
        command
            .args(machine_ssh_arguments(
                self.peer,
                &machine_script(&arguments),
            ))
            .stdin(Stdio::from(input))
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = redirected_output_with_timeout(command, TRANSFER_TIMEOUT)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(captured_stderr_error(
                self.peer,
                &output.stderr,
                output.status,
            ))
        }
    }

    pub(crate) fn record_manifest(self, manifest: &TransferManifest) -> Result<(), String> {
        let bytes = manifest.encode()?;
        let arguments = [
            "__machine".to_owned(),
            "record-manifest".to_owned(),
            "--protocol".to_owned(),
            MACHINE_PROTOCOL_VERSION.to_string(),
        ];
        let mut staged = NamedTempFile::new()
            .map_err(|error| format!("could not stage transfer manifest: {error}"))?;
        staged
            .write_all(&bytes)
            .map_err(|error| format!("could not stage transfer manifest: {error}"))?;
        let input = staged
            .reopen()
            .map_err(|error| format!("could not stage transfer manifest: {error}"))?;
        let mut command = Command::new("ssh");
        command
            .args(machine_ssh_arguments(
                self.peer,
                &machine_script(&arguments),
            ))
            .stdin(Stdio::from(input))
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = redirected_output_with_timeout(command, MACHINE_TIMEOUT)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(captured_stderr_error(
                self.peer,
                &output.stderr,
                output.status,
            ))
        }
    }

    pub(crate) fn pull_companion(
        self,
        remote_home: &Path,
        session: &RemoteSession,
        destination: &Path,
    ) -> Result<(), String> {
        SessionId::new(&session.id)?;
        validate_workspace(remote_home, &session.workspace)?;
        let source = session
            .companion
            .as_deref()
            .ok_or_else(|| "the selected remote session has no attachments".to_owned())?;
        if source != session.transcript.with_extension("") {
            return Err("the remote session companion does not match its transcript".to_owned());
        }
        validate_remote_source(
            remote_home,
            session.agent,
            source,
            RemoteSourceKind::Companion,
        )?;
        require_empty_directory(destination)?;
        let workspace = session.workspace.to_str().ok_or_else(|| {
            format!(
                "remote workspace is not valid UTF-8: {}",
                session.workspace.display()
            )
        })?;
        let arguments = [
            "__machine".to_owned(),
            "export-companion".to_owned(),
            "--protocol".to_owned(),
            MACHINE_PROTOCOL_VERSION.to_string(),
            "--agent".to_owned(),
            session.agent.name().to_owned(),
            "--session".to_owned(),
            session.id.clone(),
            "--workspace".to_owned(),
            workspace.to_owned(),
        ];
        let mut archive = NamedTempFile::new()
            .map_err(|error| format!("could not stage remote attachments: {error}"))?;
        let output = archive
            .reopen()
            .map_err(|error| format!("could not stage remote attachments: {error}"))?;
        let mut command = Command::new("ssh");
        command
            .args(machine_ssh_arguments(
                self.peer,
                &machine_script(&arguments),
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::piped());
        let output = redirected_output_with_timeout(command, TRANSFER_TIMEOUT)?;
        if !output.status.success() {
            return Err(captured_stderr_error(
                self.peer,
                &output.stderr,
                output.status,
            ));
        }
        archive
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|error| format!("could not inspect remote attachments: {error}"))?;
        read_companion_export(archive, destination)
    }

    pub fn preflight(self, workspace: &Path, agent: Agent) -> Result<(), String> {
        self.output(&preflight_script(workspace, agent)?)?;
        Ok(())
    }

    pub fn exists(self, path: &Path) -> Result<bool, String> {
        let output = self.output(&exists_script(path)?)?;
        match output.stdout.as_slice() {
            b"yes\n" => Ok(true),
            b"no\n" => Ok(false),
            _ => Err(format!(
                "{} returned an invalid file status",
                self.peer.name()
            )),
        }
    }

    pub fn file_matches(self, local: &Path, remote: &Path) -> Result<bool, String> {
        let file = File::open(local)
            .map_err(|error| format!("could not open {}: {error}", local.display()))?;
        let output = Command::new("ssh")
            .args(ssh_arguments(self.peer, &compare_script(remote)?, false))
            .stdin(Stdio::from(file))
            .output()
            .map_err(command_error)?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(output_error(self.peer, &output)),
        }
    }

    pub fn launch(self, workspace: &Path, agent: Agent, session_id: &str) -> Result<(), String> {
        let script = launch_script(workspace, agent, session_id)?;
        self.run_interactive(&script, agent)
    }

    pub fn resume(self, workspace: &Path, agent: Agent, session_id: &str) -> Result<(), String> {
        let script = resume_script(workspace, agent, session_id)?;
        self.run_interactive(&script, agent)
    }

    fn run_interactive(self, script: &str, agent: Agent) -> Result<(), String> {
        let status = Command::new("ssh")
            .args(ssh_arguments(self.peer, script, true))
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(command_error)?;
        if status.success() {
            Ok(())
        } else {
            Err(match status.code() {
                Some(code) => format!(
                    "{} {} session exited with status {code}",
                    self.peer.name(),
                    agent.name()
                ),
                None => format!(
                    "{} {} session was interrupted",
                    self.peer.name(),
                    agent.name()
                ),
            })
        }
    }

    fn output(self, script: &str) -> Result<Output, String> {
        let output = Command::new("ssh")
            .args(ssh_arguments(self.peer, script, false))
            .output()
            .map_err(command_error)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(output_error(self.peer, &output))
        }
    }

    fn machine(self, arguments: &[String], output_limit: usize) -> Result<Vec<u8>, String> {
        let script = machine_script(arguments);
        self.bounded_output(&script, output_limit)
    }

    fn bounded_output(self, script: &str, output_limit: usize) -> Result<Vec<u8>, String> {
        let mut command = Command::new("ssh");
        command
            .args(machine_ssh_arguments(self.peer, script))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = output_with_timeout(command, output_limit, MACHINE_TIMEOUT)?;
        if !output.status.success() {
            return Err(captured_output_error(self.peer, &output));
        }
        if output.stdout.truncated {
            return Err(format!(
                "{} returned too much machine-readable data",
                self.peer.name()
            ));
        }
        Ok(output.stdout.bytes)
    }
}

pub fn preflight_script(workspace: &Path, agent: Agent) -> Result<String, String> {
    let workspace = quote_path(workspace)?;
    let agent = quote(agent.name());
    Ok(format!(
        "test -d {workspace} || {{ printf '%s\\n' 'workspace does not exist' >&2; exit 1; }}; \
         command -v {agent} >/dev/null 2>&1 || {{ printf '%s\\n' 'agent command is not available' >&2; exit 1; }}; \
         command -v 'zsh' >/dev/null 2>&1 || {{ printf '%s\\n' 'zsh is not available' >&2; exit 1; }}"
    ))
}

pub fn exists_script(path: &Path) -> Result<String, String> {
    let path = quote_path(path)?;
    Ok(format!(
        "if [ -e {path} ] || [ -L {path} ]; then printf 'yes\\n'; else printf 'no\\n'; fi"
    ))
}

pub fn compare_script(path: &Path) -> Result<String, String> {
    Ok(format!("cmp -s - {}", quote_path(path)?))
}

#[cfg(test)]
pub fn mkdir_script(path: &Path) -> Result<String, String> {
    Ok(format!("mkdir -p -- {}", quote_path(path)?))
}

pub fn fork_command(workspace: &Path, agent: Agent, session_id: &str) -> Result<String, String> {
    let session_id = quote(session_id);
    Ok(match agent {
        Agent::Codex => format!("codex fork {session_id} -C {}", quote_path(workspace)?),
        Agent::Claude => format!("claude --resume {session_id} --fork-session"),
    })
}

pub fn launch_script(workspace: &Path, agent: Agent, session_id: &str) -> Result<String, String> {
    let inner = fork_command(workspace, agent, session_id)?;
    Ok(format!(
        "cd -- {} && exec zsh -lic {}",
        quote_path(workspace)?,
        quote(&inner)
    ))
}

pub fn resume_command(workspace: &Path, agent: Agent, session_id: &str) -> Result<String, String> {
    let session_id = quote(session_id);
    Ok(match agent {
        Agent::Codex => format!("codex resume {session_id} -C {}", quote_path(workspace)?),
        Agent::Claude => format!("claude --resume {session_id}"),
    })
}

pub fn resume_script(workspace: &Path, agent: Agent, session_id: &str) -> Result<String, String> {
    let inner = resume_command(workspace, agent, session_id)?;
    Ok(format!(
        "cd -- {} && exec zsh -lic {}",
        quote_path(workspace)?,
        quote(&inner)
    ))
}

pub fn ssh_arguments(peer: Host, script: &str, interactive: bool) -> Vec<OsString> {
    let session = Session::new(peer.name()).script(script);
    if interactive {
        session.interactive()
    } else {
        session
    }
    .args()
}

pub(crate) fn machine_script(arguments: &[String]) -> String {
    let command = arguments
        .iter()
        .map(|argument| quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    format!("export PATH=\"$HOME/.local/bin:$PATH\"; exec agent-hop {command}")
}

pub(crate) fn machine_ssh_arguments(peer: Host, script: &str) -> Vec<OsString> {
    Session::new(peer.name()).script(script).batch().args()
}

pub(crate) fn encode_catalog_response(catalog: &RemoteCatalog) -> Result<String, String> {
    if catalog.sessions.len() > MAX_REMOTE_SESSIONS {
        return Err(format!(
            "cannot encode more than {MAX_REMOTE_SESSIONS} remote sessions"
        ));
    }
    if catalog.warnings.len() > MAX_REMOTE_WARNINGS {
        return Err(format!(
            "cannot encode more than {MAX_REMOTE_WARNINGS} remote catalog warnings"
        ));
    }
    let sessions = catalog
        .sessions
        .iter()
        .map(|session| {
            Ok(json!({
                "agent": session.agent.name(),
                "id": session.id,
                "title": session.title,
                "project": session.project,
                "workspace": wire_path(&session.workspace)?,
                "transcript": wire_path(&session.transcript)?,
                "companion": session.companion.as_deref().map(wire_path).transpose()?,
                "modified_ms": session.modified_ms,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let warnings = catalog
        .warnings
        .iter()
        .map(|warning| clean_wire_text(warning, 1_024, false).0)
        .filter(|warning| !warning.is_empty())
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "catalog",
        "ok": true,
        "data": { "sessions": sessions, "warnings": warnings },
    }))
    .map_err(|error| format!("could not encode the remote catalog: {error}"))
}

pub(crate) fn encode_lineage_response(lineage: &RemoteLineage) -> Result<String, String> {
    if lineage.artifacts.is_empty() || lineage.artifacts.len() > MAX_LINEAGE_ARTIFACTS {
        return Err(format!(
            "lineage must contain between 1 and {MAX_LINEAGE_ARTIFACTS} artifacts"
        ));
    }
    let artifacts = lineage
        .artifacts
        .iter()
        .map(|artifact| {
            Ok(json!({
                "session_id": artifact.session_id,
                "workspace": wire_path(&artifact.workspace)?,
                "transcript": wire_path(&artifact.transcript)?,
                "history_base": artifact.history_base,
                "bytes": artifact.bytes,
                "sha256": artifact.sha256,
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    serde_json::to_string(&json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "lineage",
        "ok": true,
        "data": {
            "agent": lineage.agent.name(),
            "selected_id": lineage.selected_id,
            "artifacts": artifacts,
        },
    }))
    .map_err(|error| format!("could not encode remote lineage: {error}"))
}

pub(crate) fn encode_preview_response(preview: &RemotePreview) -> Result<String, String> {
    if preview.messages.len() > MAX_WIRE_MESSAGES {
        return Err(format!(
            "cannot encode more than {MAX_WIRE_MESSAGES} preview messages"
        ));
    }
    let messages = preview
        .messages
        .iter()
        .map(|message| {
            json!({
                "role": match message.role {
                    RemotePreviewRole::User => "user",
                    RemotePreviewRole::Assistant => "assistant",
                },
                "text": message.text,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "preview",
        "ok": true,
        "data": {
            "title": preview.title,
            "messages": messages,
            "truncated": preview.truncated,
            "warning": preview.warning,
        },
    }))
    .map_err(|error| format!("could not encode the remote preview: {error}"))
}

pub(crate) fn encode_machine_error(kind: &str, error: &str) -> String {
    // Do not reflect arbitrary transcript content. Machine handlers should pass a
    // short diagnostic; this final scrub also prevents terminal control injection.
    let (error, _) = clean_wire_text(error, 1_024, false);
    json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": kind,
        "ok": false,
        "error": error,
    })
    .to_string()
}

fn parse_catalog_response(
    bytes: &[u8],
    remote_home: &Path,
    requested_limit: usize,
) -> Result<RemoteCatalog, String> {
    let root = response_data(bytes, "catalog")?;
    let records = root
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| "the remote catalog has no sessions array".to_owned())?;
    let warning_records = root
        .get("warnings")
        .and_then(Value::as_array)
        .ok_or_else(|| "the remote catalog has no warnings array".to_owned())?;
    if warning_records.len() > MAX_REMOTE_WARNINGS {
        return Err("the remote catalog has too many warnings".to_owned());
    }
    let warnings = warning_records
        .iter()
        .map(|warning| {
            let warning = warning
                .as_str()
                .ok_or_else(|| "the remote catalog contains a non-string warning".to_owned())?;
            Ok(clean_wire_text(warning, 1_024, false).0)
        })
        .filter_map(|warning: Result<String, String>| match warning {
            Ok(warning) if warning.is_empty() => None,
            warning => Some(warning),
        })
        .collect::<Result<Vec<_>, String>>()?;
    if records.len() > requested_limit || records.len() > MAX_REMOTE_SESSIONS {
        return Err("the remote catalog exceeded the requested session limit".to_owned());
    }
    let mut sessions = Vec::with_capacity(records.len());
    for record in records {
        let record = record
            .as_object()
            .ok_or_else(|| "the remote catalog contains a non-object session".to_owned())?;
        let agent = parse_agent(required_string(record.get("agent"), "session agent")?)?;
        let id = required_string(record.get("id"), "session ID")?;
        SessionId::new(id)?;
        let title = required_string(record.get("title"), "session title")?;
        let (title, _) = clean_wire_text(title, MAX_WIRE_TITLE_CHARS, false);
        let workspace = parse_wire_path(record.get("workspace"), "session workspace")?;
        validate_workspace(remote_home, &workspace)?;
        let fallback_project = fallback_project_label(&workspace);
        let project = match record.get("project") {
            None | Some(Value::Null) => fallback_project,
            Some(value) => {
                let raw = value.as_str().ok_or_else(|| {
                    "the remote catalog contains a non-string project label".to_owned()
                })?;
                let (project, _) = clean_wire_text(raw, MAX_WIRE_PROJECT_CHARS, false);
                if project.is_empty() {
                    fallback_project
                } else {
                    project
                }
            }
        };
        let transcript = parse_wire_path(record.get("transcript"), "session transcript")?;
        validate_remote_source(
            remote_home,
            agent,
            &transcript,
            RemoteSourceKind::Transcript,
        )?;
        validate_transcript_name(agent, id, &transcript)?;
        let companion = match record.get("companion") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let path = parse_wire_path(Some(value), "session companion")?;
                validate_remote_source(remote_home, agent, &path, RemoteSourceKind::Companion)?;
                Some(path)
            }
        };
        if agent == Agent::Codex && companion.is_some() {
            return Err("a remote Codex session unexpectedly has a companion".to_owned());
        }
        if let Some(companion) = companion.as_deref()
            && companion != transcript.with_extension("")
        {
            return Err("the remote session companion does not match its transcript".to_owned());
        }
        let modified_ms = record
            .get("modified_ms")
            .and_then(Value::as_u64)
            .ok_or_else(|| "the remote session has no valid modification time".to_owned())?;
        sessions.push(RemoteSession {
            agent,
            id: id.to_owned(),
            title,
            project,
            workspace,
            transcript,
            companion,
            modified_ms,
        });
    }
    Ok(RemoteCatalog { sessions, warnings })
}

fn parse_lineage_response(
    bytes: &[u8],
    remote_home: &Path,
    expected_agent: Agent,
    expected_selected_id: &str,
) -> Result<RemoteLineage, String> {
    let root = response_data(bytes, "lineage")?;
    let agent = parse_agent(required_string(root.get("agent"), "lineage agent")?)?;
    if agent != expected_agent {
        return Err("the peer returned lineage for the wrong agent".to_string());
    }
    let selected_id = required_string(root.get("selected_id"), "selected session ID")?;
    SessionId::new(selected_id)?;
    if selected_id != expected_selected_id {
        return Err("the peer returned lineage for the wrong session".to_string());
    }
    let records = root
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| "the remote lineage has no artifacts array".to_string())?;
    if records.is_empty() || records.len() > MAX_LINEAGE_ARTIFACTS {
        return Err("the remote lineage has an invalid number of artifacts".to_string());
    }
    let mut artifacts = Vec::with_capacity(records.len());
    for record in records {
        let record = record
            .as_object()
            .ok_or_else(|| "the remote lineage contains a non-object artifact".to_string())?;
        let session_id = required_string(record.get("session_id"), "lineage session ID")?;
        SessionId::new(session_id)?;
        let workspace = parse_wire_path(record.get("workspace"), "lineage workspace")?;
        validate_workspace(remote_home, &workspace)?;
        let transcript = parse_wire_path(record.get("transcript"), "lineage transcript")?;
        validate_remote_source(
            remote_home,
            agent,
            &transcript,
            RemoteSourceKind::Transcript,
        )?;
        validate_transcript_name(agent, session_id, &transcript)?;
        let bytes = record
            .get("bytes")
            .and_then(Value::as_u64)
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| "remote lineage artifact has no valid size".to_string())?;
        let sha256 = required_string(record.get("sha256"), "lineage SHA-256")?;
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("remote lineage artifact has an invalid SHA-256".to_string());
        }
        let history_base = match record.get("history_base") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                serde_json::from_value::<HistoryBase>(value.clone())
                    .map_err(|_| "remote lineage has invalid history_base metadata".to_string())?,
            ),
        };
        artifacts.push(ArtifactDescriptor {
            session_id: session_id.to_string(),
            workspace,
            transcript,
            history_base,
            bytes,
            sha256: sha256.to_ascii_lowercase(),
        });
    }
    if artifacts
        .last()
        .is_none_or(|artifact| artifact.session_id != selected_id)
    {
        return Err("remote lineage does not end at the selected session".to_string());
    }
    if agent == Agent::Claude && artifacts.len() != 1 {
        return Err("remote Claude lineage unexpectedly contains ancestors".to_string());
    }
    for pair in artifacts.windows(2) {
        if pair[1]
            .history_base
            .as_ref()
            .is_none_or(|base| base.thread_id != pair[0].session_id)
        {
            return Err("remote Codex lineage is disconnected".to_string());
        }
    }
    Ok(RemoteLineage {
        agent,
        selected_id: selected_id.to_string(),
        artifacts,
    })
}

fn fallback_project_label(workspace: &Path) -> String {
    let raw = workspace
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| workspace.display().to_string());
    clean_wire_text(&raw, MAX_WIRE_PROJECT_CHARS, false).0
}

fn parse_preview_response(bytes: &[u8], max_chars: usize) -> Result<RemotePreview, String> {
    let root = response_data(bytes, "preview")?;
    let title = required_string(root.get("title"), "preview title")?;
    let (title, title_truncated) = clean_wire_text(title, MAX_WIRE_TITLE_CHARS, false);
    let records = root
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "the remote preview has no messages array".to_owned())?;
    if records.len() > MAX_WIRE_MESSAGES {
        return Err("the remote preview has too many messages".to_owned());
    }
    let mut messages = Vec::with_capacity(records.len());
    let mut remaining = max_chars;
    let mut locally_truncated = title_truncated;
    for (index, record) in records.iter().enumerate() {
        let record = record
            .as_object()
            .ok_or_else(|| "the remote preview contains a non-object message".to_owned())?;
        let role = match required_string(record.get("role"), "preview message role")? {
            "user" => RemotePreviewRole::User,
            "assistant" => RemotePreviewRole::Assistant,
            _ => return Err("the remote preview contains an unsupported message role".to_owned()),
        };
        let raw = required_string(record.get("text"), "preview message text")?;
        let (text, truncated) = clean_wire_text(raw, remaining, true);
        locally_truncated |= truncated;
        remaining = remaining.saturating_sub(text.chars().count());
        if !text.is_empty() {
            messages.push(RemotePreviewMessage { role, text });
        }
        if remaining == 0 {
            locally_truncated |= index + 1 < records.len();
            break;
        }
    }
    let truncated = root
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or_else(|| "the remote preview has no truncation status".to_owned())?
        || locally_truncated;
    let warning = match root.get("warning") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let warning = required_string(Some(value), "preview warning")?;
            let (warning, _) = clean_wire_text(warning, 1_024, false);
            (!warning.is_empty()).then_some(warning)
        }
    };
    Ok(RemotePreview {
        title,
        messages,
        truncated,
        warning,
    })
}

fn response_data(
    bytes: &[u8],
    expected_kind: &str,
) -> Result<serde_json::Map<String, Value>, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("the peer returned invalid machine-readable JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "the peer returned a non-object machine response".to_owned())?;
    if object.get("protocol").and_then(Value::as_str) != Some(MACHINE_PROTOCOL) {
        return Err("the peer returned an unsupported machine protocol".to_owned());
    }
    if object.get("version").and_then(Value::as_u64) != Some(MACHINE_PROTOCOL_VERSION) {
        return Err(format!(
            "the peer uses an incompatible agent-hop protocol (need version {MACHINE_PROTOCOL_VERSION})"
        ));
    }
    if object.get("kind").and_then(Value::as_str) != Some(expected_kind) {
        return Err("the peer returned the wrong machine response kind".to_owned());
    }
    if object.get("ok").and_then(Value::as_bool) != Some(true) {
        let reason = object
            .get("error")
            .and_then(Value::as_str)
            .map(|value| clean_wire_text(value, 1_024, false).0)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "the remote agent-hop request failed".to_owned());
        return Err(reason);
    }
    object
        .get("data")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| "the peer returned no machine response data".to_owned())
}

fn required_string<'a>(value: Option<&'a Value>, label: &str) -> Result<&'a str, String> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the remote {label} is not a string"))?;
    if value.len() > MAX_WIRE_PATH.max(MAX_REMOTE_PREVIEW_CHARS * 4) {
        return Err(format!("the remote {label} is unreasonably long"));
    }
    Ok(value)
}

fn parse_agent(value: &str) -> Result<Agent, String> {
    match value {
        "codex" => Ok(Agent::Codex),
        "claude" => Ok(Agent::Claude),
        _ => Err("the remote catalog contains an unknown agent".to_owned()),
    }
}

fn parse_wire_path(value: Option<&Value>, label: &str) -> Result<PathBuf, String> {
    let value = required_string(value, label)?;
    if value.len() > MAX_WIRE_PATH {
        return Err(format!("the remote {label} is unreasonably long"));
    }
    let path = PathBuf::from(value);
    require_normal_absolute(&path, &format!("the remote {label}"))?;
    Ok(path)
}

fn wire_path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn validate_workspace(remote_home: &Path, workspace: &Path) -> Result<(), String> {
    require_normal_absolute(workspace, "the remote session workspace")?;
    workspace
        .strip_prefix(remote_home)
        .map(|_| ())
        .map_err(|_| {
            format!(
                "the remote session workspace is outside the peer home: {}",
                workspace.display()
            )
        })
}

#[derive(Clone, Copy)]
enum RemoteSourceKind {
    Transcript,
    Companion,
}

fn validate_remote_source(
    remote_home: &Path,
    agent: Agent,
    source: &Path,
    kind: RemoteSourceKind,
) -> Result<(), String> {
    require_normal_absolute(remote_home, "the other machine's home directory")?;
    require_normal_absolute(source, "the remote session source")?;
    let roots = match agent {
        Agent::Codex => vec![
            remote_home.join(".codex/sessions"),
            remote_home.join(".codex/archived_sessions"),
        ],
        Agent::Claude => vec![remote_home.join(".claude/projects")],
    };
    if !roots.iter().any(|root| source.starts_with(root)) {
        return Err(format!(
            "the remote session source is outside the {} session stores: {}",
            agent.name(),
            source.display()
        ));
    }
    match kind {
        RemoteSourceKind::Transcript
            if source.extension().and_then(|value| value.to_str()) != Some("jsonl") =>
        {
            Err("the remote session transcript is not a JSONL path".to_owned())
        }
        RemoteSourceKind::Companion if agent != Agent::Claude => {
            Err("only Claude sessions can have remote companions".to_owned())
        }
        _ => Ok(()),
    }
}

fn validate_transcript_name(agent: Agent, id: &str, transcript: &Path) -> Result<(), String> {
    let filename = transcript
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "the remote transcript has no valid UTF-8 filename".to_owned())?;
    let expected = format!("{id}.jsonl");
    let matches = match agent {
        Agent::Codex => filename == expected || filename.ends_with(&format!("-{expected}")),
        Agent::Claude => filename == expected,
    };
    if matches {
        Ok(())
    } else {
        Err("the remote transcript filename does not match its session ID".to_owned())
    }
}

fn require_normal_absolute(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "{label} is not an absolute normalized path: {}",
            path.display()
        ));
    }
    Ok(())
}

fn clean_wire_text(value: &str, max_chars: usize, _multiline: bool) -> (String, bool) {
    let sanitized = sanitize(value);
    let truncated = sanitized.chars().count() > max_chars;
    (sanitized.chars().take(max_chars).collect(), truncated)
}

pub(crate) fn write_companion_export(root: &Path, mut output: impl Write) -> Result<(), String> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("could not inspect session attachments: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err("the session companion is not a safe directory".to_string());
    }
    output
        .write_all(COMPANION_STREAM_MAGIC)
        .map_err(|error| format!("could not write attachment export: {error}"))?;
    let mut pending = vec![(root.to_path_buf(), PathBuf::new())];
    let mut entries = 0usize;
    while let Some((directory, relative_directory)) = pending.pop() {
        let mut children = fs::read_dir(&directory)
            .map_err(|error| format!("could not read session attachments: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not read session attachments: {error}"))?;
        children.sort_by_key(fs::DirEntry::file_name);
        for child in children {
            entries += 1;
            if entries > MAX_COMPANION_ENTRIES {
                return Err("the session companion contains too many entries".to_string());
            }
            let relative = relative_directory.join(child.file_name());
            let relative_text = companion_relative_text(&relative)?;
            let path = child.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("could not inspect session attachments: {error}"))?;
            if metadata.file_type().is_dir() {
                write_companion_header(
                    &mut output,
                    &json!({"kind": "directory", "path": relative_text}),
                )?;
                pending.push((path, relative));
            } else if metadata.file_type().is_file() {
                let mut file = File::open(&path)
                    .map_err(|error| format!("could not open session attachment: {error}"))?;
                let opened = file
                    .metadata()
                    .map_err(|error| format!("could not inspect session attachment: {error}"))?;
                if !same_file(&metadata, &opened) {
                    return Err("a session attachment changed during export".to_string());
                }
                let length = opened.len();
                write_companion_header(
                    &mut output,
                    &json!({"kind": "file", "path": relative_text, "bytes": length}),
                )?;
                let copied = io::copy(&mut Read::by_ref(&mut file).take(length), &mut output)
                    .map_err(|error| format!("could not write attachment export: {error}"))?;
                if copied != length {
                    return Err("a session attachment changed during export".to_string());
                }
            } else {
                return Err(format!(
                    "session attachments contain an unsafe entry: {}",
                    path.display()
                ));
            }
        }
    }
    write_companion_header(&mut output, &json!({"kind": "end"}))
}

fn companion_relative_text(path: &Path) -> Result<&str, String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("a session attachment has an unsafe relative path".to_string());
    }
    let value = path
        .to_str()
        .ok_or_else(|| "a session attachment path is not valid UTF-8".to_string())?;
    if value.len() > MAX_WIRE_PATH {
        return Err("a session attachment path is unreasonably long".to_string());
    }
    Ok(value)
}

fn write_companion_header(output: &mut impl Write, header: &Value) -> Result<(), String> {
    let encoded = serde_json::to_vec(header)
        .map_err(|error| format!("could not encode attachment export: {error}"))?;
    if encoded.len() > MAX_COMPANION_HEADER {
        return Err("an attachment export header is unreasonably long".to_string());
    }
    output
        .write_all(&encoded)
        .and_then(|()| output.write_all(b"\n"))
        .map_err(|error| format!("could not write attachment export: {error}"))
}

fn read_companion_export(input: impl Read, destination: &Path) -> Result<(), String> {
    require_empty_directory(destination)?;
    let mut input = BufReader::new(input);
    if read_companion_line(&mut input)? != COMPANION_STREAM_MAGIC.strip_suffix(b"\n").unwrap() {
        return Err("the peer returned an invalid attachment export".to_string());
    }
    let mut entries = 0usize;
    loop {
        let line = read_companion_line(&mut input)?;
        let header: Value = serde_json::from_slice(&line)
            .map_err(|_| "the peer returned an invalid attachment header".to_string())?;
        let header = header
            .as_object()
            .ok_or_else(|| "the peer returned a non-object attachment header".to_string())?;
        let kind = header
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "the peer returned an attachment header without a kind".to_string())?;
        if kind == "end" {
            let mut trailing = [0_u8; 1];
            if input
                .read(&mut trailing)
                .map_err(|error| format!("could not finish reading remote attachments: {error}"))?
                != 0
            {
                return Err("the peer returned trailing attachment data".to_string());
            }
            return Ok(());
        }
        entries += 1;
        if entries > MAX_COMPANION_ENTRIES {
            return Err("the peer returned too many attachment entries".to_string());
        }
        let relative = header
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "the peer returned an attachment without a path".to_string())?;
        if relative.len() > MAX_WIRE_PATH {
            return Err("the peer returned an unreasonably long attachment path".to_string());
        }
        let relative = Path::new(relative);
        companion_relative_text(relative)?;
        let path = destination.join(relative);
        match kind {
            "directory" => fs::create_dir(&path)
                .map_err(|error| format!("could not stage attachment directory: {error}"))?,
            "file" => {
                let length = header
                    .get("bytes")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "the peer returned an attachment without a size".to_string())?;
                let parent = path.parent().ok_or_else(|| {
                    "the peer returned an attachment without a parent directory".to_string()
                })?;
                let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
                    format!("could not inspect staged attachment directory: {error}")
                })?;
                if !parent_metadata.file_type().is_dir() {
                    return Err("the peer returned an unsafe attachment path".to_string());
                }
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut output = options
                    .open(&path)
                    .map_err(|error| format!("could not stage session attachment: {error}"))?;
                let copied = io::copy(&mut input.by_ref().take(length), &mut output)
                    .map_err(|error| format!("could not stage session attachment: {error}"))?;
                if copied != length {
                    return Err("the peer returned an incomplete session attachment".to_string());
                }
                output
                    .flush()
                    .map_err(|error| format!("could not stage session attachment: {error}"))?;
            }
            _ => return Err("the peer returned an unknown attachment entry kind".to_string()),
        }
    }
}

fn read_companion_line(input: &mut impl BufRead) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    let count = input
        .by_ref()
        .take((MAX_COMPANION_HEADER + 2) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("could not read remote attachments: {error}"))?;
    if count == 0 || !line.ends_with(b"\n") {
        return Err("the peer returned an invalid attachment export header".to_string());
    }
    line.pop();
    if line.len() > MAX_COMPANION_HEADER {
        return Err("the peer returned an oversized attachment export header".to_string());
    }
    Ok(line)
}

fn require_empty_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect attachment staging directory: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err("the attachment staging destination is not a safe directory".to_string());
    }
    if fs::read_dir(path)
        .map_err(|error| format!("could not inspect attachment staging directory: {error}"))?
        .next()
        .is_some()
    {
        return Err("the attachment staging destination is not empty".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    after.file_type().is_file()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
}

#[cfg(not(unix))]
fn same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    after.file_type().is_file() && before.len() == after.len()
}

fn captured_stderr_error(peer: Host, stderr: &Captured, status: ExitStatus) -> String {
    let reason = String::from_utf8_lossy(&stderr.bytes)
        .lines()
        .map(|line| clean_wire_text(line, 1_024, false).0)
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| match status.code() {
            Some(code) => format!("remote command exited with status {code}"),
            None => "remote command was interrupted".to_string(),
        });
    format!("{}: {reason}", peer.name())
}

#[derive(Debug)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Debug)]
struct CapturedOutput {
    status: ExitStatus,
    stdout: Captured,
    stderr: Captured,
}

#[derive(Debug)]
struct RedirectedOutput {
    status: ExitStatus,
    stderr: Captured,
}

fn redirected_output_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Result<RedirectedOutput, String> {
    let mut child = command.spawn().map_err(command_error)?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "could not capture ssh errors".to_owned())?;
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_ERROR_OUTPUT));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_reader.join();
                return Err(format!(
                    "ssh transfer timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_reader.join();
                return Err(format!("could not wait for ssh transfer: {error}"));
            }
        }
    };
    let stderr = stderr_reader
        .join()
        .map_err(|_| "could not finish reading ssh errors".to_owned())?
        .map_err(|error| format!("could not read ssh errors: {error}"))?;
    Ok(RedirectedOutput { status, stderr })
}

fn output_with_timeout(
    mut command: Command,
    stdout_limit: usize,
    timeout: Duration,
) -> Result<CapturedOutput, String> {
    let mut child = command.spawn().map_err(command_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not capture ssh output".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "could not capture ssh errors".to_owned())?;
    let stdout_reader = thread::spawn(move || read_capped(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_capped(stderr, MAX_ERROR_OUTPUT));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("ssh timed out after {} seconds", timeout.as_secs()));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("could not wait for ssh: {error}"));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "could not finish reading ssh output".to_owned())?
        .map_err(|error| format!("could not read ssh output: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "could not finish reading ssh errors".to_owned())?
        .map_err(|error| format!("could not read ssh errors: {error}"))?;
    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_capped(mut reader: impl Read, limit: usize) -> io::Result<Captured> {
    let mut bytes = Vec::with_capacity(limit.min(16 * 1024));
    let mut truncated = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    Ok(Captured { bytes, truncated })
}

fn captured_output_error(peer: Host, output: &CapturedOutput) -> String {
    let reason = String::from_utf8_lossy(&output.stderr.bytes)
        .lines()
        .map(|line| clean_wire_text(line, 1_024, false).0)
        .find(|line| !line.is_empty());
    match (reason, output.status.code()) {
        (Some(reason), _) => format!("{}: {reason}", peer.name()),
        (None, Some(code)) => format!("{}: ssh exited with status {code}", peer.name()),
        (None, None) => format!("{}: ssh was interrupted", peer.name()),
    }
}

fn command_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "ssh is required".to_string()
    } else {
        format!("ssh: {error}")
    }
}

fn output_error(peer: Host, output: &Output) -> String {
    let reason = String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(|line| clean_wire_text(line.trim(), 1_024, false).0)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string());
    match (reason, output.status.code()) {
        (Some(reason), _) => format!("{}: {reason}", peer.name()),
        (None, Some(code)) => format!("{}: ssh exited with status {code}", peer.name()),
        (None, None) => format!("{}: ssh was interrupted", peer.name()),
    }
}

#[cfg(test)]
#[path = "../tests/unit/remote_tests.rs"]
mod tests;
