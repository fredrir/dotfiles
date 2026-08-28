
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let sandbox = Sandbox::bare();
        let work = sandbox.work();
        fs::create_dir(&work).unwrap();
        sandbox.git(&work, &["init", "--quiet", "."]);
        sandbox.write("mod.txt", "one\ntwo\n");
        sandbox.write("del.txt", "gone\n");
        sandbox.write("keep.txt", "keep\n");
        sandbox.write(".gitignore", "ignored.log\nignored_dir/\n");
        sandbox.git(&work, &["add", "-A"]);
        sandbox.git(&work, &["commit", "--quiet", "-m", "init"]);
        sandbox
    }

    fn bare() -> Sandbox {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("gitconfig"), "").unwrap();
        Sandbox { home }
    }

    fn work(&self) -> PathBuf {
        self.home.path().join("work")
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.work().join(relative)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path(relative)).unwrap()
    }

    fn exists(&self, relative: &str) -> bool {
        self.path(relative).symlink_metadata().is_ok()
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env("HOME", self.home.path())
            .env("GIT_CONFIG_GLOBAL", self.home.path().join("gitconfig"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env("NO_COLOR", "1")
            .env("COLUMNS", "100");
        command
    }

    fn git(&self, cwd: &Path, arguments: &[&str]) -> String {
        let output = self
            .command("git", cwd)
            .args(["-c", "init.defaultBranch=main"])
            .args(arguments)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {arguments:?} failed");
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    }

    fn status(&self) -> String {
        self.git(&self.work(), &["status", "--porcelain"])
    }

    fn gdd(&self, arguments: &[&str]) -> Output {
        self.gdd_in(&self.work(), arguments)
    }

    fn gdd_in(&self, cwd: &Path, arguments: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_git-discard"), cwd)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .expect("gdd runs")
    }

    fn answer(&self, arguments: &[&str], answer: &str) -> Output {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_git-discard"), &self.work())
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("gdd runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(answer.as_bytes())
            .unwrap();
        child.wait_with_output().expect("gdd finishes")
    }
}

fn out(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

#[test]
fn tracked_changes_go_back_to_head() {
    let sandbox = Sandbox::new();
    sandbox.write("mod.txt", "one\ntwo\nthree\n");
    fs::remove_file(sandbox.path("del.txt")).unwrap();
    sandbox.write("keep.txt", "staged\n");
    sandbox.git(&sandbox.work(), &["add", "keep.txt"]);

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    assert_eq!(sandbox.read("mod.txt"), "one\ntwo\n");
    assert_eq!(sandbox.read("del.txt"), "gone\n");
    assert_eq!(sandbox.read("keep.txt"), "keep\n");
    assert_eq!(sandbox.status(), "");
}

#[test]
fn untracked_files_and_directories_are_deleted() {
    let sandbox = Sandbox::new();
    sandbox.write("untracked.txt", "loose\n");
    sandbox.write("untracked_dir/sub/f.txt", "x\n");
    sandbox.write("added.txt", "added\n");
    sandbox.git(&sandbox.work(), &["add", "added.txt"]);

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    // A directory of nothing but untracked files is one row and one deletion,
    // not a row per file; nothing inside it is named.
    let text = out(&output);
    assert!(text.contains("untracked_dir/"), "{text}");
    assert!(!text.contains("f.txt"), "{text}");
    assert!(!sandbox.exists("untracked.txt"));
    assert!(!sandbox.exists("untracked_dir"));
    assert!(!sandbox.exists("added.txt"));
    assert_eq!(sandbox.status(), "");
}

#[test]
fn ignored_files_are_neither_listed_nor_touched() {
    let sandbox = Sandbox::new();
    sandbox.write("ignored.log", "log\n");
    sandbox.write("untracked.txt", "loose\n");

    let output = sandbox.gdd(&["-y"]);
    assert!(!out(&output).contains("ignored.log"));
    assert_eq!(sandbox.read("ignored.log"), "log\n");
    assert!(!sandbox.exists("untracked.txt"));
}

#[test]
fn ignored_files_inside_an_untracked_directory_are_kept_too() {
    let sandbox = Sandbox::new();
    sandbox.write("loose/gone.txt", "loose\n");
    sandbox.write("loose/ignored.log", "log\n");
    sandbox.write("loose/ignored_dir/deep.txt", "deep\n");
    sandbox.write("loose/deeper/gone.txt", "loose\n");
    sandbox.write("loose/deeper/ignored.log", "log\n");

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    let text = out(&output);
    assert!(!text.contains("ignored.log"), "{text}");
    assert!(!text.contains("ignored_dir"), "{text}");
    // Named one by one rather than collapsed into `loose/`.
    assert!(text.contains("loose/gone.txt"), "{text}");
    assert!(text.contains("loose/deeper/gone.txt"), "{text}");

    assert!(!sandbox.exists("loose/gone.txt"));
    assert!(!sandbox.exists("loose/deeper/gone.txt"));
    assert_eq!(sandbox.read("loose/ignored.log"), "log\n");
    assert_eq!(sandbox.read("loose/deeper/ignored.log"), "log\n");
    assert_eq!(sandbox.read("loose/ignored_dir/deep.txt"), "deep\n");
}

#[test]
fn a_sibling_with_nothing_ignored_still_collapses() {
    let sandbox = Sandbox::new();
    sandbox.write("top/clean/f.txt", "x\n");
    sandbox.write("top/dirty/gone.txt", "x\n");
    sandbox.write("top/dirty/ignored.log", "log\n");

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    let text = out(&output);
    assert!(text.contains("top/clean/"), "{text}");
    assert!(!text.contains("f.txt"), "{text}");
    assert!(text.contains("top/dirty/gone.txt"), "{text}");

    assert!(!sandbox.exists("top/clean"));
    assert!(!sandbox.exists("top/dirty/gone.txt"));
    assert_eq!(sandbox.read("top/dirty/ignored.log"), "log\n");
}

#[test]
fn a_nested_repository_is_kept() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("nested")).unwrap();
    sandbox.git(&sandbox.path("nested"), &["init", "--quiet", "."]);
    sandbox.write("nested/f.txt", "inside\n");
    sandbox.write("untracked.txt", "loose\n");

    let output = sandbox.gdd(&["-y"]);
    assert!(out(&output).contains("nested repository"));
    assert_eq!(sandbox.read("nested/f.txt"), "inside\n");
    assert!(!sandbox.exists("untracked.txt"));
}

#[test]
fn a_repository_inside_an_untracked_directory_survives_with_its_parents() {
    let sandbox = Sandbox::new();
    sandbox.write("loose/keep-me/f.txt", "inside\n");
    sandbox.git(&sandbox.path("loose/keep-me"), &["init", "--quiet", "."]);
    sandbox.write("loose/other.txt", "x\n");

    assert!(sandbox.gdd(&["-y"]).status.success());
    assert_eq!(sandbox.read("loose/keep-me/f.txt"), "inside\n");
    assert!(!sandbox.exists("loose/other.txt"));
}

#[test]
fn a_file_that_took_a_tracked_directorys_name_gives_it_back() {
    let sandbox = Sandbox::new();
    sandbox.write("docs/guide.md", "doc\n");
    sandbox.git(&sandbox.work(), &["add", "-A"]);
    sandbox.git(&sandbox.work(), &["commit", "--quiet", "-m", "docs"]);
    fs::remove_dir_all(sandbox.path("docs")).unwrap();
    sandbox.write("docs", "a file now\n");

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    assert_eq!(sandbox.read("docs/guide.md"), "doc\n");
    assert_eq!(sandbox.status(), "");
}

#[test]
fn a_directory_that_took_a_tracked_files_name_gives_it_back() {
    let sandbox = Sandbox::new();
    fs::remove_file(sandbox.path("mod.txt")).unwrap();
    sandbox.write("mod.txt/inside.txt", "loose\n");

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    assert_eq!(sandbox.read("mod.txt"), "one\ntwo\n");
    assert_eq!(sandbox.status(), "");
}

#[test]
fn answering_no_discards_nothing() {
    let sandbox = Sandbox::new();
    sandbox.write("mod.txt", "changed\n");
    sandbox.write("untracked.txt", "loose\n");

    let output = sandbox.answer(&[], "n\n");
    assert!(output.status.success());
    assert!(out(&output).contains("cancelled"));
    assert_eq!(sandbox.read("mod.txt"), "changed\n");
    assert!(sandbox.exists("untracked.txt"));
}

#[test]
fn an_empty_answer_is_yes() {
    let sandbox = Sandbox::new();
    sandbox.write("mod.txt", "changed\n");

    let output = sandbox.answer(&[], "\n");
    assert!(output.status.success());
    assert_eq!(sandbox.read("mod.txt"), "one\ntwo\n");
}

#[test]
fn no_answer_at_all_discards_nothing_and_fails() {
    let sandbox = Sandbox::new();
    sandbox.write("mod.txt", "changed\n");

    let output = sandbox.gdd(&[]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(sandbox.read("mod.txt"), "changed\n");
}

#[test]
fn a_dry_run_discards_nothing() {
    let sandbox = Sandbox::new();
    sandbox.write("mod.txt", "changed\n");
    sandbox.write("untracked.txt", "loose\n");

    let output = sandbox.gdd(&["--dry-run"]);
    assert!(output.status.success());
    let text = out(&output);
    assert!(text.contains("restore to HEAD"), "{text}");
    assert!(text.contains("delete permanently"), "{text}");
    assert_eq!(sandbox.read("mod.txt"), "changed\n");
    assert!(sandbox.exists("untracked.txt"));
}

#[test]
fn paths_limit_what_is_discarded() {
    let sandbox = Sandbox::new();
    sandbox.write("docs/guide.md", "doc\n");
    sandbox.git(&sandbox.work(), &["add", "-A"]);
    sandbox.git(&sandbox.work(), &["commit", "--quiet", "-m", "docs"]);
    sandbox.write("docs/guide.md", "edited\n");
    sandbox.write("mod.txt", "changed\n");

    assert!(sandbox.gdd(&["-y", "docs"]).status.success());
    assert_eq!(sandbox.read("docs/guide.md"), "doc\n");
    assert_eq!(sandbox.read("mod.txt"), "changed\n");
}

#[test]
fn paths_are_read_from_the_directory_they_are_typed_in() {
    let sandbox = Sandbox::new();
    sandbox.write("docs/guide.md", "doc\n");
    sandbox.write("src/main.rs", "code\n");
    sandbox.git(&sandbox.work(), &["add", "-A"]);
    sandbox.git(&sandbox.work(), &["commit", "--quiet", "-m", "more"]);
    sandbox.write("docs/guide.md", "edited\n");
    sandbox.write("src/main.rs", "edited\n");

    let output = sandbox.gdd_in(&sandbox.path("docs"), &["-y", "."]);
    assert!(output.status.success(), "{}", out(&output));
    assert_eq!(sandbox.read("docs/guide.md"), "doc\n");
    assert_eq!(sandbox.read("src/main.rs"), "edited\n");
}

#[test]
fn the_line_counts_are_the_diff_against_head() {
    let sandbox = Sandbox::new();
    sandbox.write("mod.txt", "ONE\ntwo\nthree\n");
    sandbox.write("untracked.txt", "a\nb\n");

    let text = out(&sandbox.gdd(&["--dry-run"]));
    assert!(text.contains("+2 -1"), "{text}");
    assert!(text.contains("1 restored, 1 deleted   +4  -1"), "{text}");
}

#[test]
fn binary_files_are_named_rather_than_counted() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.path("blob.bin"), [0u8, 1, 2, 0, 3]).unwrap();

    let text = out(&sandbox.gdd(&["--dry-run"]));
    assert!(text.contains("binary"), "{text}");
}

