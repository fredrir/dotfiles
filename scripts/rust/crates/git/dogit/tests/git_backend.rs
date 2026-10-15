#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, atomic::AtomicBool};

use dogit::git::{CommandSpec, DiscardGuard, Operation, OperationGuard, Repo};
use tempfile::TempDir;

struct Fixture {
    directory: TempDir,
    repo: Repo,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "--initial-branch=main"]);
        git(root, &["config", "user.name", "Dogit Test"]);
        git(root, &["config", "user.email", "dogit@example.invalid"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        git(root, &["config", "core.autocrlf", "false"]);
        git(root, &["config", "core.hooksPath", ".git/hooks"]);
        let repo = Repo::open(root).unwrap();
        Self { directory, repo }
    }

    fn write(&self, path: impl AsRef<Path>, content: &str) {
        let path = self.directory.path().join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn commit(&self, message: &str) {
        git(self.directory.path(), &["add", "--all"]);
        git(self.directory.path(), &["commit", "--message", message]);
    }

    fn execute(&self, spec: CommandSpec) -> Output {
        let mut command = self.repo.command(&spec);
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true");
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        if let Some(input) = spec.stdin {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(&input).unwrap();
        }
        child.wait_with_output().unwrap()
    }

    fn apply(&self, spec: CommandSpec) {
        let output = self.execute(spec);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_EDITOR", "true")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

#[test]
fn status_preserves_index_and_worktree_independently() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("Initial");
    fixture.write("file", "staged\n");
    fixture.apply(fixture.repo.stage(&paths(&["file"])).unwrap());
    fixture.write("file", "unstaged\n");
    let status = fixture.repo.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.entries.len(), 1);
    assert_eq!(
        (status.entries[0].index, status.entries[0].worktree),
        ('M', 'M')
    );
    assert!(status.entries[0].staged() && status.entries[0].unstaged());
}

#[test]
fn literal_paths_cannot_select_siblings_or_become_options() {
    let fixture = Fixture::new();
    for path in ["*.txt", "other.txt", ":(glob)*", "-A", "line\nbreak"] {
        fixture.write(path, path);
    }
    fixture.apply(
        fixture
            .repo
            .stage(&paths(&["*.txt", ":(glob)*", "-A", "line\nbreak"]))
            .unwrap(),
    );
    let status = fixture.repo.status().unwrap();
    for entry in &status.entries {
        assert_eq!(entry.staged(), entry.path != Path::new("other.txt"));
    }
    assert_eq!(status.entries.len(), 5);
}

#[cfg(unix)]
#[test]
#[cfg(not(target_os = "macos"))]
fn non_utf8_paths_round_trip_through_status_staging_and_file_list() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"strange-\xff\n".to_vec()));
    fixture.write(&path, "content");
    assert_eq!(fixture.repo.status().unwrap().entries[0].path, path);
    fixture.apply(fixture.repo.stage(std::slice::from_ref(&path)).unwrap());
    assert_eq!(fixture.repo.files().unwrap(), vec![path]);
}

#[test]
fn unstage_before_first_commit_preserves_worktree_content() {
    let fixture = Fixture::new();
    fixture.write("new", "first\n");
    fixture.apply(fixture.repo.stage(&paths(&["new"])).unwrap());
    fixture.write("new", "second\n");
    fixture.apply(fixture.repo.unstage(&paths(&["new"])).unwrap());
    let status = fixture.repo.status().unwrap();
    assert!(status.oid.is_none());
    assert!(status.entries[0].untracked);
    assert_eq!(
        fs::read_to_string(fixture.repo.root.join("new")).unwrap(),
        "second\n"
    );
}

#[test]
fn rename_retains_source_and_unstages_both_sides() {
    let fixture = Fixture::new();
    fixture.write("old", "original\n");
    fixture.commit("Initial");
    git(&fixture.repo.root, &["mv", "old", "new"]);
    let status = fixture.repo.status().unwrap();
    assert_eq!(status.entries[0].path, Path::new("new"));
    assert_eq!(
        status.entries[0].original_path.as_deref(),
        Some(Path::new("old"))
    );
    fixture.apply(fixture.repo.unstage(&paths(&["new"])).unwrap());
    assert!(
        fixture
            .repo
            .status()
            .unwrap()
            .entries
            .iter()
            .all(|entry| !entry.staged())
    );
}

