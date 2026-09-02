use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cli::Agent;
use crate::preview::sanitize;
use crate::session::{
    Session, SessionId, materialize, parse_candidate, path_matches, scan_claude, scan_codex,
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
    Disabled(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticKind {
    Invalid,
    Unsafe,
    Duplicate,
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
    scan_source(home, current_cwd, Agent::Codex, &mut catalog);
    scan_source(home, current_cwd, Agent::Claude, &mut catalog);
    catalog.sessions.sort_by(|left, right| {
        right
            .current_workspace
            .cmp(&left.current_workspace)
            .then_with(|| right.modified.cmp(&left.modified))
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

fn scan_source(home: &Path, current_cwd: &Path, agent: Agent, catalog: &mut Catalog) {
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

    for path in &scan.unsafe_entries {
        catalog.diagnostics.push(CatalogDiagnostic {
            agent,
            kind: DiagnosticKind::Unsafe,
            message: sanitize(&format!(
                "ignored unsafe non-regular transcript: {}",
                path.display()
            )),
        });
    }
    for error in &scan.errors {
        catalog.diagnostics.push(CatalogDiagnostic {
            agent,
            kind: DiagnosticKind::Invalid,
            message: sanitize(error),
        });
    }

    let mut candidates = Vec::new();
    for path in &scan.regular {
        match parse_candidate(agent, path.clone(), None) {
            Ok(candidate) => candidates.push(candidate),
            // These are well-formed Codex transcripts, but not resumable
            // top-level CLI sessions (for example subagents or editor-owned
            // threads). They are intentionally outside this catalog.
            Err(error)
                if agent == Agent::Codex
                    && error == "the first record is not a user-authored Codex CLI session" => {}
            Err(error) => catalog.diagnostics.push(CatalogDiagnostic {
                agent,
                kind: DiagnosticKind::Invalid,
                message: sanitize(&format!("ignored {}: {error}", path.display())),
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
    for (id, paths) in duplicate_paths.iter().filter(|(_, paths)| paths.len() > 1) {
        catalog.diagnostics.push(CatalogDiagnostic {
            agent,
            kind: DiagnosticKind::Duplicate,
            message: sanitize(&format!(
                "session {id} is disabled: found {} transcript files",
                paths.len()
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
        match materialize(home, agent, candidate) {
            Ok(session) => catalog.sessions.push(CatalogSession {
                current_workspace: session.workspace == current_cwd,
                session,
                modified,
            }),
            Err(error) => catalog.diagnostics.push(CatalogDiagnostic {
                agent,
                kind: DiagnosticKind::Invalid,
                message: sanitize(&error),
            }),
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
        DiagnosticKind::Duplicate => 0,
        DiagnosticKind::Unsafe => 1,
        DiagnosticKind::Invalid => 2,
    }
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
    fn scans_both_stores_and_orders_current_then_newest_deterministically() {
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
        assert_eq!(catalog.sessions[0].session.id.as_str(), "codex-current");
        assert!(catalog.sessions[0].current_workspace);
        assert_eq!(catalog.sessions[1].session.id.as_str(), "claude-other");
        assert_eq!(catalog.sources.len(), 2);
    }

    #[test]
    fn unavailable_source_does_not_hide_the_other_source() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let workspace = home.join("work");
        fs::create_dir_all(&workspace).unwrap();
        claude(&home, "project", "only-claude", &workspace);

        let catalog = scan(&home, &workspace);
        assert_eq!(catalog.sessions.len(), 1);
        assert!(matches!(catalog.sources[0].state, SourceState::Disabled(_)));
        assert_eq!(
            catalog.sources[1].state,
            SourceState::Available { sessions: 1 }
        );
    }

    #[test]
    fn malformed_files_are_diagnostic_and_valid_files_remain() {
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
        assert!(
            catalog
                .diagnostics
                .iter()
                .any(|item| item.kind == DiagnosticKind::Invalid)
        );
    }

    #[test]
    fn malformed_and_unsafe_duplicates_disable_a_valid_id() {
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
        assert!(
            catalog
                .diagnostics
                .iter()
                .any(|item| item.kind == DiagnosticKind::Duplicate)
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
}
