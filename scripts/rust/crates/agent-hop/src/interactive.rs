use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hostkit::Host;

use crate::catalog::{self, DiagnosticKind, SourceState};
use crate::cli::{Agent, ColorMode};
use crate::favorites::Favorites;
use crate::plan;
use crate::preferences::Preferences;
use crate::preview::{self, PreviewLimits};
use crate::remote::{MAX_REMOTE_SESSIONS, Remote, RemotePreviewRole};
use crate::session::Session;
use crate::tui::{
    CatalogSnapshot, CatalogSource, Origin, PickerAction, PickerOptions, PickerOutcome, Preview,
    PreviewLine, PreviewRole, SessionEntry,
};

#[derive(Clone)]
pub(crate) struct BrowserSource {
    this: Host,
    peer: Host,
    home: PathBuf,
    current_cwd: PathBuf,
    targets: HashMap<String, Session>,
    preview_cache: HashMap<String, Preview>,
    favorites: Favorites,
    preferences_warning: Option<String>,
    saved_view: crate::tui::PickerView,
}

impl BrowserSource {
    pub(crate) fn new(this: Host, home: PathBuf, current_cwd: PathBuf) -> BrowserSource {
        let preferences = Preferences::load(&home);
        BrowserSource {
            this,
            peer: this.peer(),
            favorites: Favorites::load(&home),
            preferences_warning: preferences.warning().map(str::to_string),
            saved_view: preferences.view(),
            home,
            current_cwd,
            targets: HashMap::new(),
            preview_cache: HashMap::new(),
        }
    }

    fn local_entries(&mut self, warnings: &mut Vec<String>) -> Vec<SessionEntry> {
        let found = catalog::scan(&self.home, &self.current_cwd);
        for source in &found.sources {
            if let SourceState::Disabled(reason) = &source.state {
                warnings.push(format!(
                    "{} sessions unavailable on {}: {reason}",
                    source.agent.name(),
                    self.this.name()
                ));
            }
        }
        let duplicate = found
            .diagnostics
            .iter()
            .filter(|item| item.kind == DiagnosticKind::Duplicate)
            .count();
        let unsafe_count = found
            .diagnostics
            .iter()
            .filter(|item| item.kind == DiagnosticKind::Unsafe)
            .count();
        let invalid = found
            .diagnostics
            .iter()
            .filter(|item| item.kind == DiagnosticKind::Invalid)
            .count();
        if duplicate + unsafe_count + invalid > 0 {
            warnings.push(format!(
                "{} hidden local session issue{}: {duplicate} duplicate, {unsafe_count} unsafe, {invalid} invalid",
                duplicate + unsafe_count + invalid,
                if duplicate + unsafe_count + invalid == 1 { "" } else { "s" }
            ));
        }

        found
            .sessions
            .into_iter()
            .map(|entry| {
                let session = entry.session;
                let key = key(self.this, session.agent, session.id.as_str());
                let title = short_title(&session);
                let favorite = self.favorites.contains(&key);
                let workspace = plan::display(&session.workspace, &self.home);
                let project = project_label(&session.workspace, &self.home);
                let row = SessionEntry {
                    key: key.clone(),
                    id: session.id.as_str().to_string(),
                    agent: session.agent,
                    origin: Origin::Local,
                    host: Some(self.this.name().to_string()),
                    project,
                    workspace,
                    title,
                    updated: relative_age(entry.modified),
                    current_project: entry.current_workspace,
                    favorite,
                    disabled_reason: None,
                    warning: None,
                    sort_timestamp: timestamp_millis(entry.modified),
                };
                self.targets.insert(key, session);
                row
            })
            .chain(found.diagnostics.iter().enumerate().map(|(index, item)| {
                let reason = item.message.clone();
                SessionEntry {
                    key: format!(
                        "{}:diagnostic:{}:{index}",
                        self.this.name(),
                        item.agent.name()
                    ),
                    id: "diagnostic".to_string(),
                    agent: item.agent,
                    origin: Origin::Local,
                    host: Some(self.this.name().to_string()),
                    project: "session store".to_string(),
                    workspace: "diagnostic".to_string(),
                    title: match item.kind {
                        DiagnosticKind::Duplicate => "Duplicate session entry",
                        DiagnosticKind::Unsafe => "Unsafe session entry",
                        DiagnosticKind::Invalid => "Unreadable session entry",
                    }
                    .to_string(),
                    updated: "—".to_string(),
                    current_project: false,
                    favorite: false,
                    disabled_reason: Some(reason),
                    warning: None,
                    sort_timestamp: 0,
                }
            }))
            .collect()
    }