#[test]
fn a_section_stops_at_twelve_rows_unless_asked_for_all() {
    let sandbox = Sandbox::new();
    for number in 1..=15 {
        sandbox.write(&format!("loose-{number:02}.txt"), "x\n");
    }

    let text = out(&sandbox.gdd(&["--dry-run"]));
    assert!(text.contains("… and 3 more"), "{text}");
    assert!(!text.contains("loose-15.txt"), "{text}");

    let text = out(&sandbox.gdd(&["--dry-run", "--all"]));
    assert!(text.contains("loose-15.txt"), "{text}");
    assert!(!text.contains("and 3 more"), "{text}");
}

#[test]
fn staged_files_are_discarded_before_the_first_commit() {
    let sandbox = Sandbox::bare();
    let work = sandbox.work();
    fs::create_dir(&work).unwrap();
    sandbox.git(&work, &["init", "--quiet", "."]);
    sandbox.write("staged.txt", "x\n");
    sandbox.write("deep/dir/staged.txt", "x\n");
    sandbox.git(&work, &["add", "-A"]);
    sandbox.write("loose.txt", "y\n");

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    assert!(!sandbox.exists("staged.txt"));
    assert!(!sandbox.exists("deep"));
    assert!(!sandbox.exists("loose.txt"));
    assert_eq!(sandbox.status(), "");
}

