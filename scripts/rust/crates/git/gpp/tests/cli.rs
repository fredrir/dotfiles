//! Black-box checks against a real repository, since what `gpp` does is
//! decide what to make of git's own statuses.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A work repository with a bare origin to push to, run with git reading no
/// configuration and no identity from the machine running the tests.
struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let sandbox = Sandbox {
            home: tempfile::tempdir().unwrap(),
        };
        let home = sandbox.home.path();
        fs::write(home.join("gitconfig"), "").unwrap();
        sandbox.git(home, &["init", "--quiet", "--bare", "origin.git"]);
        sandbox.git(home, &["init", "--quiet", "work"]);
        let work = sandbox.work();
        fs::write(work.join("seed"), "seed\n").unwrap();
        sandbox.git(&work, &["add", "."]);
        sandbox.git(&work, &["commit", "--quiet", "-m", "seed"]);
        sandbox.git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                home.join("origin.git").to_str().unwrap(),
            ],
        );
        sandbox.git(&work, &["push", "--quiet", "-u", "origin", "HEAD"]);
        sandbox
    }

    fn work(&self) -> PathBuf {
        self.home.path().join("work")
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
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
        command
    }

    fn git(&self, cwd: &Path, arguments: &[&str]) -> Output {
        let output = self
            .command("git", cwd)
            .args(["-c", "init.defaultBranch=main"])
            .args(arguments)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {arguments:?} failed");
        output
    }

    fn read(&self, cwd: &Path, arguments: &[&str]) -> String {
        String::from_utf8_lossy(&self.git(cwd, arguments).stdout)
            .trim_end()
            .to_string()
    }

    fn gpp(&self, cwd: &Path, arguments: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_gpp"), cwd)
            .args(arguments)
            .output()
            .expect("gpp runs")
    }
}

#[test]
fn stages_commits_and_pushes() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    fs::write(work.join("new.txt"), "content\n").unwrap();

    let output = sandbox.gpp(&work, &["add", "the", "file"]);
    assert!(output.status.success());
    assert_eq!(
        sandbox.read(&work, &["log", "-1", "--format=%s"]),
        "add the file"
    );
    assert_eq!(
        sandbox.read(&work, &["rev-parse", "HEAD"]),
        sandbox.read(&work, &["rev-parse", "@{u}"])
    );
}

#[test]
fn untracked_files_are_staged_too() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    fs::create_dir(work.join("new")).unwrap();
    fs::write(work.join("new/file.txt"), "content\n").unwrap();

    assert!(
        sandbox
            .gpp(&work, &["add", "a", "directory"])
            .status
            .success()
    );
    assert_eq!(
        sandbox.read(&work, &["show", "--name-only", "--format=", "HEAD"]),
        "new/file.txt"
    );
}

#[test]
fn a_message_may_contain_flags() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    fs::write(work.join("new.txt"), "content\n").unwrap();

    assert!(
        sandbox
            .gpp(&work, &["teach", "-n", "to", "count"])
            .status
            .success()
    );
    assert_eq!(
        sandbox.read(&work, &["log", "-1", "--format=%s"]),
        "teach -n to count"
    );
}

#[test]
fn nothing_to_commit_stops_before_committing() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    let before = sandbox.read(&work, &["rev-parse", "HEAD"]);

    let output = sandbox.gpp(&work, &["empty"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("nothing to commit"));
    assert_eq!(sandbox.read(&work, &["rev-parse", "HEAD"]), before);
}

/// "Nothing to commit" is a question about the index, not about the working
/// tree: something staged earlier still has to be committed.
#[test]
fn something_staged_earlier_is_still_committed() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    fs::write(work.join("new.txt"), "content\n").unwrap();
    sandbox.git(&work, &["add", "new.txt"]);

    let output = sandbox.gpp(&work, &["stage", "then", "commit"]);
    assert!(output.status.success());
    assert_eq!(
        sandbox.read(&work, &["log", "-1", "--format=%s"]),
        "stage then commit"
    );
}

/// An unmerged index is one git cannot summarise with a cached tree, so the
/// question is answered the long way — and either way the conflict is git's
/// to report, not something to mistake for an empty index.
#[test]
fn a_conflict_is_left_for_git_to_report() {
    let sandbox = Sandbox::new();
    let work = sandbox.work();
    fs::create_dir(work.join("elsewhere")).unwrap();
    fs::write(work.join("elsewhere/other.txt"), "x\n").unwrap();
    sandbox.git(&work, &["add", "-A"]);
    sandbox.git(&work, &["commit", "--quiet", "-m", "elsewhere"]);
    sandbox.git(&work, &["checkout", "--quiet", "-b", "other"]);
    fs::write(work.join("seed"), "theirs\n").unwrap();
    sandbox.git(&work, &["commit", "--quiet", "-am", "theirs"]);
    sandbox.git(&work, &["checkout", "--quiet", "-"]);
    fs::write(work.join("seed"), "ours\n").unwrap();
    sandbox.git(&work, &["commit", "--quiet", "-am", "ours"]);
    let merge = sandbox
        .command("git", &work)
        .args(["merge", "other"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the merge should conflict");
    fs::write(work.join("elsewhere/other.txt"), "changed\n").unwrap();

    let output = sandbox.gpp(&work.join("elsewhere"), &["commit", "from", "here"]);
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("nothing to commit"));
}

#[test]
fn a_missing_message_is_a_usage_error() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.gpp(&sandbox.work(), &[]).status.code(), Some(2));
}

#[test]
fn outside_a_repository_git_decides_the_status() {
    let sandbox = Sandbox::new();
    let output = sandbox.gpp(sandbox.home.path(), &["nowhere"]);
    assert_eq!(output.status.code(), Some(128));
}

#[test]
fn completions_need_no_message() {
    let sandbox = Sandbox::new();
    let output = sandbox.gpp(&sandbox.work(), &["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("#compdef gpp"));
}

/// The shared `--completions` flag is flattened in, and clap will hand a
/// flattened struct's documentation to the command it lands in.
#[test]
fn help_describes_this_tool() {
    let sandbox = Sandbox::new();
    let output = sandbox.gpp(&sandbox.work(), &["--help"]);
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .starts_with("Stage everything, commit with the given message, and push")
    );
}