    fn remote_entries(&mut self, warnings: &mut Vec<String>) -> Vec<SessionEntry> {
        let remote = Remote::new(self.peer);
        let remote_home = match remote.home_noninteractive() {
            Ok(home) => home,
            Err(error) => {
                warnings.push(format!(
                    "remote sessions on {} are unavailable: {error}",
                    self.peer.name()
                ));
                return Vec::new();
            }
        };
        let remote_workspace = crate::session::workspace_relative(&self.home, &self.current_cwd)
            .ok()
            .map(|relative| remote_home.join(relative));
        let found = match remote.catalog(
            &remote_home,
            remote_workspace.as_deref(),
            MAX_REMOTE_SESSIONS,
        ) {
            Ok(catalog) => catalog,
            Err(error) => {
                warnings.push(format!(
                    "remote sessions on {} are unavailable: {error}",
                    self.peer.name()
                ));
                return Vec::new();
            }
        };
        warnings.extend(found.warnings);
        found
            .sessions
            .into_iter()
            .map(|session| {
                let key = key(self.peer, session.agent, &session.id);
                let favorite = self.favorites.contains(&key);
                let current_project = remote_workspace
                    .as_deref()
                    .is_some_and(|workspace| workspace == session.workspace);
                SessionEntry {
                    key: key.clone(),
                    id: session.id.clone(),
                    agent: session.agent,
                    origin: Origin::Remote,
                    host: Some(self.peer.name().to_string()),
                    project: project_label(&session.workspace, &remote_home),
                    workspace: plan::display(&session.workspace, &remote_home),
                    title: session.title,
                    updated: relative_age(UNIX_EPOCH + Duration::from_millis(session.modified_ms)),
                    current_project,
                    favorite,
                    disabled_reason: None,
                    warning: None,
                    sort_timestamp: u128::from(session.modified_ms),
                }
            })
            .collect()
    }
}

impl CatalogSource for BrowserSource {
    fn refresh_local(&mut self) -> Result<CatalogSnapshot, String> {
        self.favorites = Favorites::load(&self.home);
        self.targets.clear();
        self.preview_cache.clear();
        let mut warnings = Vec::new();
        if let Some(warning) = self.favorites.warning() {
            warnings.push(warning.to_string());
        }
        if let Some(warning) = &self.preferences_warning {
            warnings.push(warning.clone());
        }
        let sessions = self.local_entries(&mut warnings);
        Ok(CatalogSnapshot { sessions, warnings }.merge(CatalogSnapshot::default()))
    }

    fn refresh_remote(&mut self) -> Result<CatalogSnapshot, String> {
        let mut warnings = Vec::new();
        let sessions = self.remote_entries(&mut warnings);
        Ok(CatalogSnapshot { sessions, warnings }.merge(CatalogSnapshot::default()))
    }

    fn preview(&mut self, key: &str) -> Result<Preview, String> {
        if let Some(preview) = self.preview_cache.get(key) {
            return Ok(preview.clone());
        }
        let preview = if let Some(session) = self.targets.get(key) {
            local_preview(session)?
        } else {
            let (agent, id) = remote_key(self.peer, key)?;
            let found = Remote::new(self.peer).preview(agent, id, 12_000)?;
            Preview {
                lines: found
                    .messages
                    .into_iter()
                    .map(|message| PreviewLine {
                        role: match message.role {
                            RemotePreviewRole::User => PreviewRole::User,
                            RemotePreviewRole::Assistant => PreviewRole::Assistant,
                        },
                        text: message.text,
                    })
                    .collect(),
                truncated: found.truncated,
                warning: found.warning,
            }
        };
        self.preview_cache.insert(key.to_string(), preview.clone());
        Ok(preview)
    }
}

pub(crate) fn browse(
    color: ColorMode,
    default_action: PickerAction,
) -> Result<PickerOutcome, String> {
    let this = Host::this()?;
    let home = crate::local_home()?;
    let current_cwd = crate::physical_current_directory()?;
    let source = BrowserSource::new(this, home.clone(), current_cwd);
    let options = PickerOptions {
        color: color.enabled(true),
        reduced_motion: PickerOptions::default().reduced_motion,
        initial_action: default_action,
        initial_view: source.saved_view,
    };
    let favorites = Favorites::load(&home);
    let outcome = crate::tui::run(source, favorites, options)?;
    if let Err(error) = Preferences::load(&home).save(outcome.view()) {
        eprintln!("agent-hop: warning: {error}");
    }
    Ok(outcome)
}

pub(crate) fn list() -> Result<(), String> {
    let this = Host::this()?;
    let home = crate::local_home()?;
    let current_cwd = crate::physical_current_directory()?;
    let mut source = BrowserSource::new(this, home, current_cwd);
    let snapshot = source.refresh_local()?.merge(source.refresh_remote()?);
    println!("HOST\tORIGIN\tAGENT\tUPDATED\tWORKSPACE\tTITLE\tSESSION");
    for session in snapshot.sessions.iter().filter(|session| listable(session)) {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            session.host.as_deref().unwrap_or("-"),
            session.origin.label(),
            session.agent.name(),
            session.updated,
            clean_field(&session.workspace),
            clean_field(&session.title),
            clean_field(&session.id),
        );
    }
    for warning in snapshot.warnings {
        eprintln!("agent-hop: warning: {}", clean_field(&warning));
    }
    Ok(())
}