#[test]
fn history_includes_parents_subject_and_author_and_branches() {
    let fixture = Fixture::new();
    fixture.write("file", "first");
    fixture.commit("First subject");
    fixture.write("file", "second");
    fixture.commit("Second subject");
    let commits = fixture.repo.commits(100).unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].subject, "Second subject");
    assert_eq!(commits[0].parents, vec![commits[1].oid.clone()]);
    assert_eq!(commits[0].author, "Dogit Test");
    let branch = &fixture.repo.branches().unwrap()[0];
    assert_eq!(branch.name, "main");
    assert!(branch.current);
    assert!(!branch.remote);
    assert_eq!(
        fixture
            .repo
            .compare("HEAD~1", "HEAD", None)
            .unwrap()
            .windows(7)
            .filter(|window| *window == b"+second")
            .count(),
        1
    );
}

#[test]
fn snapshot_works_for_unborn_and_detached_heads() {
    let fixture = Fixture::new();
    assert!(fixture.repo.snapshot(100).unwrap().commits.is_empty());
    fixture.write("file", "base");
    fixture.commit("Initial");
    git(&fixture.repo.root, &["switch", "--detach", "HEAD"]);
    let snapshot = fixture.repo.snapshot(100).unwrap();
    assert!(snapshot.status.branch.is_none());
    assert!(snapshot.status.oid.is_some());
    assert!(snapshot.worktrees[0].detached);
}

#[test]
fn stash_lifecycle_preserves_staged_content() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.write("file", "staged");
    fixture.apply(fixture.repo.stage(&paths(&["file"])).unwrap());
    fixture.write("extra", "untracked");
    fixture.apply(fixture.repo.stash_save(Some("Checkpoint"), true).unwrap());
    assert!(fixture.repo.status().unwrap().entries.is_empty());
    let stashes = fixture.repo.stashes().unwrap();
    assert_eq!(stashes.len(), 1);
    assert_eq!(stashes[0].reference, "stash@{0}");
    assert!(stashes[0].subject.contains("Checkpoint"));
    let history = fixture.repo.commits(100).unwrap();
    assert_eq!(
        history.len(),
        1,
        "synthetic stash commits belong in the stash view"
    );
    assert_eq!(history[0].subject, "Initial");
    fixture.apply(
        fixture
            .repo
            .stash_apply(&stashes[0].reference, false)
            .unwrap(),
    );
    assert!(
        fixture
            .repo
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.path == Path::new("file") && entry.staged())
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.root.join("extra")).unwrap(),
        "untracked"
    );
    fixture.apply(fixture.repo.stash_drop(&stashes[0].reference).unwrap());
    assert!(fixture.repo.stashes().unwrap().is_empty());
}

#[test]
fn revision_validation_blocks_options_and_resolves_to_commit_ids() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    assert!(
        fixture
            .repo
            .verify_revision("--output=/tmp/not-a-revision")
            .is_err()
    );
    assert!(fixture.repo.verify_revision("HEAD:file").is_err());
    assert!(fixture.repo.verify_revision("").is_err());
    assert_eq!(fixture.repo.verify_revision("HEAD").unwrap().len(), 40);
    assert!(fixture.repo.switch("--detach").is_err());
    assert!(fixture.repo.stash_drop("--all").is_err());
}

#[test]
fn restore_only_discards_unstaged_content() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.write("file", "staged");
    fixture.apply(fixture.repo.stage(&paths(&["file"])).unwrap());
    fixture.write("file", "unstaged");
    for spec in fixture.repo.discard(&paths(&["file"])).unwrap() {
        fixture.apply(spec);
    }
    assert_eq!(
        fs::read_to_string(fixture.repo.root.join("file")).unwrap(),
        "staged"
    );
    let status = fixture.repo.status().unwrap();
    assert!(status.entries[0].staged());
    assert!(!status.entries[0].unstaged());
}

