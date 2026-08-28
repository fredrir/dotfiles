
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let sandbox = Sandbox {
            home: tempfile::tempdir().unwrap(),
        };
        let home = sandbox.home.path();
        let remotes = home.join("remotes");
        fs::create_dir_all(remotes.join("user")).unwrap();
        fs::create_dir_all(remotes.join("fredrir")).unwrap();
        fs::create_dir(home.join("work")).unwrap();
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
        sandbox.write(&source, "README.md", "main\n");
        sandbox.write(&source, ".hidden", "hidden\n");
        sandbox.write(&source, "folder_8/folder_9/a.txt", "nine\n");
        sandbox.write(&source, "folder_8/keep.txt", "keep\n");
        sandbox.git(&source, &["add", "."]);
        sandbox.git(&source, &["commit", "--quiet", "-m", "main"]);
        sandbox.git(&source, &["checkout", "--quiet", "-b", "dev"]);
        sandbox.write(&source, "README.md", "dev\n");
        sandbox.write(&source, "folder_8/folder_10/b.txt", "ten\n");
        sandbox.git(&source, &["add", "."]);
        sandbox.git(&source, &["commit", "--quiet", "-m", "dev"]);
        sandbox.git(&source, &["checkout", "--quiet", "main"]);

        for repository in ["user/repo", "fredrir/nsql"] {
            sandbox.git(
                home,
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

    fn work(&self) -> PathBuf {
        self.home.path().join("work")
    }

    fn write(&self, root: &Path, path: &str, content: &str) {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env("HOME", self.home.path())
            .env("GIT_CONFIG_GLOBAL", self.home.path().join("gitconfig"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env("NO_COLOR", "1");
        command
    }

    fn git(&self, cwd: &Path, arguments: &[&str]) -> Output {
        let output = self
            .command("git", cwd)
            .args(arguments)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {arguments:?} failed");
        output
    }

    fn gget(&self, arguments: &[&str], answers: &str) -> Output {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_gget"), &self.work())
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("gget runs");
        child
            .stdin
            .take()
            .expect("stdin is a pipe")
            .write_all(answers.as_bytes())
            .expect("the answers are read");
        child.wait_with_output().expect("gget finishes")
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.work().join(path)).expect("the file is here")
    }

    fn names(&self) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(self.work())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().display().to_string())
            .collect();
        found.sort();
        found
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_folder_comes_down_under_its_own_name() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["https://github.com/user/repo/folder_8/folder_9"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.names(), ["folder_9"]);
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
    let sandbox = Sandbox::new();
    let output = sandbox.gget(
        &["https://github.com/user/repo/tree/dev/folder_8/folder_10"],
        "",
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("folder_10/b.txt"), "ten\n");
}

#[test]
fn a_blob_url_brings_the_one_file() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["https://github.com/user/repo/blob/dev/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.names(), ["README.md"]);
    assert_eq!(sandbox.read("README.md"), "dev\n");
}

#[test]
fn the_owner_flag_stands_in_for_the_account() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-f", "nsql/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("README.md"), "main\n");
}

#[test]
fn a_branch_flag_chooses_the_version() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-f", "-b", "dev", "nsql/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("README.md"), "dev\n");
    assert_eq!(
        stdout(&output).trim(),
        "fredrir/nsql@dev README.md -> README.md"
    );
}

#[test]
fn a_branch_flag_overrules_the_url() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(
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
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["git@github.com:user/repo.git"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("repo/README.md"), "main\n");
}

#[test]
fn the_whole_repository_arrives_as_files() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["user/repo"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.names(), ["repo"]);
    assert_eq!(sandbox.read("repo/folder_8/folder_9/a.txt"), "nine\n");
    // Files, not a clone: there is nothing here to pull, push or blame.
    assert!(!sandbox.work().join("repo/.git").exists());
}

