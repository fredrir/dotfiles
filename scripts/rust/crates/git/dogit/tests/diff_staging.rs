#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use dogit::diff::{DiffSide, ExactDiff, LineKind, Selection};

struct Repo(tempfile::TempDir);

impl Repo {
    fn new() -> Self {
        let repo = Self(tempfile::tempdir().unwrap());
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.name", "Dogit test"]);
        repo.git(&["config", "user.email", "dogit@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo.git(&["config", "core.hooksPath", "/dev/null"]);
        repo
    }

    fn root(&self) -> &Path {
        self.0.path()
    }
    fn path(&self, path: &str) -> PathBuf {
        self.root().join(path)
    }
    fn write(&self, path: &str, content: &[u8]) {
        if let Some(parent) = self.path(path).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(self.path(path), content).unwrap();
    }
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = Command::new("git")
            .current_dir(self.root())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    fn commit(&self, path: &str, content: &[u8]) {
        self.write(path, content);
        self.git(&["add", "--", path]);
        self.git(&["commit", "-qm", "fixture"]);
    }
    fn diff(&self, path: &str, side: DiffSide) -> ExactDiff {
        ExactDiff::load(self.root(), Path::new(path), side).unwrap()
    }
    fn index(&self, path: &str) -> Vec<u8> {
        self.git(&["show", &format!(":{path}")])
    }
}

fn choose(diff: &ExactDiff, kind: LineKind, contents: &[&[u8]]) -> Selection {
    Selection::Lines(
        diff.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind == kind && contents.contains(&line.content.as_slice()))
            .map(|line| line.id)
            .collect(),
    )
}

#[test]
fn directory_preview_keeps_index_worktree_and_untracked_scopes_separate() {
    let repo = Repo::new();
    repo.commit("src/main.rs", b"fn original() {}\n");
    repo.write("src/main.rs", b"fn staged() {}\n");
    repo.git(&["add", "src/main.rs"]);
    repo.write("src/main.rs", b"fn working() {}\n");
    repo.write("src/new.rs", b"fn new_file() {}\n");
    repo.write("outside.rs", b"fn outside() {}\n");
    let index = repo.git(&["write-tree"]);
    let scope =
        |side| ExactDiff::directory_cancellable(repo.root(), Path::new("src"), side, None).unwrap();
    let staged = scope(DiffSide::Index);
    let unstaged = scope(DiffSide::Worktree);
    let staged_text = String::from_utf8_lossy(&staged.raw);
    let unstaged_text = String::from_utf8_lossy(&unstaged.raw);
    assert!(staged_text.contains("+fn staged()"));
    assert!(!staged_text.contains("working") && !staged_text.contains("new_file"));
    assert!(unstaged_text.contains("+fn working()"));
    assert!(unstaged_text.contains("+fn new_file()"));
    assert!(!unstaged_text.contains("outside"));
    assert!(staged.apply(&Selection::Hunk(0)).is_err());
    assert!(unstaged.apply(&Selection::Hunk(0)).is_err());
    assert_eq!(repo.git(&["write-tree"]), index);
    assert_eq!(
        fs::read(repo.path("src/main.rs")).unwrap(),
        b"fn working() {}\n"
    );
}

#[test]
fn stage_selected_addition_without_its_neighbor() {
    let repo = Repo::new();
    repo.commit("file", b"first\nlast\n");
    repo.write("file", b"first\nchosen\nremaining\nlast\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"chosen\n"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"first\nchosen\nlast\n");
    assert_eq!(
        fs::read(repo.path("file")).unwrap(),
        b"first\nchosen\nremaining\nlast\n"
    );
}

#[test]
fn unstage_selected_addition_preserves_neighbor_and_worktree() {
    let repo = Repo::new();
    repo.commit("file", b"first\nlast\n");
    let content = b"first\none\ntwo\nlast\n";
    repo.write("file", content);
    repo.git(&["add", "file"]);
    let diff = repo.diff("file", DiffSide::Index);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"one\n"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"first\ntwo\nlast\n");
    assert_eq!(fs::read(repo.path("file")).unwrap(), content);
}

#[test]
fn mixed_selection_can_stage_a_replacement_without_other_changes() {
    let repo = Repo::new();
    repo.commit("file", b"a\nold\nkeep\nend\n");
    repo.write("file", b"a\nnew\nkeep\nextra\nend\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    let ids = diff
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.content == b"old\n" || line.content == b"new\n")
        .map(|line| line.id)
        .collect();
    diff.apply(&Selection::Lines(ids)).unwrap();
    assert_eq!(repo.index("file"), b"a\nnew\nkeep\nend\n");
}

#[test]
fn partial_staging_starts_from_existing_index_content() {
    let repo = Repo::new();
    repo.commit("file", b"a\nz\n");
    repo.write("file", b"a\nstaged\nz\n");
    repo.git(&["add", "file"]);
    repo.write("file", b"a\nstaged\nchosen\nremaining\nz\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"chosen\n"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"a\nstaged\nchosen\nz\n");
}

#[test]
fn selected_hunk_leaves_other_hunks_unstaged() {
    let repo = Repo::new();
    let original = (0..30).map(|n| format!("line {n}\n")).collect::<String>();
    repo.commit("file", original.as_bytes());
    let changed = original
        .replace("line 2\n", "first change\n")
        .replace("line 25\n", "second change\n");
    repo.write("file", changed.as_bytes());
    let diff = repo.diff("file", DiffSide::Worktree);
    assert_eq!(diff.hunks.len(), 2);
    diff.apply(&Selection::Hunk(1)).unwrap();
    assert_eq!(
        repo.index("file"),
        original.replace("line 25\n", "second change\n").as_bytes()
    );
}

#[test]
fn stage_and_unstage_new_file_in_parts() {
    let repo = Repo::new();
    repo.write("new", b"one\ntwo\nthree\n");
    let diff = repo.diff("new", DiffSide::Worktree);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"one\n", b"three\n"]))
        .unwrap();
    assert_eq!(repo.index("new"), b"one\nthree\n");
    let diff = repo.diff("new", DiffSide::Index);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"one\n"]))
        .unwrap();
    assert_eq!(repo.index("new"), b"three\n");
    repo.diff("new", DiffSide::Index)
        .apply(&Selection::Hunk(0))
        .unwrap();
    assert!(repo.git(&["ls-files"]).is_empty());
    assert_eq!(fs::read(repo.path("new")).unwrap(), b"one\ntwo\nthree\n");
}