#[test]
fn untracked_discard_is_literal_and_rejects_broad_or_outside_paths() {
    let fixture = Fixture::new();
    fixture.write("*.txt", "remove");
    fixture.write("keep.txt", "keep");
    fixture.write("folder/keep", "keep");
    for spec in fixture.repo.discard(&paths(&["*.txt"])).unwrap() {
        fixture.apply(spec);
    }
    assert!(!fixture.repo.root.join("*.txt").exists());
    assert!(fixture.repo.root.join("keep.txt").exists());
    for path in [".", "..", "../outside", "/tmp/outside", "folder"] {
        assert!(fixture.repo.discard(&paths(&[path])).is_err(), "{path}");
    }
    assert!(fixture.repo.restore(&[]).is_err());
}

#[test]
fn merge_conflict_has_structured_status_and_can_abort() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("Initial");
    fixture.apply(fixture.repo.create_branch("side", None).unwrap());
    fixture.write("file", "side\n");
    fixture.commit("Side");
    fixture.apply(fixture.repo.switch("main").unwrap());
    fixture.write("file", "main\n");
    fixture.commit("Main");
    assert!(
        !fixture
            .execute(fixture.repo.merge("side").unwrap())
            .status
            .success()
    );
    assert_eq!(fixture.repo.operation().unwrap(), Some(Operation::Merge));
    assert!(fixture.repo.status().unwrap().entries[0].unmerged);
    let guard = OperationGuard::capture(&fixture.repo).unwrap();
    let head = fixture.repo.verify_revision("HEAD").unwrap();
    fixture.apply(fixture.repo.abort_operation().unwrap());
    assert!(
        !fixture
            .execute(fixture.repo.merge("side").unwrap())
            .status
            .success()
    );
    assert_eq!(fixture.repo.verify_revision("HEAD").unwrap(), head);
    assert!(
        guard.abort(&fixture.repo).is_err(),
        "a restarted operation needs a new confirmation"
    );
    let guard = OperationGuard::capture(&fixture.repo).unwrap();
    fs::write(
        fixture.repo.git_dir.join("MERGE_MSG"),
        "Changed merge message\n",
    )
    .unwrap();
    assert!(guard.abort(&fixture.repo).is_err());
    fixture.apply(
        OperationGuard::capture(&fixture.repo)
            .unwrap()
            .abort(&fixture.repo)
            .unwrap(),
    );
    assert_eq!(fixture.repo.operation().unwrap(), None);
    assert!(fixture.repo.status().unwrap().entries.is_empty());
}

#[test]
fn merge_retains_branch_identity_in_the_generated_message() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("Initial");
    fixture.apply(fixture.repo.create_branch("side", None).unwrap());
    fixture.write("side-file", "side\n");
    fixture.commit("Side");
    fixture.apply(fixture.repo.switch("main").unwrap());
    fixture.write("main-file", "main\n");
    fixture.commit("Main");
    fixture.apply(fixture.repo.merge("side").unwrap());
    let commits = fixture.repo.commits(100).unwrap();
    assert_eq!(commits[0].subject, "Merge branch 'side'");
    assert_eq!(commits[0].parents.len(), 2);
    assert!(fixture.repo.status().unwrap().entries.is_empty());
}

#[test]
fn worktree_paths_with_newlines_round_trip_and_use_worktree_git_directory() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    let worktree = fixture.directory.path().join("linked\nworktree");
    let output = Command::new("git")
        .current_dir(&fixture.repo.root)
        .args(["worktree", "add", "-b", "linked"])
        .arg(&worktree)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let worktree = fs::canonicalize(worktree).unwrap();
    assert!(
        fixture
            .repo
            .worktrees()
            .unwrap()
            .iter()
            .any(|item| item.path == worktree)
    );
    let linked = Repo::open(&worktree).unwrap();
    assert_ne!(linked.git_dir, fixture.repo.git_dir);
    fs::write(
        linked.git_dir.join("CHERRY_PICK_HEAD"),
        linked.verify_revision("HEAD").unwrap(),
    )
    .unwrap();
    assert_eq!(linked.operation().unwrap(), Some(Operation::CherryPick));
    assert_eq!(fixture.repo.operation().unwrap(), None);
}

