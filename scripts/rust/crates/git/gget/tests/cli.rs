use std::fs;
use std::process::Output;

use testkit::{GitSandbox, names, stderr, stdout};

fn sandbox() -> GitSandbox {
    let sandbox = GitSandbox::plain();
    let home = sandbox.home().to_path_buf();
    let remotes = home.join("remotes");
    fs::create_dir_all(remotes.join("user")).unwrap();
    fs::create_dir_all(remotes.join("fredrir")).unwrap();
    fs::write(
        home.join("gitconfig"),
        format!(
            "[url \"file://{}/\"]\n\tinsteadOf = https://github.com/\n\
             [uploadpack]\n\tallowfilter = true\n",
            remotes.display()
        ),
    )
    .unwrap();

    // main: two folders and a README; dev: a third folder and a README of
    // its own, so which branch a run used is visible in what it wrote.
    let source = home.join("source");
    fs::create_dir(&source).unwrap();
    sandbox.git(&source, &["init", "--quiet", "-b", "main"]);
    sandbox.write_in(&source, "README.md", "main\n");
    sandbox.write_in(&source, ".hidden", "hidden\n");
    sandbox.write_in(&source, "folder_8/folder_9/a.txt", "nine\n");
    sandbox.write_in(&source, "folder_8/keep.txt", "keep\n");
    sandbox.git(&source, &["add", "."]);
    sandbox.git(&source, &["commit", "--quiet", "-m", "main"]);
    sandbox.git(&source, &["checkout", "--quiet", "-b", "dev"]);
    sandbox.write_in(&source, "README.md", "dev\n");
    sandbox.write_in(&source, "folder_8/folder_10/b.txt", "ten\n");
    sandbox.git(&source, &["add", "."]);
    sandbox.git(&source, &["commit", "--quiet", "-m", "dev"]);
    sandbox.git(&source, &["checkout", "--quiet", "main"]);

    for repository in ["user/repo", "fredrir/nsql"] {
        sandbox.git(
            &home,
            &[
                "clone",
                "--quiet",
                "--bare",
                source.to_str().unwrap(),
                remotes.join(repository).to_str().unwrap(),
            ],
        );
    }
    sandbox
}

fn gget(sandbox: &GitSandbox, arguments: &[&str], answers: &str) -> Output {
    sandbox
        .bin(env!("CARGO_BIN_EXE_gget"), &sandbox.work())
        .args(arguments)
        .stdin(answers)
        .output()
}

fn listed(sandbox: &GitSandbox) -> Vec<String> {
    names(&sandbox.work())
}

