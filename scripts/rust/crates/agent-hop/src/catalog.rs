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
mod tests {
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    fn write(path: &Path, content: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn codex(home: &Path, day: &str, id: &str, workspace: &Path) -> PathBuf {
        let path = home.join(format!(
            ".codex/sessions/2026/09/{day}/rollout-2026-09-{day}T00-00-00-{id}.jsonl"
        ));
        write(
            &path,
            format!(
                "{}\n",
                json!({"type":"session_meta","payload":{"id":id,"cwd":workspace,"thread_source":"user","source":"cli"}})
            ),
        );
        path
    }

    fn claude(home: &Path, project: &str, id: &str, workspace: &Path) -> PathBuf {
        let path = home.join(format!(".claude/projects/{project}/{id}.jsonl"));
        write(
            &path,
            format!("{}\n", json!({"sessionId":id,"cwd":workspace})),
        );
        path
    }

    fn stores(home: &Path) {
        fs::create_dir_all(home.join(".codex/sessions/2026/09/01")).unwrap();
        fs::create_dir_all(home.join(".claude/projects/project")).unwrap();
    }

    #[test]
    fn scans_both_stores_and_orders_newest_first_deterministically() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let current = home.join("current");
        let other = home.join("other");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&other).unwrap();
        stores(&home);
        let old_current = codex(&home, "01", "codex-current", &current);
        let new_other = claude(&home, "project", "claude-other", &other);
        fs::File::options()
            .write(true)
            .open(&old_current)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(1))
            .unwrap();
        fs::File::options()
            .write(true)
            .open(&new_other)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(9))
            .unwrap();

        let catalog = scan(&home, &current);
        assert_eq!(catalog.sessions.len(), 2);
        assert_eq!(catalog.sessions[0].session.id.as_str(), "claude-other");
        assert!(!catalog.sessions[0].current_workspace);
        assert_eq!(catalog.sessions[1].session.id.as_str(), "codex-current");
        assert!(catalog.sessions[1].current_workspace);
        assert_eq!(catalog.sources.len(), 2);
    }

    #[test]
    fn absent_optional_source_does_not_hide_the_other_source() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let workspace = home.join("work");
        fs::create_dir_all(&workspace).unwrap();
        claude(&home, "project", "only-claude", &workspace);

        let catalog = scan(&home, &workspace);
        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(catalog.sources[0].state, SourceState::Absent);
        assert_eq!(
            catalog.sources[1].state,
            SourceState::Available { sessions: 1 }
        );
    }

    #[test]
    fn malformed_files_are_silently_ignored_and_valid_files_remain() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let workspace = home.join("work");
        fs::create_dir_all(&workspace).unwrap();
        stores(&home);
        write(
            &home.join(".codex/sessions/2026/09/01/rollout-broken.jsonl"),
            "{broken\n",
        );
        codex(&home, "01", "valid", &workspace);

        let catalog = scan(&home, &workspace);
        assert_eq!(catalog.sessions.len(), 1);
        assert!(catalog.diagnostics.is_empty());
    }

    #[test]
    fn malformed_duplicates_disable_a_valid_id_without_becoming_issues() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let workspace = home.join("work");
        fs::create_dir_all(&workspace).unwrap();
        stores(&home);
        codex(&home, "01", "same-id", &workspace);
        let duplicate =
            home.join(".codex/sessions/2026/09/02/rollout-2026-09-02T00-00-00-same-id.jsonl");
        write(&duplicate, "not json\n");

        let catalog = scan(&home, &workspace);
        assert!(catalog.sessions.is_empty());
        assert!(catalog.diagnostics.is_empty());
        assert_eq!(
            catalog.sources[0].state,
            SourceState::Available { sessions: 0 }
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_duplicates_disable_a_valid_id_without_becoming_issues() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let workspace = home.join("work");
        fs::create_dir_all(&workspace).unwrap();
        stores(&home);
        codex(&home, "01", "same-id", &workspace);
        let target = home.join("target.jsonl");
        write(&target, "not a transcript\n");
        let duplicate =
            home.join(".codex/sessions/2026/09/02/rollout-2026-09-02T00-00-00-same-id.jsonl");
        fs::create_dir_all(duplicate.parent().unwrap()).unwrap();
        symlink(&target, duplicate).unwrap();

        let catalog = scan(&home, &workspace);
        assert!(catalog.sessions.is_empty());
        assert!(catalog.diagnostics.is_empty());
        assert_eq!(
            catalog.sources[0].state,
            SourceState::Available { sessions: 0 }
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_entries_are_diagnostic_not_followed() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        stores(&home);
        let target = home.join("secret");
        write(&target, "secret");
        let link = home.join(".claude/projects/project/unsafe.jsonl");
        symlink(&target, &link).unwrap();

        let catalog = scan(&home, &home);
        assert!(catalog.sessions.is_empty());
        assert!(
            catalog
                .diagnostics
                .iter()
                .any(|item| item.kind == DiagnosticKind::Unsafe)
        );
    }

    #[test]
    fn broken_store_is_distinct_from_an_absent_optional_store() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir_all(home.join(".codex")).unwrap();
        write(&home.join(".codex/sessions"), "not a directory");

        let catalog = scan(&home, &home);
        assert!(matches!(catalog.sources[0].state, SourceState::Disabled(_)));
        assert_eq!(catalog.sources[1].state, SourceState::Absent);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_store_parent_is_broken_not_absent() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        fs::create_dir_all(&home).unwrap();
        symlink(home.join("missing-codex-root"), home.join(".codex")).unwrap();

        let catalog = scan(&home, &home);
        assert!(matches!(catalog.sources[0].state, SourceState::Disabled(_)));
        assert_eq!(catalog.sources[1].state, SourceState::Absent);
    }

    #[cfg(unix)]
    #[test]
    fn valid_symlinked_store_parent_can_have_an_absent_optional_child() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let state = temporary.path().join("codex-state");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir(&state).unwrap();
        symlink(&state, home.join(".codex")).unwrap();

        let catalog = scan(&home, &home);
        assert_eq!(catalog.sources[0].state, SourceState::Absent);
        assert_eq!(catalog.sources[1].state, SourceState::Absent);
    }

    #[test]
    fn diagnostic_summaries_keep_bounded_actionable_details() {
        let diagnostics = vec![
            CatalogDiagnostic {
                agent: Agent::Codex,
                kind: DiagnosticKind::ScanFailure,
                message: "could not read /work/session.jsonl: permission denied".to_owned(),
            },
            CatalogDiagnostic {
                agent: Agent::Codex,
                kind: DiagnosticKind::Unsafe,
                message: "ignored unsafe /work/link.jsonl".to_owned(),
            },
        ];
        let summary = diagnostic_summary(&diagnostics, Agent::Codex).unwrap();
        assert!(summary.contains("1 unsafe entry ignored, 1 read failure"));
        assert!(summary.contains("permission denied"));
        assert!(summary.contains("/work/link.jsonl"));
        assert!(diagnostic_summary(&diagnostics, Agent::Claude).is_none());
    }

    #[test]
    fn project_labels_use_the_nearest_git_root_and_fallback_to_workspace() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        let nested_repo = root.join("packages/app");
        let workspace = nested_repo.join("src/deep");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(nested_repo.join(".git"), "gitdir: elsewhere").unwrap();

        assert_eq!(project_label(&workspace), "app");

        fs::remove_file(nested_repo.join(".git")).unwrap();
        assert_eq!(project_label(&workspace), "repo");

        fs::remove_dir(root.join(".git")).unwrap();
        assert_eq!(project_label(&workspace), "deep");
    }
}