fn listable(session: &SessionEntry) -> bool {
    session.disabled_reason.is_none()
}

fn local_preview(session: &Session) -> Result<Preview, String> {
    let found = preview::load(session, PreviewLimits::default())?;
    Ok(Preview {
        lines: found
            .messages
            .into_iter()
            .map(|message| PreviewLine {
                role: match message.role {
                    preview::PreviewRole::User => PreviewRole::User,
                    preview::PreviewRole::Assistant => PreviewRole::Assistant,
                },
                text: message.text,
            })
            .collect(),
        truncated: found.truncated,
        warning: (found.skipped_records > 0).then(|| {
            format!(
                "Ignored {} malformed or incomplete transcript record{}",
                found.skipped_records,
                if found.skipped_records == 1 { "" } else { "s" }
            )
        }),
    })
}

fn short_title(session: &Session) -> String {
    let limits = PreviewLimits {
        head_bytes: 64 * 1024,
        tail_bytes: 0,
        max_records_per_window: 1_024,
        max_messages: 1,
        max_message_chars: 256,
        max_title_chars: 96,
    };
    preview::load(session, limits)
        .map(|preview| preview.title)
        .unwrap_or_else(|_| "Untitled session".to_string())
}

fn project_label(workspace: &Path, home: &Path) -> String {
    if workspace == home {
        return "~".to_string();
    }
    workspace
        .file_name()
        .and_then(|name| name.to_str())
        .map(preview::sanitize)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| plan::display(workspace, home))
}

fn key(host: Host, agent: Agent, id: &str) -> String {
    format!("{}:{}:{id}", host.name(), agent.name())
}

fn remote_key(peer: Host, key: &str) -> Result<(Agent, &str), String> {
    let remainder = key
        .strip_prefix(&format!("{}:", peer.name()))
        .ok_or_else(|| "the selected session is no longer in the catalog".to_string())?;
    let (agent, id) = remainder
        .split_once(':')
        .ok_or_else(|| "the selected remote session key is invalid".to_string())?;
    let agent = match agent {
        "codex" => Agent::Codex,
        "claude" => Agent::Claude,
        _ => return Err("the selected remote session agent is invalid".to_string()),
    };
    crate::session::SessionId::new(id)?;
    Ok((agent, id))
}

fn relative_age(modified: SystemTime) -> String {
    let elapsed = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default()
        .as_secs();
    match elapsed {
        0..=59 => "now".to_string(),
        60..=3_599 => format!("{}m", elapsed / 60),
        3_600..=86_399 => format!("{}h", elapsed / 3_600),
        _ => format!("{}d", elapsed / 86_400),
    }
}

fn timestamp_millis(modified: SystemTime) -> u128 {
    modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn clean_field(value: &str) -> String {
    preview::sanitize(value).replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_ages_are_compact() {
        let now = SystemTime::now();
        assert_eq!(relative_age(now), "now");
        assert_eq!(relative_age(now - Duration::from_secs(120)), "2m");
        assert!(timestamp_millis(now) > timestamp_millis(UNIX_EPOCH));
    }

    #[test]
    fn stable_keys_include_the_source_host_and_agent() {
        assert_eq!(key(Host::Macie, Agent::Codex, "id"), "macie:codex:id");
        assert_ne!(
            key(Host::Macie, Agent::Codex, "id"),
            key(Host::Archie, Agent::Codex, "id")
        );
    }

    #[test]
    fn remote_keys_preserve_ids_containing_colons() {
        let (agent, id) = remote_key(Host::Archie, "archie:claude:id:with:colons").unwrap();
        assert_eq!(agent, Agent::Claude);
        assert_eq!(id, "id:with:colons");
    }

    #[test]
    fn plain_listing_omits_disabled_diagnostic_rows() {
        let valid = SessionEntry {
            key: "macie:codex:valid".into(),
            id: "valid".into(),
            agent: Agent::Codex,
            origin: Origin::Local,
            host: Some("macie".into()),
            project: "dotfiles".into(),
            workspace: "~/dotfiles".into(),
            title: "A valid session".into(),
            updated: "now".into(),
            current_project: true,
            favorite: false,
            disabled_reason: None,
            warning: None,
            sort_timestamp: 1,
        };
        let mut diagnostic = valid.clone();
        diagnostic.key = "macie:diagnostic:codex:0".into();
        diagnostic.id = "diagnostic".into();
        diagnostic.disabled_reason = Some("malformed transcript".into());

        let listed = [valid, diagnostic]
            .into_iter()
            .filter(listable)
            .collect::<Vec<_>>();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "valid");
    }
}