#[test]
fn a_folder_comes_down_under_its_own_name() {
    let sandbox = sandbox();
    let output = gget(
        &sandbox,
        &["https://github.com/user/repo/folder_8/folder_9"],
        "",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(listed(&sandbox), ["folder_9"]);
    assert_eq!(sandbox.read("folder_9/a.txt"), "nine\n");
    // The folders above it, its neighbours, and the repository it lives in
    // are all things that were not asked for.
    assert_eq!(
        stdout(&output).trim(),
        "user/repo@main folder_8/folder_9 -> folder_9"
    );
}

#[test]
fn a_ref_in_the_url_is_the_branch_it_comes_from() {
    let sandbox = sandbox();
    let output = gget(
        &sandbox,
        &["https://github.com/user/repo/tree/dev/folder_8/folder_10"],
        "",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("folder_10/b.txt"), "ten\n");
}

#[test]
fn a_blob_url_brings_the_one_file() {
    let sandbox = sandbox();
    let output = gget(
        &sandbox,
        &["https://github.com/user/repo/blob/dev/README.md"],
        "",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(listed(&sandbox), ["README.md"]);
    assert_eq!(sandbox.read("README.md"), "dev\n");
}

#[test]
fn the_owner_flag_stands_in_for_the_account() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["-f", "nsql/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("README.md"), "main\n");
}

#[test]
fn a_branch_flag_chooses_the_version() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["-f", "-b", "dev", "nsql/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("README.md"), "dev\n");
    assert_eq!(
        stdout(&output).trim(),
        "fredrir/nsql@dev README.md -> README.md"
    );
}

#[test]
fn a_branch_flag_overrules_the_url() {
    let sandbox = sandbox();
    let output = gget(
        &sandbox,
        &[
            "-b",
            "dev",
            "https://github.com/user/repo/tree/main/README.md",
        ],
        "",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("README.md"), "dev\n");
}

#[test]
fn the_ssh_form_names_the_same_place() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["git@github.com:user/repo.git"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("repo/README.md"), "main\n");
}

#[test]
fn the_whole_repository_arrives_as_files() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["user/repo"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(listed(&sandbox), ["repo"]);
    assert_eq!(sandbox.read("repo/folder_8/folder_9/a.txt"), "nine\n");
    // Files, not a clone: there is nothing here to pull, push or blame.
    assert!(!sandbox.exists("repo/.git"));
}

#[test]
fn replacing_something_is_asked_about_first() {
    let sandbox = sandbox();
    sandbox.write("README.md", "mine\n");
    let output = gget(&sandbox, &["-f", "nsql/README.md"], "n\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("overwrite README.md with fredrir/nsql"));
    assert!(stdout(&output).contains("gget: cancelled"));
    assert_eq!(sandbox.read("README.md"), "mine\n");

    let output = gget(&sandbox, &["-f", "nsql/README.md"], "\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("README.md"), "main\n");
}

#[test]
fn a_directory_is_replaced_whole() {
    let sandbox = sandbox();
    sandbox.write("folder_9/gone.txt", "gone\n");
    let output = gget(&sandbox, &["user/repo/folder_8/folder_9"], "y\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("folder_9/a.txt"), "nine\n");
    assert!(!sandbox.exists("folder_9/gone.txt"));
}

#[test]
fn the_yes_flag_answers_in_advance() {
    let sandbox = sandbox();
    sandbox.write("README.md", "mine\n");
    let output = gget(&sandbox, &["-y", "-f", "nsql/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stdout(&output).contains("overwrite"));
    assert_eq!(sandbox.read("README.md"), "main\n");
}

#[test]
fn no_answer_leaves_it_alone() {
    let sandbox = sandbox();
    sandbox.write("README.md", "mine\n");
    let output = gget(&sandbox, &["-f", "nsql/README.md"], "");
    assert!(!output.status.success());
    assert_eq!(sandbox.read("README.md"), "mine\n");
}

#[test]
fn a_path_that_is_not_there_says_so() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["user/repo/folder_8/nope"], "");
    assert!(!output.status.success());
    assert_eq!(
        stderr(&output).trim(),
        "gget: no folder_8/nope in user/repo@main"
    );
    assert!(listed(&sandbox).is_empty());
}

#[test]
fn a_branch_that_is_not_there_is_gits_own_answer() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["-b", "nope", "user/repo/README.md"], "");
    assert_eq!(output.status.code(), Some(128));
    assert!(stderr(&output).contains("nope"));
    assert!(listed(&sandbox).is_empty());
}

#[test]
fn another_host_never_reaches_the_network() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["https://gitlab.com/user/repo"], "");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("not a github.com address"));
    assert!(listed(&sandbox).is_empty());
}

#[test]
fn a_failed_run_leaves_nothing_behind() {
    let sandbox = sandbox();
    gget(&sandbox, &["user/repo/folder_8/nope"], "");
    gget(&sandbox, &["-b", "nope", "user/repo/README.md"], "");
    assert!(listed(&sandbox).is_empty());
}

#[test]
fn completions_need_no_target() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["--completions", "zsh"], "");
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef gget"));
}

#[test]
fn help_describes_this_tool() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["--help"], "");
    assert!(stdout(&output).starts_with("Download a file or folder out of"));
}

#[test]
fn a_listing_shows_what_the_directory_holds() {
    let sandbox = sandbox();
    let output = gget(
        &sandbox,
        &["-l", "https://github.com/user/repo/folder_8"],
        "",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("folder_9"), "{listing}");
    assert!(listing.contains("keep.txt"), "{listing}");
    assert!(listed(&sandbox).is_empty());
}

#[test]
fn a_listing_hides_dotfiles_until_it_is_asked() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["-l", "user/repo"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    let listing = stdout(&output);
    assert!(listing.contains("README.md"), "{listing}");
    assert!(!listing.contains(".hidden"), "{listing}");

    let output = gget(&sandbox, &["-la", "user/repo"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains(".hidden"), "{}", stdout(&output));
}

#[test]
fn a_listing_reads_the_branch_out_of_the_url() {
    let sandbox = sandbox();
    let output = gget(
        &sandbox,
        &["-l", "https://github.com/user/repo/tree/dev/folder_8"],
        "",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("folder_10"), "{}", stdout(&output));
}

#[test]
fn listing_a_file_names_that_file() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["-l", "user/repo/blob/main/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("README.md"), "{}", stdout(&output));
    assert!(listed(&sandbox).is_empty());
}

#[test]
fn listing_a_path_that_is_not_there_says_so() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["-l", "user/repo/folder_8/nope"], "");
    assert!(!output.status.success());
    assert_eq!(
        stderr(&output).trim(),
        "gget: no folder_8/nope in user/repo@main"
    );
    assert!(listed(&sandbox).is_empty());
}

#[test]
fn the_dotfile_flag_is_only_for_a_listing() {
    let sandbox = sandbox();
    let output = gget(&sandbox, &["-a", "user/repo"], "");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--list"), "{}", stderr(&output));
    assert!(listed(&sandbox).is_empty());
}