#[test]
fn stage_and_unstage_deleted_file_in_parts() {
    let repo = Repo::new();
    repo.commit("file", b"one\ntwo\nthree\n");
    fs::remove_file(repo.path("file")).unwrap();
    let diff = repo.diff("file", DiffSide::Worktree);
    diff.apply(&choose(&diff, LineKind::Deletion, &[b"two\n"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"one\nthree\n");
    repo.diff("file", DiffSide::Worktree)
        .apply(&Selection::Hunk(0))
        .unwrap();
    assert!(repo.git(&["ls-files"]).is_empty());
    let diff = repo.diff("file", DiffSide::Index);
    diff.apply(&choose(&diff, LineKind::Deletion, &[b"two\n"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"two\n");
    assert!(!repo.path("file").exists());
}

#[test]
fn preserves_crlf_and_missing_final_newline() {
    let repo = Repo::new();
    repo.commit("file", b"first\r\nold\r\nlast");
    repo.write("file", b"first\r\nnew\r\nlast");
    let diff = repo.diff("file", DiffSide::Worktree);
    assert!(diff.hunks[0].lines.last().unwrap().no_newline);
    diff.apply(&Selection::Hunk(0)).unwrap();
    assert_eq!(repo.index("file"), b"first\r\nnew\r\nlast");
    repo.diff("file", DiffSide::Index)
        .apply(&Selection::Hunk(0))
        .unwrap();
    assert_eq!(repo.index("file"), b"first\r\nold\r\nlast");
}

#[test]
fn supports_literal_pathspecs_and_patch_quoted_filenames() {
    let repo = Repo::new();
    let path = ":(glob)* [x]\t\"å";
    repo.write(path, b"old\n");
    repo.git(&["--literal-pathspecs", "add", "--", path]);
    repo.git(&["commit", "-qm", "fixture"]);
    repo.write(path, b"new\n");
    repo.diff(path, DiffSide::Worktree)
        .apply(&Selection::Hunk(0))
        .unwrap();
    assert_eq!(repo.index(path), b"new\n");
}

#[test]
fn external_file_change_rejects_the_old_selection_without_touching_index() {
    let repo = Repo::new();
    repo.commit("file", b"old\n");
    repo.write("file", b"selected\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    repo.write("file", b"external edit\n");
    assert!(
        diff.apply(&Selection::Hunk(0))
            .unwrap_err()
            .contains("file changed")
    );
    assert_eq!(repo.index("file"), b"old\n");
    assert!(!repo.path(".git/index.lock").exists());
}

#[test]
fn external_index_change_rejects_old_selection() {
    let repo = Repo::new();
    repo.commit("file", b"old\n");
    repo.write("file", b"selected\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    repo.write("other", b"external\n");
    repo.git(&["add", "other"]);
    assert!(
        diff.apply(&Selection::Hunk(0))
            .unwrap_err()
            .contains("index changed")
    );
    assert_eq!(repo.index("file"), b"old\n");
    assert_eq!(repo.index("other"), b"external\n");
}

#[test]
fn existing_index_lock_is_respected_and_left_in_place() {
    let repo = Repo::new();
    repo.commit("file", b"old\n");
    repo.write("file", b"new\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    repo.write(".git/index.lock", b"another operation");
    assert!(
        diff.apply(&Selection::Hunk(0))
            .unwrap_err()
            .contains("Another Git operation")
    );
    assert_eq!(
        fs::read(repo.path(".git/index.lock")).unwrap(),
        b"another operation"
    );
}

#[test]
fn binary_and_mode_only_changes_have_explicit_whole_file_requirement() {
    let repo = Repo::new();
    repo.commit("binary", b"\0old");
    repo.write("binary", b"\0new");
    let diff = repo.diff("binary", DiffSide::Worktree);
    assert!(diff.partial_reason.unwrap().contains("Binary"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        repo.commit("executable", b"text\n");
        fs::set_permissions(repo.path("executable"), fs::Permissions::from_mode(0o755)).unwrap();
        let diff = repo.diff("executable", DiffSide::Worktree);
        assert!(diff.partial_reason.unwrap().contains("mode"));
    }
}

#[test]
fn history_documents_are_read_only_and_include_all_files() {
    let repo = Repo::new();
    repo.commit("one", b"old\n");
    repo.write("one", b"new\n");
    repo.write("two", b"added\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "second"]);
    let diff = ExactDiff::between(repo.root(), "HEAD~", "HEAD", None).unwrap();
    assert_eq!(diff.hunks.len(), 2);
    assert!(
        diff.apply(&Selection::Hunk(0))
            .unwrap_err()
            .contains("read-only")
    );
    let commit = ExactDiff::commit(repo.root(), "HEAD").unwrap();
    assert_eq!(commit.hunks.len(), 2);
}

#[test]
fn empty_commit_root_has_a_renderable_patch() {
    let repo = Repo::new();
    repo.commit("file", b"content\n");
    assert_eq!(
        ExactDiff::commit(repo.root(), "HEAD").unwrap().hunks.len(),
        1
    );
}

#[test]
fn split_index_staging_publishes_a_standalone_readable_index() {
    let repo = Repo::new();
    repo.commit("file", b"first\nlast\n");
    repo.commit("unchanged", b"preserve me\n");
    repo.git(&["config", "core.splitIndex", "true"]);
    repo.git(&["update-index", "--split-index"]);
    repo.write("file", b"first\nchosen\nremaining\nlast\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"chosen\n"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"first\nchosen\nlast\n");
    assert_eq!(repo.index("unchanged"), b"preserve me\n");
    let staged = repo.diff("file", DiffSide::Index);
    staged.apply(&Selection::Hunk(0)).unwrap();
    assert_eq!(repo.index("file"), b"first\nlast\n");
    assert!(!repo.path(".git/index.lock").exists());
}

#[test]
#[cfg(unix)]
fn partial_staging_preserves_index_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit("file", b"old\n");
    repo.write("file", b"new\n");
    fs::set_permissions(repo.path(".git/index"), fs::Permissions::from_mode(0o640)).unwrap();
    repo.diff("file", DiffSide::Worktree)
        .apply(&Selection::Hunk(0))
        .unwrap();
    assert_eq!(
        fs::metadata(repo.path(".git/index"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
}

#[test]
fn exact_untracked_patch_ignores_external_and_text_conversion_drivers() {
    let repo = Repo::new();
    repo.git(&["config", "diff.external", "false"]);
    repo.git(&["config", "diff.custom.textconv", "false"]);
    repo.write(".gitattributes", b"*.txt diff=custom\n");
    repo.write("new.txt", b"one\ntwo\n");
    let diff = repo.diff("new.txt", DiffSide::Worktree);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"one\n"]))
        .unwrap();
    assert_eq!(repo.index("new.txt"), b"one\n");
}

#[test]
fn partial_staging_preserves_non_utf8_content() {
    let repo = Repo::new();
    repo.commit("file", b"first\n\xff original\nlast\n");
    repo.write("file", b"first\n\xfe replacement\nlast\n");
    repo.diff("file", DiffSide::Worktree)
        .apply(&Selection::Hunk(0))
        .unwrap();
    assert_eq!(repo.index("file"), b"first\n\xfe replacement\nlast\n");
}

#[test]
#[cfg(unix)]
fn non_utf8_filenames_round_trip_through_partial_patches() {
    use std::os::unix::ffi::OsStrExt;
    let repo = Repo::new();
    let path = Path::new(std::ffi::OsStr::from_bytes(b"name\xff"));
    fs::write(repo.root().join(path), b"one\ntwo\n").unwrap();
    let diff = ExactDiff::load(repo.root(), path, DiffSide::Worktree).unwrap();
    diff.apply(&choose(&diff, LineKind::Addition, &[b"one\n"]))
        .unwrap();
    let mut object = std::ffi::OsString::from(":");
    object.push(path);
    let output = Command::new("git")
        .current_dir(repo.root())
        .args(["cat-file", "blob"])
        .arg(object)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"one\n");
}

#[test]
fn partial_unstage_across_hunks_uses_the_index_line_numbers() {
    let repo = Repo::new();
    let original = (0..35).map(|n| format!("line {n}\n")).collect::<String>();
    repo.commit("file", original.as_bytes());
    let changed = original
        .replace("line 2\n", "line 2\nfirst\nsecond\n")
        .replace("line 30\n", "last change\n");
    repo.write("file", changed.as_bytes());
    repo.git(&["add", "file"]);
    let diff = repo.diff("file", DiffSide::Index);
    let selected = diff
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| {
            line.content == b"first\n"
                || line.content == b"line 30\n"
                || line.content == b"last change\n"
        })
        .map(|line| line.id)
        .collect();
    diff.apply(&Selection::Lines(selected)).unwrap();
    assert_eq!(
        repo.index("file"),
        original.replace("line 2\n", "line 2\nsecond\n").as_bytes()
    );
    assert_eq!(fs::read(repo.path("file")).unwrap(), changed.as_bytes());
}

#[test]
fn selecting_the_only_added_line_without_final_newline_keeps_exact_bytes() {
    let repo = Repo::new();
    repo.commit("file", b"first\n");
    repo.write("file", b"first\nlast");
    let diff = repo.diff("file", DiffSide::Worktree);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"last"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"first\nlast");
    let diff = repo.diff("file", DiffSide::Index);
    diff.apply(&choose(&diff, LineKind::Addition, &[b"last"]))
        .unwrap();
    assert_eq!(repo.index("file"), b"first\n");
}

#[test]
fn commit_file_only_returns_the_selected_file() {
    let repo = Repo::new();
    repo.commit("one.rs", b"fn one() {}\n");
    repo.write("one.rs", b"fn one() { run(); }\n");
    repo.write("two.rs", b"fn two() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "second"]);
    let diff = ExactDiff::commit_file(repo.root(), "HEAD", Path::new("one.rs")).unwrap();
    assert_eq!(diff.hunks.len(), 1);
    assert_eq!(diff.path, Path::new("one.rs"));
    assert!(!String::from_utf8_lossy(&diff.raw).contains("two.rs"));
}

#[test]
fn empty_untracked_file_is_a_valid_document() {
    let repo = Repo::new();
    repo.write("empty", b"");
    let diff = repo.diff("empty", DiffSide::Worktree);
    assert!(diff.hunks.is_empty());
}

#[test]
#[cfg(unix)]
fn dangling_symlink_requires_whole_file_staging() {
    let repo = Repo::new();
    std::os::unix::fs::symlink("missing-target", repo.path("link")).unwrap();
    let diff = repo.diff("link", DiffSide::Worktree);
    assert!(diff.partial_reason.unwrap().contains("Symbolic links"));
}

#[test]
fn pre_cancelled_diff_load_returns_promptly() {
    use std::sync::atomic::AtomicBool;
    let repo = Repo::new();
    repo.commit("file", b"old\n");
    repo.write("file", b"new\n");
    let cancelled = AtomicBool::new(true);
    let result = ExactDiff::load_cancellable(
        repo.root(),
        Path::new("file"),
        DiffSide::Worktree,
        Some(&cancelled),
    );
    assert!(result.unwrap_err().contains("cancelled"));
}

#[test]
fn installed_difftastic_renders_changes_root_commits_and_revision_comparisons() {
    if Command::new("difft").arg("--version").output().is_err() {
        return;
    }
    let repo = Repo::new();
    repo.commit("main.rs", b"fn main() { println!(\"before\"); }\n");
    let root = ExactDiff::commit_file(repo.root(), "HEAD", Path::new("main.rs")).unwrap();
    let output = root.structural(100).unwrap().unwrap();
    assert!(output.contains("main.rs"));
    assert!(output.contains("before"));
    repo.write("main.rs", b"fn main() { println!(\"after\"); }\n");
    let output = repo
        .diff("main.rs", DiffSide::Worktree)
        .structural(100)
        .unwrap()
        .unwrap();
    assert!(output.contains("before"));
    assert!(output.contains("after"));
    assert!(output.contains("\x1b["));
    repo.write("second.rs", b"pub fn second() -> u32 { 42 }\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "second"]);
    let comparison = ExactDiff::between(repo.root(), "HEAD~", "HEAD", None).unwrap();
    let output = comparison.structural(120).unwrap().unwrap();
    assert!(output.contains("main.rs"));
    assert!(output.contains("second.rs"));
    assert!(!output.contains("/tmp/"));
}

#[test]
fn structural_revision_comparison_handles_file_to_directory_changes() {
    if Command::new("difft").arg("--version").output().is_err() {
        return;
    }
    let repo = Repo::new();
    repo.commit("node", b"before\n");
    fs::remove_file(repo.path("node")).unwrap();
    repo.write("node/child.rs", b"fn child() {}\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "file became directory"]);
    let diff = ExactDiff::between(repo.root(), "HEAD~", "HEAD", None).unwrap();
    let output = diff.structural(100).unwrap().unwrap();
    assert!(output.contains("node"));
    assert!(output.contains("child.rs"));
}

#[test]
fn clean_or_untracked_files_on_the_index_side_have_an_empty_preview() {
    let repo = Repo::new();
    repo.write("new", b"not staged\n");
    let diff = repo.diff("new", DiffSide::Index);
    assert!(diff.raw.is_empty());
    repo.commit("tracked", b"committed\n");
    assert!(repo.diff("new", DiffSide::Index).raw.is_empty());
    assert!(repo.diff("tracked", DiffSide::Index).raw.is_empty());
}

#[test]
fn aggregate_staged_preview_contains_exact_index_content_for_all_files() {
    let repo = Repo::new();
    repo.write("one.rs", b"fn staged_one() {}\n");
    repo.write("two.rs", b"fn staged_two() {}\n");
    repo.git(&["add", "."]);
    repo.write("two.rs", b"fn not_staged() {}\n");
    let diff = ExactDiff::staged(repo.root()).unwrap();
    assert_eq!(diff.hunks.len(), 2);
    let text = String::from_utf8_lossy(&diff.raw);
    assert!(text.contains("staged_one"));
    assert!(text.contains("staged_two"));
    assert!(!text.contains("not_staged"));
    assert!(diff.apply(&Selection::Hunk(0)).is_err());
    assert_eq!(repo.index("two.rs"), b"fn staged_two() {}\n");
}

#[test]
fn selected_line_after_an_unterminated_line_cannot_silently_concatenate_content() {
    let repo = Repo::new();
    repo.commit("file", b"unterminated");
    repo.write("file", b"unterminated\nadded\n");
    let diff = repo.diff("file", DiffSide::Worktree);
    let result = diff.apply(&choose(&diff, LineKind::Addition, &[b"added\n"]));
    assert!(result.unwrap_err().contains("final-line replacement"));
    assert_eq!(repo.index("file"), b"unterminated");
    assert_eq!(
        fs::read(repo.path("file")).unwrap(),
        b"unterminated\nadded\n"
    );
    // The suggested whole-hunk action includes the necessary newline change.
    diff.apply(&Selection::Hunk(0)).unwrap();
    assert_eq!(repo.index("file"), b"unterminated\nadded\n");
}