#[test]
fn replacing_something_is_asked_about_first() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.work().join("README.md"), "mine\n").unwrap();
    let output = sandbox.gget(&["-f", "nsql/README.md"], "n\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("overwrite README.md with fredrir/nsql"));
    assert!(stdout(&output).contains("gget: cancelled"));
    assert_eq!(sandbox.read("README.md"), "mine\n");

    let output = sandbox.gget(&["-f", "nsql/README.md"], "\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("README.md"), "main\n");
}

#[test]
fn a_directory_is_replaced_whole() {
    let sandbox = Sandbox::new();
    let stale = sandbox.work().join("folder_9");
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("gone.txt"), "gone\n").unwrap();
    let output = sandbox.gget(&["user/repo/folder_8/folder_9"], "y\n");
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(sandbox.read("folder_9/a.txt"), "nine\n");
    assert!(!stale.join("gone.txt").exists());
}

#[test]
fn the_yes_flag_answers_in_advance() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.work().join("README.md"), "mine\n").unwrap();
    let output = sandbox.gget(&["-y", "-f", "nsql/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stdout(&output).contains("overwrite"));
    assert_eq!(sandbox.read("README.md"), "main\n");
}

#[test]
fn no_answer_leaves_it_alone() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.work().join("README.md"), "mine\n").unwrap();
    let output = sandbox.gget(&["-f", "nsql/README.md"], "");
    assert!(!output.status.success());
    assert_eq!(sandbox.read("README.md"), "mine\n");
}

#[test]
fn a_path_that_is_not_there_says_so() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["user/repo/folder_8/nope"], "");
    assert!(!output.status.success());
    assert_eq!(
        stderr(&output).trim(),
        "gget: no folder_8/nope in user/repo@main"
    );
    assert!(sandbox.names().is_empty());
}

#[test]
fn a_branch_that_is_not_there_is_gits_own_answer() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-b", "nope", "user/repo/README.md"], "");
    assert_eq!(output.status.code(), Some(128));
    assert!(stderr(&output).contains("nope"));
    assert!(sandbox.names().is_empty());
}

#[test]
fn another_host_never_reaches_the_network() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["https://gitlab.com/user/repo"], "");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("not a github.com address"));
    assert!(sandbox.names().is_empty());
}

#[test]
fn a_failed_run_leaves_nothing_behind() {
    let sandbox = Sandbox::new();
    sandbox.gget(&["user/repo/folder_8/nope"], "");
    sandbox.gget(&["-b", "nope", "user/repo/README.md"], "");
    assert!(sandbox.names().is_empty());
}

#[test]
fn completions_need_no_target() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["--completions", "zsh"], "");
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef gget"));
}

#[test]
fn help_describes_this_tool() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["--help"], "");
    assert!(stdout(&output).starts_with("Download a file or folder out of"));
}

#[test]
fn a_listing_shows_what_the_directory_holds() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-l", "https://github.com/user/repo/folder_8"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    let listed = stdout(&output);
    assert!(listed.contains("folder_9"), "{listed}");
    assert!(listed.contains("keep.txt"), "{listed}");
    assert!(sandbox.names().is_empty());
}

#[test]
fn a_listing_hides_dotfiles_until_it_is_asked() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-l", "user/repo"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    let listed = stdout(&output);
    assert!(listed.contains("README.md"), "{listed}");
    assert!(!listed.contains(".hidden"), "{listed}");

    let output = sandbox.gget(&["-la", "user/repo"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains(".hidden"), "{}", stdout(&output));
}

#[test]
fn a_listing_reads_the_branch_out_of_the_url() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-l", "https://github.com/user/repo/tree/dev/folder_8"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("folder_10"), "{}", stdout(&output));
}

#[test]
fn listing_a_file_names_that_file() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-l", "user/repo/blob/main/README.md"], "");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("README.md"), "{}", stdout(&output));
    assert!(sandbox.names().is_empty());
}

#[test]
fn listing_a_path_that_is_not_there_says_so() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-l", "user/repo/folder_8/nope"], "");
    assert!(!output.status.success());
    assert_eq!(
        stderr(&output).trim(),
        "gget: no folder_8/nope in user/repo@main"
    );
    assert!(sandbox.names().is_empty());
}

#[test]
fn the_dotfile_flag_is_only_for_a_listing() {
    let sandbox = Sandbox::new();
    let output = sandbox.gget(&["-a", "user/repo"], "");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--list"), "{}", stderr(&output));
    assert!(sandbox.names().is_empty());
}
