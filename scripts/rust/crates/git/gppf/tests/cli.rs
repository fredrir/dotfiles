use std::fs;
use std::path::Path;
use std::process::Output;

use testkit::{GitSandbox, stderr, stdout};

fn gppf(sandbox: &GitSandbox, cwd: &Path, arguments: &[&str]) -> Output {
    sandbox
        .bin(env!("CARGO_BIN_EXE_gppf"), cwd)
        .args(arguments)
        .output()
}

#[test]
fn stages_commits_and_pushes() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    fs::write(work.join("new.txt"), "content\n").unwrap();

    let output = gppf(&sandbox, &work, &["add", "the", "file"]);
    assert!(output.status.success());
    assert_eq!(
        sandbox.git(&work, &["log", "-1", "--format=%s"]),
        "add the file"
    );
    assert_eq!(
        sandbox.git(&work, &["rev-parse", "HEAD"]),
        sandbox.git(&work, &["rev-parse", "@{u}"])
    );
}

#[test]
fn untracked_files_are_staged_too() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    fs::create_dir(work.join("new")).unwrap();
    fs::write(work.join("new/file.txt"), "content\n").unwrap();

    assert!(
        gppf(&sandbox, &work, &["add", "a", "directory"])
            .status
            .success()
    );
    assert_eq!(
        sandbox.git(&work, &["show", "--name-only", "--format=", "HEAD"]),
        "new/file.txt"
    );
}

#[test]
fn a_message_may_contain_flags() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    fs::write(work.join("new.txt"), "content\n").unwrap();

    assert!(
        gppf(&sandbox, &work, &["teach", "-n", "to", "count"])
            .status
            .success()
    );
    assert_eq!(
        sandbox.git(&work, &["log", "-1", "--format=%s"]),
        "teach -n to count"
    );
}

#[test]
fn nothing_to_commit_stops_before_committing() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    let before = sandbox.git(&work, &["rev-parse", "HEAD"]);

    let output = gppf(&sandbox, &work, &["empty"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("nothing to commit"));
    assert_eq!(sandbox.git(&work, &["rev-parse", "HEAD"]), before);
}

#[test]
fn staging_starts_at_the_repository_root() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    fs::create_dir(work.join("nested")).unwrap();
    fs::write(work.join("nested/inner.txt"), "inner\n").unwrap();
    fs::write(work.join("root.txt"), "root\n").unwrap();

    let output = gppf(
        &sandbox,
        &work.join("nested"),
        &["from", "a", "subdirectory"],
    );
    assert!(output.status.success());
    assert_eq!(
        sandbox.git(&work, &["show", "--name-only", "--format=", "HEAD"]),
        "nested/inner.txt\nroot.txt"
    );
}

#[test]
fn something_staged_earlier_is_still_committed() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    fs::write(work.join("new.txt"), "content\n").unwrap();
    sandbox.git(&work, &["add", "new.txt"]);

    let output = gppf(&sandbox, &work, &["stage", "then", "commit"]);
    assert!(output.status.success());
    assert_eq!(
        sandbox.git(&work, &["log", "-1", "--format=%s"]),
        "stage then commit"
    );
}

#[test]
fn a_conflicted_merge_is_committed_as_it_stands() {
    let sandbox = GitSandbox::with_origin();
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

    let output = gppf(
        &sandbox,
        &work.join("elsewhere"),
        &["commit", "from", "here"],
    );
    assert!(output.status.success());
    assert_eq!(
        sandbox.git(&work, &["log", "-1", "--format=%s"]),
        "commit from here"
    );
    // The merge concluded: `HEAD` has a second parent to resolve.
    sandbox.git(&work, &["rev-parse", "--verify", "HEAD^2"]);
}

#[test]
fn a_missing_message_defaults_to_dot() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    fs::write(work.join("new.txt"), "content\n").unwrap();

    assert!(gppf(&sandbox, &work, &[]).status.success());
    assert_eq!(sandbox.git(&work, &["log", "-1", "--format=%s"]), ".");
    assert_eq!(
        sandbox.git(&work, &["rev-parse", "HEAD"]),
        sandbox.git(&work, &["rev-parse", "@{u}"])
    );
}

#[test]
fn outside_a_repository_git_decides_the_status() {
    let sandbox = GitSandbox::with_origin();
    let output = gppf(&sandbox, sandbox.home(), &["nowhere"]);
    assert_eq!(output.status.code(), Some(128));
}

#[test]
fn completions_need_no_message() {
    let sandbox = GitSandbox::with_origin();
    let output = gppf(&sandbox, &sandbox.work(), &["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef gpp"));
}

#[test]
fn help_advertises_the_default_message() {
    let sandbox = GitSandbox::with_origin();
    let output = gppf(&sandbox, &sandbox.work(), &["--help"]);
    let printed = stdout(&output);
    assert!(printed.starts_with("Stage everything, commit with the given message, and push"));
    assert!(
        printed.contains("Usage: gppf [OPTIONS] [MESSAGE]..."),
        "{printed}"
    );
    assert!(printed.contains("[default: .]"), "{printed}");
}
