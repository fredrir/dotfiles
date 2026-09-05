use super::*;

#[test]
fn a_plain_sandbox_has_a_home_an_empty_gitconfig_and_a_work_tree() {
    let sandbox = GitSandbox::plain();
    assert_eq!(
        fs::read_to_string(sandbox.home().join("gitconfig")).unwrap(),
        ""
    );
    assert!(sandbox.work().is_dir());
}

#[test]
fn a_committed_sandbox_starts_clean_with_its_seed_files() {
    let sandbox = GitSandbox::committed();
    assert_eq!(sandbox.status(), "");
    assert_eq!(sandbox.read("mod.txt"), "one\ntwo\n");
    assert_eq!(sandbox.read("del.txt"), "gone\n");
    assert_eq!(sandbox.read("keep.txt"), "keep\n");
    assert!(sandbox.exists(".gitignore"));
    assert_eq!(
        sandbox.git(&sandbox.work(), &["log", "-1", "--format=%s"]),
        "init"
    );
}

#[test]
fn the_author_and_the_branch_are_deterministic() {
    let sandbox = GitSandbox::committed();
    let work = sandbox.work();
    assert_eq!(
        sandbox.git(&work, &["log", "-1", "--format=%an <%ae>"]),
        "test <test@example.invalid>"
    );
    assert_eq!(
        sandbox.git(&work, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );
}

#[test]
fn the_seeded_ignore_rules_keep_the_status_clean() {
    let sandbox = GitSandbox::committed();
    sandbox.write("ignored.log", "noise\n");
    sandbox.write("ignored_dir/inside.txt", "noise\n");
    assert_eq!(sandbox.status(), "");
    assert!(sandbox.exists("ignored_dir/inside.txt"));
}

#[test]
fn writing_outside_the_work_tree_creates_its_parents() {
    let sandbox = GitSandbox::plain();
    let source = sandbox.home().join("source");
    sandbox.write_in(&source, "folder/deep.txt", "deep\n");
    assert_eq!(
        fs::read_to_string(source.join("folder/deep.txt")).unwrap(),
        "deep\n"
    );
}

#[test]
fn an_origin_sandbox_has_its_work_tree_pushed() {
    let sandbox = GitSandbox::with_origin();
    let work = sandbox.work();
    assert_eq!(sandbox.read("seed"), "seed\n");
    assert_eq!(
        sandbox.git(&work, &["rev-parse", "HEAD"]),
        sandbox.git(&work, &["rev-parse", "@{u}"])
    );
    assert!(sandbox.origin().join("HEAD").exists());
}

#[test]
fn a_sandbox_command_runs_where_it_is_told() {
    let sandbox = GitSandbox::committed();
    let output = sandbox
        .command("git", &sandbox.work())
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn a_sandbox_binary_inherits_the_sandbox_environment() {
    let sandbox = GitSandbox::committed();
    let ran = sandbox
        .bin("/bin/sh", &sandbox.work())
        .args([
            "-c",
            "printenv HOME; printenv GIT_AUTHOR_NAME; printenv NO_COLOR",
        ])
        .run();
    assert_eq!(
        ran.stdout,
        format!("{}\ntest\n1\n", sandbox.home().display())
    );
}
