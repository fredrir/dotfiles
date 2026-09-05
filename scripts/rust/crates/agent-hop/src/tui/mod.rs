//! Interactive session picker.
//!
//! The picker deliberately knows nothing about transcript formats or SSH.  A
//! [`CatalogSource`] supplies lightweight rows and resolves the selected
//! session's preview only when it is needed.

mod model;
mod terminal;
mod view;

use std::io::{self, IsTerminal};

use crate::cli::{Agent, ColorMode};

pub(crate) use model::{Effect, Model, UiEvent};
pub(crate) use terminal::run;
pub(crate) use view::render;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Origin {
    Local,
    Remote,
}

impl Origin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreviewRole {
    User,
    Assistant,
}

impl PreviewRole {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::User => "you",
            Self::Assistant => "agent",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreviewLine {
    pub(crate) role: PreviewRole,
    pub(crate) text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Preview {
    pub(crate) lines: Vec<PreviewLine>,
    /// True when only the tail of a large transcript was read.
    pub(crate) truncated: bool,
    /// Non-fatal parse or consistency warning associated with this preview.
    pub(crate) warning: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SessionEntry {
    /// Stable across catalog refreshes. Include origin/host when IDs can clash.
    pub(crate) key: String,
    pub(crate) id: String,
    pub(crate) agent: Agent,
    pub(crate) origin: Origin,
    pub(crate) host: Option<String>,
    pub(crate) project: String,
    pub(crate) workspace: String,
    pub(crate) title: String,
    /// A compact, already-localized age or timestamp (for example `8m`).
    pub(crate) updated: String,
    pub(crate) current_project: bool,
    pub(crate) favorite: bool,
    /// A disabled row remains inspectable but cannot be applied.
    pub(crate) disabled_reason: Option<String>,
    /// Non-fatal diagnostic associated with just this row.
    pub(crate) warning: Option<String>,
    /// Exact timestamp used only for deterministic catalog ordering.
    pub(crate) sort_timestamp: u128,
}

impl SessionEntry {
    pub(crate) fn searchable_text(&self) -> String {
        format!(
            "{} {} {} {} {} {} {} {} {}",
            self.title,
            self.project,
            self.workspace,
            self.id,
            self.agent.name(),
            self.origin.label(),
            self.host.as_deref().unwrap_or_default(),
            self.updated,
            if self.favorite {
                "favorite starred"
            } else {
                ""
            },
        )
    }

    /// A plain-text description suitable for both terminal display and the
    /// clipboard. Catalog strings are cleaned at this boundary as a final
    /// guard against control characters reaching the user's terminal.
    pub(crate) fn complete_description(&self) -> String {
        format!(
            "Summary: {}\nAgent: {}\nUpdated: {}\nProject: {}\nOrigin: {}\nHost: {}\nWorkspace: {}\nFavorite: {}\nSession ID: {}",
            clean(&self.title),
            self.agent.name(),
            clean(&self.updated),
            clean(&self.project),
            self.origin.label(),
            self.host
                .as_deref()
                .map(clean)
                .unwrap_or_else(|| "local".into()),
            clean(&self.workspace),
            if self.favorite { "yes" } else { "no" },
            clean(&self.id),
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CatalogSnapshot {
    pub(crate) sessions: Vec<SessionEntry>,
    pub(crate) warnings: Vec<String>,
}

impl CatalogSnapshot {
    pub(crate) fn merge(mut self, mut other: Self) -> Self {
        self.sessions.append(&mut other.sessions);
        self.warnings.append(&mut other.warnings);
        self.sessions.sort_by(|left, right| {
            right
                .sort_timestamp
                .cmp(&left.sort_timestamp)
                .then_with(|| left.host.cmp(&right.host))
                .then_with(|| left.agent.name().cmp(right.agent.name()))
                .then_with(|| left.id.cmp(&right.id))
        });
        self.warnings.sort();
        self.warnings.dedup();
        self
    }
}

/// Boundary between discovery/storage and the interactive picker.
pub(crate) trait CatalogSource {
    fn refresh_local(&mut self) -> Result<CatalogSnapshot, String>;
    fn refresh_remote(&mut self) -> Result<CatalogSnapshot, String>;
    fn preview(&mut self, key: &str) -> Result<Preview, String>;
}

/// Small synchronous mutation boundary. Favorite files are local and tiny;
/// committing here means a quick quit cannot strand a write behind SSH work.
pub(crate) trait FavoriteStore {
    fn set_favorite(&mut self, key: &str, favorite: bool) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PickerOptions {
    pub(crate) color: bool,
    pub(crate) reduced_motion: bool,
    pub(crate) initial_action: PickerAction,
    pub(crate) initial_view: PickerView,
}

impl Default for PickerOptions {
    fn default() -> Self {
        Self {
            color: ColorMode::Auto.enabled(io::stdout().is_terminal()),
            reduced_motion: reduced_motion_requested(),
            initial_action: PickerAction::HopAndOpen,
            initial_view: PickerView::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum OriginFilter {
    #[default]
    All,
    Local,
    Remote,
}

impl OriginFilter {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Local,
            Self::Local => Self::Remote,
            Self::Remote => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AgentFilter {
    #[default]
    All,
    Codex,
    Claude,
}

impl AgentFilter {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Codex,
            Self::Codex => Self::Claude,
            Self::Claude => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ScopeFilter {
    #[default]
    All,
    CurrentProject,
    Favorites,
}

impl ScopeFilter {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::CurrentProject,
            Self::CurrentProject => Self::Favorites,
            Self::Favorites => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PreviewDensity {
    #[default]
    Conversation,
    Compact,
    Metadata,
}

impl PreviewDensity {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Conversation => Self::Compact,
            Self::Compact => Self::Metadata,
            Self::Metadata => Self::Conversation,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Compact => "compact",
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PickerView {
    pub(crate) origin: OriginFilter,
    pub(crate) agent: AgentFilter,
    pub(crate) scope: ScopeFilter,
    pub(crate) preview: PreviewDensity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerAction {
    HopAndOpen,
    CopyOnly,
    DryRun,
}

impl PickerAction {
    pub(crate) const ALL: [Self; 3] = [Self::HopAndOpen, Self::CopyOnly, Self::DryRun];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::HopAndOpen => "Hop & open",
            Self::CopyOnly => "Copy only",
            Self::DryRun => "Dry run",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::HopAndOpen => "copy the session and continue it on the destination",
            Self::CopyOnly => "copy the session without starting the agent",
            Self::DryRun => "show the transfer plan without changing anything",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PickedSession {
    pub(crate) session: SessionEntry,
    pub(crate) action: PickerAction,
    pub(crate) view: PickerView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PickerOutcome {
    Cancelled(PickerView),
    Picked(Box<PickedSession>),
}

impl PickerOutcome {
    pub(crate) fn view(&self) -> PickerView {
        match self {
            Self::Cancelled(view) => *view,
            Self::Picked(picked) => picked.view,
        }
    }
}

pub(crate) fn capable() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var("TERM")
            .ok()
            .is_none_or(|term| !term.eq_ignore_ascii_case("dumb"))
}

fn reduced_motion_requested() -> bool {
    [
        "AGENT_HOP_REDUCED_MOTION",
        "PREFERS_REDUCED_MOTION",
        "REDUCE_MOTION",
        "REDUCED_MOTION",
    ]
    .into_iter()
    .any(|name| std::env::var(name).ok().is_some_and(|value| flag(&value)))
}

fn flag(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

fn clean(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            character if character.is_control() => '�',
            character => character,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