#[cfg(unix)]
#[test]
fn commit_runs_real_hooks_and_amend_keeps_the_selected_message() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let hook = fixture.repo.git_dir.join("hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\nprintf hook-ran > hook-marker\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    fixture.write("file", "base");
    fixture.apply(fixture.repo.stage(&paths(&["file"])).unwrap());
    fixture.apply(fixture.repo.commit(Some("First"), false).unwrap());
    assert!(fixture.repo.root.join("hook-marker").exists());
    fixture.apply(fixture.repo.commit(Some("Replacement"), true).unwrap());
    let commits = fixture.repo.commits(100).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "Replacement");
}

#[test]
fn cancelled_read_returns_without_successful_snapshot() {
    let fixture = Fixture::new();
    let repo = fixture
        .repo
        .clone()
        .with_cancel(Arc::new(AtomicBool::new(true)));
    assert!(repo.status().is_err());
}

#[test]
fn detached_commits_remain_visible_after_leaving_all_branches() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    git(&fixture.repo.root, &["switch", "--detach"]);
    fixture.write("file", "detached");
    fixture.commit("Detached change");
    let oid = fixture.repo.verify_revision("HEAD").unwrap();
    assert!(
        fixture
            .repo
            .commits(100)
            .unwrap()
            .iter()
            .any(|commit| commit.oid == oid)
    );
}

#[test]
fn discard_confirmation_rejects_changed_file_and_index() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.write("file", "first edit");
    let guard = DiscardGuard::capture(&fixture.repo, &paths(&["file"])).unwrap();
    fixture.write("file", "other edit");
    assert!(guard.commands(&fixture.repo).is_err());
    assert_eq!(
        fs::read_to_string(fixture.repo.root.join("file")).unwrap(),
        "other edit"
    );
    let guard = DiscardGuard::capture(&fixture.repo, &paths(&["file"])).unwrap();
    fixture.write("other", "new");
    fixture.apply(fixture.repo.stage(&paths(&["other"])).unwrap());
    assert!(guard.commands(&fixture.repo).is_err());
}

#[test]
fn unchanged_discard_confirmation_preserves_index_and_unselected_files() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.write("file", "staged");
    fixture.apply(fixture.repo.stage(&paths(&["file"])).unwrap());
    fixture.write("file", "unstaged");
    fixture.write("untracked", "remove");
    fixture.write("keep", "keep");
    let guard = DiscardGuard::capture(&fixture.repo, &paths(&["file", "untracked"])).unwrap();
    for spec in guard.commands(&fixture.repo).unwrap() {
        fixture.apply(spec);
    }
    assert_eq!(
        fs::read_to_string(fixture.repo.root.join("file")).unwrap(),
        "staged"
    );
    assert!(!fixture.repo.root.join("untracked").exists());
    assert!(fixture.repo.root.join("keep").exists());
    assert!(
        fixture
            .repo
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.path == Path::new("file") && entry.staged())
    );
}

#[cfg(unix)]
#[test]
fn discard_confirmation_checks_symlink_identity_without_following_target() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.write("first", "keep");
    fixture.write("second", "keep");
    symlink("first", fixture.repo.root.join("link")).unwrap();
    let guard = DiscardGuard::capture(&fixture.repo, &paths(&["link"])).unwrap();
    fs::remove_file(fixture.repo.root.join("link")).unwrap();
    symlink("second", fixture.repo.root.join("link")).unwrap();
    assert!(guard.commands(&fixture.repo).is_err());
    assert_eq!(
        fs::read_to_string(fixture.repo.root.join("first")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.root.join("second")).unwrap(),
        "keep"
    );
}

#[cfg(unix)]
#[test]
fn discard_confirmation_detects_changes_behind_a_symbolic_index() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.write("file", "changed");
    let index = fixture.repo.git_dir.join("index");
    let target = fixture.repo.git_dir.join("actual-index");
    fs::rename(&index, &target).unwrap();
    symlink("actual-index", &index).unwrap();
    let guard = DiscardGuard::capture(&fixture.repo, &paths(&["file"])).unwrap();
    // Mutate the target in place, keeping the symlink itself unchanged.
    let mut contents = fs::read(&target).unwrap();
    contents.push(0);
    fs::write(&target, contents).unwrap();
    assert!(guard.commands(&fixture.repo).is_err());
}

