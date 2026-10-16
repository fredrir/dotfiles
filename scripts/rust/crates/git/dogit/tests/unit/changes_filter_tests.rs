use super::*;

pub(super) fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2026-10-15T23:30:00-07:00")
        .env("GIT_COMMITTER_DATE", "2026-10-16T08:30:00+02:00")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}

pub(super) fn fixture() -> (tempfile::TempDir, App) {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    git(temp.path(), &["config", "user.name", "Example Author"]);
    git(
        temp.path(),
        &["config", "user.email", "author@example.test"],
    );
    git(temp.path(), &["config", "commit.gpgSign", "false"]);
    std::fs::create_dir_all(temp.path().join("src/deep")).unwrap();
    for path in ["src/deep/one.rs", "src/two.rs", "README"] {
        std::fs::write(temp.path().join(path), "original\n").unwrap();
    }
    git(temp.path(), &["add", "."]);
    git(
        temp.path(),
        &[
            "commit",
            "-qm",
            "A clear subject\n\nA detailed body.\n\nCo-authored-by: Contributor <other@example.test>",
        ],
    );
    let repo = Repo::open(temp.path()).unwrap();
    let mut app = App::new(repo, Start::Changes);
    app.snapshot = app.repo.snapshot(20).unwrap();
    app.files = app.repo.files().unwrap();
    app.rebuild();
    (temp, app)
}

#[test]
fn history_and_changes_share_tree_grouping_and_guides() {
    let (_temp, mut app) = fixture();
    app.all_files = true;
    let (changes, _) = app.changes_entries();
    app.history_commit = Some(app.snapshot.commits[0].oid.clone());
    app.revision_paths = app.files.clone();
    app.view.workspace = Workspace::History;
    let (history, summary) = app.history_entries();
    assert_eq!(summary.items, 3);
    let shape = |entries: &[Entry]| {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.target.clone(),
                    entry.item.label.clone(),
                    entry.item.depth,
                    entry.item.graph.clone(),
                    entry.item.kind,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(shape(&changes), shape(&history));
    assert!(history.iter().all(|entry| entry.item.file_state.is_some()));
    app.history_collapsed.insert(PathBuf::from("src"));
    let (collapsed, _) = app.history_entries();
    assert!(collapsed.len() < history.len());
    assert!(app.collapsed.is_empty());
}

#[test]
fn changes_filters_do_not_hide_history_paths() {
    let (_temp, mut app) = fixture();
    app.changes_filter = Some(DiffSide::Index);
    assert!(app.changes_entries().0.is_empty());
    app.view.workspace = Workspace::History;
    app.history_commit = Some(app.snapshot.commits[0].oid.clone());
    app.revision_paths = app.files.clone();
    assert_eq!(app.history_entries().1.items, 3);
    app.query = "one.rs".into();
    assert_eq!(app.history_entries().1.items, 1);
}

#[test]
fn commit_details_preserve_message_author_time_and_signature() {
    let (_temp, app) = fixture();
    let details = app
        .repo
        .commit_details(&app.snapshot.commits[0].oid)
        .unwrap();
    assert_eq!(details.author, "Example Author");
    assert_eq!(details.authored_iso, "2026-10-15T23:30:00-07:00");
    assert_eq!(details.date(), "October 15, 2026");
    assert_eq!(details.short_oid().len(), 7);
    assert_eq!(details.verification(), ("×", "Unsigned"));
    assert!(
        details
            .message
            .unwrap()
            .contains("A detailed body.\n\nCo-authored-by:")
    );
}

#[test]
fn history_directory_diff_uses_commit_even_when_worktree_changed() {
    let (temp, mut app) = fixture();
    std::fs::write(temp.path().join("src/deep/one.rs"), "worktree only\n").unwrap();
    let oid = app.snapshot.commits[0].oid.clone();
    app.view.workspace = Workspace::History;
    app.history_commit = Some(oid);
    app.revision_paths = app.files.clone();
    app.rebuild();
    app.view.selected = app
        .entries
        .iter()
        .position(|entry| entry.target == Target::Path("src".into(), true))
        .unwrap();
    app.load_content(true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.content_pending && Instant::now() < deadline {
        app.collect();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!app.content_pending);
    let diff = app.diff.as_ref().expect("directory diff loaded");
    let raw = String::from_utf8_lossy(&diff.raw);
    assert!(raw.contains("+original"));
    assert!(!raw.contains("worktree only"));
    assert!(!raw.contains("b/README"));
    assert!(app.loaded_preview.as_ref().unwrap().stamps.is_empty());
}