#[test]
fn a_tree_with_nothing_to_discard_says_so() {
    let sandbox = Sandbox::new();
    let output = sandbox.gdd(&[]);
    assert!(output.status.success());
    assert!(out(&output).contains("nothing to discard"));
}

#[test]
fn a_plan_of_nothing_but_a_nested_repository_discards_nothing() {
    let sandbox = Sandbox::new();
    fs::create_dir(sandbox.path("nested")).unwrap();
    sandbox.git(&sandbox.path("nested"), &["init", "--quiet", "."]);

    let output = sandbox.gdd(&[]);
    assert!(output.status.success());
    assert!(out(&output).contains("nothing to discard"));
    assert!(sandbox.exists("nested"));
}

#[test]
fn outside_a_repository_it_says_so() {
    let sandbox = Sandbox::bare();
    let output = sandbox.gdd_in(sandbox.home.path(), &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(out(&output).contains("not a git repository"));
}

#[test]
fn an_unknown_option_is_a_usage_error() {
    let sandbox = Sandbox::new();
    let output = sandbox.gdd(&["--bogus"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(out(&output).contains("--bogus"));
}

#[test]
fn the_executable_bit_and_symlinks_come_back() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    sandbox.write("run.sh", "#!/bin/sh\n");
    sandbox.git(&work, &["update-index", "--add", "--chmod=+x", "run.sh"]);
    sandbox.git(&work, &["commit", "--quiet", "-m", "script"]);
    sandbox.git(&work, &["config", "core.symlinks", "true"]);
    std::os::unix::fs::symlink("mod.txt", sandbox.path("link.txt")).unwrap();
    sandbox.git(&work, &["add", "link.txt"]);
    sandbox.git(&work, &["commit", "--quiet", "-m", "link"]);

    fs::remove_file(sandbox.path("link.txt")).unwrap();
    std::os::unix::fs::symlink("keep.txt", sandbox.path("link.txt")).unwrap();
    sandbox.write("run.sh", "#!/bin/sh\necho\n");
    sandbox.git(&work, &["update-index", "--chmod=-x", "run.sh"]);

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    assert_eq!(sandbox.status(), "");
    assert_eq!(
        fs::read_link(sandbox.path("link.txt")).unwrap(),
        Path::new("mod.txt")
    );
    assert_eq!(
        sandbox.git(&work, &["ls-files", "-s", "run.sh"])[..6],
        *"100755"
    );
}

#[test]
fn the_index_still_describes_the_working_tree_afterwards() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    sandbox.write("mod.txt", "changed\n");
    sandbox.write("keep.txt", "staged\n");
    sandbox.write("added.txt", "added\n");
    fs::remove_file(sandbox.path("del.txt")).unwrap();
    sandbox.git(&work, &["add", "keep.txt", "added.txt"]);

    assert!(sandbox.gdd(&["-y"]).status.success());
    assert_eq!(sandbox.status(), "");
    assert_eq!(
        sandbox.git(&work, &["write-tree"]),
        sandbox.git(&work, &["rev-parse", "HEAD^{tree}"])
    );
    assert_eq!(
        sandbox.git(&work, &["fsck", "--no-progress", "--no-dangling"]),
        ""
    );
}

#[test]
fn a_conflicted_path_goes_back_to_head() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    sandbox.git(&work, &["checkout", "--quiet", "-b", "other"]);
    sandbox.write("mod.txt", "theirs\n");
    sandbox.git(&work, &["commit", "--quiet", "-am", "theirs"]);
    sandbox.git(&work, &["checkout", "--quiet", "main"]);
    sandbox.write("mod.txt", "ours\n");
    sandbox.git(&work, &["commit", "--quiet", "-am", "ours"]);
    let merge = sandbox
        .command("git", &work)
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the merge should conflict");

    let output = sandbox.gdd(&["-y"]);
    assert!(output.status.success(), "{}", out(&output));
    assert!(out(&output).contains("unmerged"));
    assert_eq!(sandbox.read("mod.txt"), "ours\n");
    assert_eq!(sandbox.status(), "");
}

#[test]
fn completions_need_no_repository() {
    let sandbox = Sandbox::bare();
    let output = sandbox.gdd_in(sandbox.home.path(), &["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("#compdef gdd"));
}

#[test]
fn help_describes_this_tool() {
    let sandbox = Sandbox::new();
    let output = sandbox.gdd(&["--help"]);
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .starts_with("Discard every change in the working tree")
    );
}