#[test]
fn stash_identity_detects_reflog_number_changes() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.write("file", "first");
    fixture.apply(fixture.repo.stash_save(Some("First stash"), false).unwrap());
    let first = fixture.repo.stashes().unwrap().remove(0);
    fixture
        .repo
        .verify_stash(&first.reference, &first.oid)
        .unwrap();
    fixture.write("file", "second");
    fixture.apply(
        fixture
            .repo
            .stash_save(Some("Second stash"), false)
            .unwrap(),
    );
    assert!(
        fixture
            .repo
            .verify_stash(&first.reference, &first.oid)
            .is_err()
    );
    fixture.repo.verify_stash("stash@{1}", &first.oid).unwrap();
}

#[test]
fn cherry_pick_and_revert_use_real_commits_and_leave_worktree_clean() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.apply(fixture.repo.create_branch("side", None).unwrap());
    fixture.write("added", "side change");
    fixture.commit("Side change");
    let change = fixture.repo.verify_revision("HEAD").unwrap();
    fixture.apply(fixture.repo.switch("main").unwrap());
    fixture.apply(fixture.repo.cherry_pick(&change).unwrap());
    assert!(fixture.repo.root.join("added").exists());
    let picked = fixture.repo.verify_revision("HEAD").unwrap();
    fixture.apply(fixture.repo.revert(&picked).unwrap());
    assert!(!fixture.repo.root.join("added").exists());
    assert!(fixture.repo.status().unwrap().entries.is_empty());
    assert_eq!(fixture.repo.operation().unwrap(), None);
}

#[test]
fn merge_cherry_pick_and_revert_require_an_explicit_valid_mainline() {
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.commit("Initial");
    fixture.apply(fixture.repo.create_branch("side", None).unwrap());
    fixture.write("side-file", "side");
    fixture.commit("Side");
    fixture.apply(fixture.repo.switch("main").unwrap());
    fixture.write("main-file", "main");
    fixture.commit("Main");
    let main = fixture.repo.verify_revision("HEAD").unwrap();
    fixture.apply(fixture.repo.merge("side").unwrap());
    let merge = fixture.repo.verify_revision("HEAD").unwrap();
    assert!(fixture.repo.revert_mainline(&merge, 0).is_err());
    assert!(fixture.repo.revert_mainline(&merge, 3).is_err());
    assert!(fixture.repo.cherry_pick_mainline(&main, 1).is_err());
    fixture.apply(fixture.repo.revert_mainline(&merge, 1).unwrap());
    assert!(!fixture.repo.root.join("side-file").exists());
    assert!(fixture.repo.root.join("main-file").exists());
    fixture.apply(fixture.repo.create_branch("replay", Some(&main)).unwrap());
    fixture.apply(fixture.repo.cherry_pick_mainline(&merge, 1).unwrap());
    assert!(fixture.repo.root.join("side-file").exists());
    assert!(fixture.repo.root.join("main-file").exists());
    assert!(fixture.repo.status().unwrap().entries.is_empty());
}

#[test]
fn rebase_conflict_can_continue_after_explicit_resolution() {
    let fixture = Fixture::new();
    fixture.write("file", "base\n");
    fixture.commit("Initial");
    fixture.apply(fixture.repo.create_branch("side", None).unwrap());
    fixture.write("file", "side\n");
    fixture.commit("Side");
    fixture.apply(fixture.repo.switch("main").unwrap());
    fixture.write("file", "main\n");
    fixture.commit("Main");
    let main = fixture.repo.verify_revision("HEAD").unwrap();
    fixture.apply(fixture.repo.switch("side").unwrap());
    assert!(
        !fixture
            .execute(fixture.repo.rebase("main").unwrap())
            .status
            .success()
    );
    assert_eq!(fixture.repo.operation().unwrap(), Some(Operation::Rebase));
    assert!(
        fixture
            .repo
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|entry| entry.unmerged)
    );
    fixture.write("file", "main and side\n");
    fixture.apply(fixture.repo.stage(&paths(&["file"])).unwrap());
    fixture.apply(fixture.repo.continue_operation().unwrap());
    assert_eq!(fixture.repo.operation().unwrap(), None);
    assert_eq!(fixture.repo.verify_revision("HEAD^").unwrap(), main);
    assert!(fixture.repo.status().unwrap().entries.is_empty());
}

