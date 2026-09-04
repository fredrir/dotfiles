use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cli::Agent;
use crate::preview::sanitize;
use crate::session::{
    CandidateFailureKind, MaterializeFailureKind, Session, SessionId, materialize_for_catalog,
    parse_candidate_for_catalog, path_matches, scan_claude, scan_codex,
};

#[derive(Clone, Debug)]
pub(crate) struct Catalog {
    pub(crate) sessions: Vec<CatalogSession>,
    pub(crate) sources: Vec<CatalogSource>,
    pub(crate) diagnostics: Vec<CatalogDiagnostic>,
}

#[derive(Clone, Debug)]
pub(crate) struct CatalogSession {
    pub(crate) session: Session,
    pub(crate) project: String,
    pub(crate) modified: SystemTime,
    pub(crate) current_workspace: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogSource {
    pub(crate) agent: Agent,
    pub(crate) state: SourceState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SourceState {
    Available { sessions: usize },
    Absent,
    Disabled(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticKind {
    ScanFailure,
    Unsafe,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogDiagnostic {
    pub(crate) agent: Agent,
    pub(crate) kind: DiagnosticKind,
    pub(crate) message: String,
}

/// Scan both local stores without allowing a broken or unavailable store to hide
/// sessions from the other one. Selection must still call `session::discover`
/// immediately before applying a transfer.
pub(crate) fn scan(home: &Path, current_cwd: &Path) -> Catalog {
    let mut catalog = Catalog {
        sessions: Vec::new(),
        sources: Vec::with_capacity(2),
        diagnostics: Vec::new(),
    };
    let mut projects = HashMap::new();
    scan_source(home, current_cwd, Agent::Codex, &mut projects, &mut catalog);
    scan_source(
        home,
        current_cwd,
        Agent::Claude,
        &mut projects,
        &mut catalog,
    );
    catalog.sessions.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.session.agent.name().cmp(right.session.agent.name()))
            .then_with(|| left.session.id.cmp(&right.session.id))
            .then_with(|| left.session.transcript.cmp(&right.session.transcript))
    });
    catalog.diagnostics.sort_by(|left, right| {
        left.agent
            .name()
            .cmp(right.agent.name())
            .then_with(|| diagnostic_rank(left.kind).cmp(&diagnostic_rank(right.kind)))
            .then_with(|| left.message.cmp(&right.message))
    });
    catalog
}

fn scan_source(
    home: &Path,
    current_cwd: &Path,
    agent: Agent,
    projects: &mut HashMap<PathBuf, String>,
    catalog: &mut Catalog,
) {
    if store_is_absent(home, agent) {
        catalog.sources.push(CatalogSource {
            agent,
            state: SourceState::Absent,
        });
        return;
    }
    let result = match agent {
        Agent::Codex => scan_codex(home),
        Agent::Claude => scan_claude(home),
    };
    let scan = match result {
        Ok(scan) => scan,
        Err(error) => {
            catalog.sources.push(CatalogSource {
                agent,
                state: SourceState::Disabled(sanitize(&error)),
            });
            return;
        }
    };

    for error in &scan.errors {
        catalog.diagnostics.push(CatalogDiagnostic {
            agent,
            kind: DiagnosticKind::ScanFailure,
            message: sanitize(error),
        });
    }

    let mut candidates = Vec::new();
    for path in &scan.regular {
        match parse_candidate_for_catalog(agent, path.clone()) {
            Ok(candidate) => candidates.push(candidate),
            // Malformed, incomplete, and otherwise non-resumable transcripts are
            // catalog input noise. They are ignored without becoming rows or
            // user-facing issues.
            Err(error) if error.kind == CandidateFailureKind::Invalid => {}
            Err(error) if error.kind == CandidateFailureKind::Unsafe => {
                catalog.diagnostics.push(CatalogDiagnostic {
                    agent,
                    kind: DiagnosticKind::Unsafe,
                    message: sanitize(&error.message),
                });
            }
            Err(error) => catalog.diagnostics.push(CatalogDiagnostic {
                agent,
                kind: DiagnosticKind::ScanFailure,
                message: sanitize(&error.message),
            }),
        }
    }

    // Count raw filename matches as well as successfully parsed candidates. This
    // deliberately mirrors explicit discovery: malformed and unsafe duplicates
    // disable an otherwise valid candidate rather than presenting a dead choice.
    let mut duplicate_paths: BTreeMap<SessionId, Vec<PathBuf>> = BTreeMap::new();
    for candidate in &candidates {
        duplicate_paths.entry(candidate.id.clone()).or_default();
    }
    for (id, paths) in &mut duplicate_paths {
        paths.extend(
            scan.regular
                .iter()
                .chain(scan.unsafe_entries.iter())
                .filter(|path| path_matches(agent, path, id))
                .cloned(),
        );
        paths.sort();
        paths.dedup();
    }
    let duplicate_entries = duplicate_paths
        .values()
        .filter(|paths| paths.len() > 1)
        .flatten()
        .cloned()
        .collect::<HashSet<_>>();
    for path in &scan.unsafe_entries {
        // A same-ID unsafe entry still disables the otherwise valid candidate,
        // but that is a duplicate condition rather than a separate warning.
        if duplicate_entries.contains(path) {
            continue;
        }
        catalog.diagnostics.push(CatalogDiagnostic {
            agent,
            kind: DiagnosticKind::Unsafe,
            message: sanitize(&format!(
                "ignored unsafe non-regular transcript: {}",
                path.display()
            )),
        });
    }
    let start = catalog.sessions.len();
    for candidate in candidates {
        if duplicate_paths
            .get(&candidate.id)
            .is_some_and(|paths| paths.len() > 1)
        {
            continue;
        }
        let modified = candidate.modified;
        match materialize_for_catalog(home, agent, candidate) {
            Ok(session) => {
                let project = projects
                    .entry(session.workspace.clone())
                    .or_insert_with(|| project_label(&session.workspace))
                    .clone();
                catalog.sessions.push(CatalogSession {
                    current_workspace: session.workspace == current_cwd,
                    session,
                    project,
                    modified,
                });
            }
            Err(error) => match error.kind {
                MaterializeFailureKind::Invalid => {}
                MaterializeFailureKind::Unsafe => catalog.diagnostics.push(CatalogDiagnostic {
                    agent,
                    kind: DiagnosticKind::Unsafe,
                    message: sanitize(&error.message),
                }),
                MaterializeFailureKind::Storage => catalog.diagnostics.push(CatalogDiagnostic {
                    agent,
                    kind: DiagnosticKind::ScanFailure,
                    message: sanitize(&error.message),
                }),
            },
        }
    }
    catalog.sources.push(CatalogSource {
        agent,
        state: SourceState::Available {
            sessions: catalog.sessions.len() - start,
        },
    });
}

fn diagnostic_rank(kind: DiagnosticKind) -> u8 {
    match kind {
        DiagnosticKind::Unsafe => 0,
        DiagnosticKind::ScanFailure => 1,
    }
}

pub(crate) fn diagnostic_summary(
    diagnostics: &[CatalogDiagnostic],
    agent: Agent,
) -> Option<String> {
    let selected = diagnostics
        .iter()
        .filter(|item| item.agent == agent)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return None;
    }
    let unsafe_count = selected
        .iter()
        .filter(|item| item.kind == DiagnosticKind::Unsafe)
        .count();
    let scan_failures = selected
        .iter()
        .filter(|item| item.kind == DiagnosticKind::ScanFailure)
        .count();
    let mut counts = Vec::with_capacity(2);
    if unsafe_count > 0 {
        counts.push(format!(
            "{unsafe_count} unsafe entr{} ignored",
            if unsafe_count == 1 { "y" } else { "ies" }
        ));
    }
    if scan_failures > 0 {
        counts.push(format!(
            "{scan_failures} read failure{}",
            if scan_failures == 1 { "" } else { "s" }
        ));
    }

    let mut details = selected
        .iter()
        .map(|item| bounded_detail(&item.message, 240))
        .collect::<Vec<_>>();
    details.dedup();
    let omitted = details.len().saturating_sub(3);
    details.truncate(3);
    if omitted > 0 {
        details.push(format!("+{omitted} more"));
    }
    Some(format!("{} — {}", counts.join(", "), details.join("; ")))
}

fn bounded_detail(value: &str, max_chars: usize) -> String {
    let mut characters = value.chars();
    let mut bounded = characters.by_ref().take(max_chars).collect::<String>();
    if characters.next().is_some() {
        bounded.push('…');
    }
    bounded
}

fn store_is_absent(home: &Path, agent: Agent) -> bool {
    let root = match agent {
        Agent::Codex => home.join(".codex/sessions"),
        Agent::Claude => home.join(".claude/projects"),
    };
    match fs::symlink_metadata(&root) {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A missing optional root is expected. A non-directory or dangling
            // parent is a broken store and must remain visible as an issue.
            let Some(parent) = root.parent() else {
                return false;
            };
            match fs::symlink_metadata(parent) {
                Ok(metadata) if metadata.file_type().is_dir() => true,
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    fs::metadata(parent).is_ok_and(|target| target.file_type().is_dir())
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(_) => false,
                Ok(_) => false,
            }
        }
        Err(_) => false,
    }
}

/// Name the nearest Git worktree containing `workspace`, falling back to the
/// workspace directory name. A `.git` marker may be a directory or a regular
/// file (as used by worktrees and submodules), but not a symlink or special file.
pub(crate) fn project_label(workspace: &Path) -> String {
    for directory in workspace.ancestors() {
        if fs::symlink_metadata(directory.join(".git"))
            .is_ok_and(|metadata| metadata.file_type().is_dir() || metadata.file_type().is_file())
        {
            return directory_label(directory);
        }
    }
    directory_label(workspace)
}

fn directory_label(directory: &Path) -> String {
    directory
        .file_name()
        .map(|name| sanitize(&name.to_string_lossy()))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| sanitize(&directory.display().to_string()))
}

#[cfg(test)]
#[path = "../tests/unit/catalog_tests.rs"]
mod tests;
