use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::tui::{AgentFilter, OriginFilter, PickerView, PreviewDensity, ScopeFilter};

const VERSION: u64 = 1;

pub(crate) struct Preferences {
    path: PathBuf,
    view: PickerView,
    warning: Option<String>,
}

impl Preferences {
    pub(crate) fn load(home: &Path) -> Self {
        Self::load_from(config_path(home))
    }

    fn load_from(path: PathBuf) -> Self {
        let mut preferences = Self {
            path,
            view: PickerView::default(),
            warning: None,
        };
        let text = match fs::read_to_string(&preferences.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return preferences,
            Err(error) => {
                preferences.warning = Some(format!(
                    "could not read picker preferences {}: {error}",
                    preferences.path.display()
                ));
                return preferences;
            }
        };
        match parse(&text) {
            Ok(view) => preferences.view = view,
            Err(error) => {
                preferences.warning = Some(format!(
                    "could not read picker preferences {}: {error}",
                    preferences.path.display()
                ));
            }
        }
        preferences
    }

    pub(crate) fn view(&self) -> PickerView {
        self.view
    }

    pub(crate) fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub(crate) fn save(&mut self, view: PickerView) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "picker preferences path has no parent directory".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        let document = json!({
            "version": VERSION,
            "origin": origin_name(view.origin),
            "agent": agent_name(view.agent),
            "scope": scope_name(view.scope),
            "preview": view.preview.label(),
        });
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|error| format!("could not create picker preferences: {error}"))?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), &document)
            .map_err(|error| format!("could not encode picker preferences: {error}"))?;
        temporary
            .as_file_mut()
            .write_all(b"\n")
            .map_err(|error| format!("could not finish picker preferences: {error}"))?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|error| format!("could not finish picker preferences: {error}"))?;
        temporary.persist(&self.path).map_err(|error| {
            format!(
                "could not replace picker preferences {}: {}",
                self.path.display(),
                error.error
            )
        })?;
        self.view = view;
        self.warning = None;
        Ok(())
    }
}

fn config_path(home: &Path) -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() && Path::new(&value).is_absolute() => PathBuf::from(value),
        _ => home.join(".config"),
    };
    base.join("agent-hop/view.json")
}

fn parse(text: &str) -> Result<PickerView, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err("unsupported picker preferences version".to_string());
    }
    Ok(PickerView {
        origin: match field(&value, "origin")? {
            "all" => OriginFilter::All,
            "local" => OriginFilter::Local,
            "remote" => OriginFilter::Remote,
            _ => return Err("picker preference origin is invalid".to_string()),
        },
        agent: match field(&value, "agent")? {
            "all" => AgentFilter::All,
            "codex" => AgentFilter::Codex,
            "claude" => AgentFilter::Claude,
            _ => return Err("picker preference agent is invalid".to_string()),
        },
        scope: match field(&value, "scope")? {
            "all" => ScopeFilter::All,
            "project" => ScopeFilter::CurrentProject,
            "favorites" => ScopeFilter::Favorites,
            _ => return Err("picker preference scope is invalid".to_string()),
        },
        preview: match field(&value, "preview")? {
            "conversation" => PreviewDensity::Conversation,
            "compact" => PreviewDensity::Compact,
            "metadata" => PreviewDensity::Metadata,
            _ => return Err("picker preference preview is invalid".to_string()),
        },
    })
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("picker preference {name} is missing"))
}

fn origin_name(filter: OriginFilter) -> &'static str {
    match filter {
        OriginFilter::All => "all",
        OriginFilter::Local => "local",
        OriginFilter::Remote => "remote",
    }
}

fn agent_name(filter: AgentFilter) -> &'static str {
    match filter {
        AgentFilter::All => "all",
        AgentFilter::Codex => "codex",
        AgentFilter::Claude => "claude",
    }
}

fn scope_name(filter: ScopeFilter) -> &'static str {
    match filter {
        ScopeFilter::All => "all",
        ScopeFilter::CurrentProject => "project",
        ScopeFilter::Favorites => "favorites",
    }
}

#[cfg(test)]
#[path = "../tests/unit/preferences_tests.rs"]
mod tests;