#[test]
fn fetch_pull_push_and_remote_tracking_preserve_git_configuration() {
    let fixture = Fixture::new();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "--initial-branch=main"]);
    git(
        &fixture.repo.root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    fixture.write("file", "base");
    fixture.commit("Initial");
    git(
        &fixture.repo.root,
        &["push", "--set-upstream", "origin", "main"],
    );
    let peer = tempfile::tempdir().unwrap();
    git(
        peer.path(),
        &["clone", "--", remote.path().to_str().unwrap(), "."],
    );
    git(peer.path(), &["config", "user.name", "Peer"]);
    git(
        peer.path(),
        &["config", "user.email", "peer@example.invalid"],
    );
    fs::write(peer.path().join("remote-change"), "peer").unwrap();
    git(peer.path(), &["add", "--all"]);
    git(peer.path(), &["commit", "--message", "Peer change"]);
    git(peer.path(), &["push"]);
    fixture.apply(fixture.repo.fetch());
    assert_eq!(fixture.repo.status().unwrap().behind, 1);
    git(
        &fixture.repo.root,
        &["remote", "set-head", "origin", "main"],
    );
    assert!(
        fixture
            .repo
            .branches()
            .unwrap()
            .iter()
            .all(|branch| branch.name != "origin/HEAD")
    );
    fixture.apply(fixture.repo.pull());
    assert!(fixture.repo.root.join("remote-change").exists());
    fixture.write("local-change", "local");
    fixture.commit("Local change");
    fixture.apply(fixture.repo.push());
    assert_eq!(
        git(remote.path(), &["rev-parse", "main"]),
        git(&fixture.repo.root, &["rev-parse", "HEAD"])
    );
    git(peer.path(), &["switch", "--create", "topic"]);
    git(peer.path(), &["push", "--set-upstream", "origin", "topic"]);
    fixture.apply(fixture.repo.fetch());
    git(&fixture.repo.root, &["branch", "origin/topic", "main"]);
    fixture.apply(fixture.repo.switch_remote("origin/topic").unwrap());
    let status = fixture.repo.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("topic"));
    assert_eq!(status.upstream.as_deref(), Some("remotes/origin/topic"));
    assert_eq!(
        git(&fixture.repo.root, &["config", "branch.topic.remote"]),
        b"origin\n"
    );
    assert_eq!(
        git(&fixture.repo.root, &["config", "branch.topic.merge"]),
        b"refs/heads/topic\n"
    );
}

#[cfg(unix)]
#[test]
fn rejected_hook_and_signing_failure_never_create_a_commit() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.write("file", "base");
    fixture.apply(fixture.repo.stage(&paths(&["file"])).unwrap());
    let hook = fixture.repo.git_dir.join("hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        !fixture
            .execute(fixture.repo.commit(Some("Rejected"), false).unwrap())
            .status
            .success()
    );
    assert!(fixture.repo.status().unwrap().oid.is_none());
    fs::remove_file(hook).unwrap();
    let signer = fixture.repo.git_dir.join("test-signer");
    fs::write(
        &signer,
        "#!/bin/sh\nprintf invoked > signing-marker\nexit 42\n",
    )
    .unwrap();
    fs::set_permissions(&signer, fs::Permissions::from_mode(0o755)).unwrap();
    let key = fixture.repo.git_dir.join("signing-key");
    fs::write(&key, "test key").unwrap();
    git(&fixture.repo.root, &["config", "commit.gpgsign", "true"]);
    git(&fixture.repo.root, &["config", "gpg.format", "ssh"]);
    git(
        &fixture.repo.root,
        &["config", "gpg.ssh.program", signer.to_str().unwrap()],
    );
    git(
        &fixture.repo.root,
        &["config", "user.signingkey", key.to_str().unwrap()],
    );
    let spec = fixture.repo.commit(Some("Must sign"), false).unwrap();
    assert!(spec.interactive);
    assert!(!fixture.execute(spec).status.success());
    assert!(fixture.repo.root.join("signing-marker").exists());
    assert!(fixture.repo.status().unwrap().oid.is_none());
    assert!(fixture.repo.status().unwrap().entries[0].staged());
}
