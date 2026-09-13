#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output};
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal, terminal_state};

const PROMPT: &str = "[q/Enter] Abort";

fn token(character: char) -> String {
    format!("{}{}", "ghp_", character.to_string().repeat(36))
}

struct Fixture {
    temporary: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("config/targets.dotfile"), "").unwrap();
        let fixture = Self {
            temporary,
            root,
            home,
        };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "scanner review test"]);
        fixture.git(&["config", "user.email", "review@example.invalid"]);
        fixture.git(&["config", "core.hooksPath", "/dev/null"]);
        fixture.git(&["config", "commit.gpgSign", "false"]);
        fixture.git(&["add", "config/targets.dotfile"]);
        fixture.git(&["commit", "-qm", "initial"]);
        fixture
    }

    fn environment(&self, command: &mut Command) {
        command
            .env("DOTFILE_ROOT", &self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("CI")
            .env_remove("SOPS_AGE_KEY")
            .env_remove("SOPS_AGE_KEY_FILE")
            .env_remove("SOPS_AGE_KEY_CMD")
            .current_dir(&self.root);
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
        self.environment(&mut command);
        command.args(["secret", "scan"]).args(arguments);
        command
    }

    fn scan(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    fn git_command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new("git");
        self.environment(&mut command);
        command.args(arguments);
        command
    }

    fn git(&self, arguments: &[&str]) -> Output {
        let output = self.git_command(arguments).output().unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn stage(&self, relative: &str, text: &str) {
        self.write(relative, text);
        self.git(&["add", "--", relative]);
    }

    fn index(&self) -> Vec<u8> {
        fs::read(self.root.join(".git/index")).unwrap()
    }

    fn install_hooks(&self) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../.githooks");
        fs::create_dir_all(self.root.join(".githooks")).unwrap();
        for name in ["_dotfile.sh", "pre-commit", "pre-push"] {
            let destination = self.root.join(".githooks").join(name);
            fs::copy(source.join(name), &destination).unwrap();
            fs::set_permissions(destination, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bin = self.home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        symlink(env!("CARGO_BIN_EXE_dotfile"), bin.join("dotfile")).unwrap();
        self.git(&["config", "core.hooksPath", ".githooks"]);
    }

    fn canary(&self, value: &str) {
        let path = self.root.join("config/canaries");
        fs::write(&path, format!("private-fixture = {value}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn outgoing_branches(files: [(&str, String); 2]) -> Self {
        let fixture = Self::new();
        for (branch, (path, content)) in ["first", "second"].into_iter().zip(files) {
            fixture.git(&["checkout", "-qb", branch, "main"]);
            fixture.stage(path, &content);
            fixture.git(&["commit", "-qm", &format!("{branch} fixture")]);
        }
        let remote = fixture.temporary.path().join("origin.git");
        fixture.git(&["init", "-q", "--bare", remote.to_str().unwrap()]);
        fixture.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
        fixture.install_hooks();
        fixture
    }
}

struct Terminal {
    child: Child,
    master: File,
    before: libc::termios,
    output: Vec<u8>,
}
impl Terminal {
    fn spawn(mut command: Command) -> Self {
        let (master, slave, before) = open_pty(30, 120);
        let (input, output, error) = stdio(&slave);
        command.stdin(input).stdout(output).stderr(error);
        take_controlling_terminal(&mut command);
        Self {
            child: command.spawn().unwrap(),
            master,
            before,
            output: Vec::new(),
        }
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }
    fn wait_for(&mut self, text: &str, after: usize) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            read_available(&self.master, &mut self.output, 20);
            if String::from_utf8_lossy(&self.output[after..]).contains(text) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "process exited before {text:?}: {}",
                self.text()
            );
            assert!(
                Instant::now() < deadline,
                "waiting for {text:?}: {}",
                self.text()
            );
        }
    }
    fn send(&mut self, keys: &[u8]) -> usize {
        let offset = self.output.len();
        self.master.write_all(keys).unwrap();
        offset
    }
    fn finish(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            read_available(&self.master, &mut self.output, 20);
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "review did not finish: {}",
                self.text()
            );
        };
        read_available(&self.master, &mut self.output, 20);
        let after = terminal_state(&self.master);
        assert_eq!(
            self.before.c_lflag, after.c_lflag,
            "terminal local flags were not restored"
        );
        assert_eq!(
            self.before.c_iflag, after.c_iflag,
            "terminal input flags were not restored"
        );
        assert_eq!(
            self.before.c_oflag, after.c_oflag,
            "terminal output flags were not restored"
        );
        assert_eq!(
            self.before.c_cflag, after.c_cflag,
            "terminal control flags were not restored"
        );
        assert_eq!(
            self.before.c_cc, after.c_cc,
            "terminal control characters were not restored"
        );
        status
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn inspection_reads_redacted_staged_context_with_stdout_hidden_and_stdin_untouched() {
    let fixture = Fixture::new();
    let path = "example\n\x1b[2J\u{202e}.txt";
    let secret = token('Q');
    let staged = format!(
        "STAGED_BEFORE café \x1b]0;fixture-title\x07\u{202e}\nexample = {secret}\nSTAGED_AFTER\n"
    );
    fixture.stage(path, &staged);
    fixture.write(path, "WORKTREE_ONLY\n");
    let before = fixture.index();
    let stream = "refs/heads/first 111111 refs/heads/first 000000\nrefs/heads/second 222222 refs/heads/second 000000\n";
    let input = fixture.temporary.path().join("hook-input");
    let unread = fixture.temporary.path().join("unread-input");
    fs::write(&input, stream).unwrap();
    let mut command = Command::new("sh");
    fixture.environment(&mut command);
    command.env("REVIEW_BINARY", env!("CARGO_BIN_EXE_dotfile"))
        .env("REVIEW_INPUT", &input).env("REVIEW_UNREAD", &unread)
        .args(["-c", "exec 0<\"$REVIEW_INPUT\"; \"$REVIEW_BINARY\" secret scan --staged --review >/dev/null; result=$?; cat >\"$REVIEW_UNREAD\"; exit \"$result\""]);
    let mut terminal = Terminal::spawn(command);
    terminal.wait_for(PROMPT, 0);
    let after = terminal.send(b"i");
    terminal.wait_for("Inspection", after);
    terminal.wait_for("STAGED_BEFORE", after);
    terminal.wait_for(PROMPT, after);
    terminal.send(b"q");
    assert!(!terminal.finish().success());
    let output = terminal.text();
    assert!(output.contains("STAGED_AFTER"));
    assert!(output.contains("redacted"));
    assert!(!output.contains(&secret));
    assert!(!output.contains("WORKTREE_ONLY"));
    assert!(!output.contains("\x1b]0;fixture-title\x07"));
    assert!(!output.contains("\x1b[2J"));
    assert!(!output.contains('\u{202e}'));
    assert!(output.contains("example\\n\\u{1b}[2J\\u{202e}.txt"));
    assert_eq!(fs::read_to_string(unread).unwrap(), stream);
    assert_eq!(fixture.index(), before);
    assert_eq!(
        fs::read_to_string(fixture.root.join(path)).unwrap(),
        "WORKTREE_ONLY\n"
    );
}

#[test]
fn enter_escape_eof_and_control_c_abort_without_remembering_approval() {
    for key in [&b"\r"[..], &b"\x1b"[..], &b"\x04"[..], &b"\x03"[..]] {
        let fixture = Fixture::new();
        fixture.stage("example.txt", &token('R'));
        let before = fixture.index();
        let mut terminal = Terminal::spawn(fixture.command(&["--staged", "--review"]));
        terminal.wait_for(PROMPT, 0);
        terminal.send(key);
        let status = terminal.finish();
        assert!(!status.success(), "{key:?}: {}", terminal.text());
        if key == b"\x03" {
            assert_eq!(status.code(), Some(130));
        }
        assert_eq!(fixture.index(), before);
        assert!(!fixture.scan(&["--staged"]).status.success());
    }
}

#[test]
fn termination_restores_the_terminal_and_discards_pending_approval() {
    let fixture = Fixture::new();
    fixture.stage("example.txt", &token('D'));
    let before = fixture.index();
    let mut terminal = Terminal::spawn(fixture.command(&["--staged", "--review"]));
    terminal.wait_for(PROMPT, 0);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(terminal.child.id().try_into().unwrap()),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    assert_eq!(terminal.finish().code(), Some(143));
    assert_eq!(fixture.index(), before);
    assert!(!fixture.scan(&["--staged"]).status.success());
}

#[test]
fn redirected_stderr_and_ci_fail_closed_without_a_review_prompt() {
    let fixture = Fixture::new();
    let secret = token('S');
    fixture.stage("example.txt", &secret);
    let before = fixture.index();
    let output = fixture.scan(&["--staged", "--review"]);
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&secret));
    let mut command = fixture.command(&["--staged", "--review"]);
    command.env("CI", "true");
    let mut terminal = Terminal::spawn(command);
    assert!(!terminal.finish().success());
    assert!(!terminal.text().contains(PROMPT));
    assert_eq!(fixture.index(), before);
}

#[test]
fn compact_review_uses_one_counted_summary_and_honors_terminal_color_policy() {
    let sgr = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    for (policy, setting, colored) in [
        ("enabled", None, true),
        ("NO_COLOR", Some(("NO_COLOR", "1")), false),
        ("TERM=dumb", Some(("TERM", "dumb")), false),
        ("CLICOLOR=0", Some(("CLICOLOR", "0")), false),
    ] {
        let fixture = Fixture::new();
        let secret = token('P');
        fixture.stage("first.txt", &format!("{secret}\n").repeat(16));
        fixture.stage("second.txt", &token('N'));
        let before = fixture.index();
        let mut command = fixture.command(&["--staged", "--review"]);
        command
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR")
            .env_remove("THEME_DIR")
            .env_remove("THEME_GIT")
            .env_remove("THEME_SUDO");
        if let Some((name, value)) = setting {
            command.env(name, value);
        }
        let mut terminal = Terminal::spawn(command);
        terminal.wait_for("Abort", 0);
        let raw = terminal.text();
        assert_eq!(sgr.is_match(&raw), colored, "{policy}: {raw:?}");
        let display = sgr.replace_all(&raw, "");
        assert!(
            display.contains("Secret review  17 findings 2 files"),
            "{policy}: {display}"
        );
        assert!(display.contains("1/2  first.txt"), "{policy}: {display}");
        assert!(display.contains("github-token ×16"), "{policy}: {display}");
        assert!(display.contains("[a] Accept & remember"), "{display}");
        assert!(display.contains(PROMPT), "{display}");
        for once in ["Secret review", "first.txt", "github-token"] {
            assert_eq!(display.matches(once).count(), 1, "{policy}: {display}");
        }
        for absent in [
            "second.txt",
            "✗ pattern",
            "blob ",
            "sha256 ",
            "this exact file",
        ] {
            assert!(!display.contains(absent), "{policy}: {display}");
        }
        assert!(!display.contains(&secret));
        let after = terminal.send(b"i");
        terminal.wait_for("Inspection", after);
        terminal.wait_for("Abort", after);
        let inspection = String::from_utf8_lossy(&terminal.output[after..]);
        assert_eq!(
            sgr.is_match(&inspection),
            colored,
            "{policy}: {inspection:?}"
        );
        let inspection = sgr.replace_all(&inspection, "");
        assert!(
            inspection.contains("Inspection 1/1 masked"),
            "{policy}: {inspection}"
        );
        assert!(inspection.contains("[redacted]"), "{inspection}");
        assert!(!inspection.contains(&secret));
        terminal.send(b"q");
        assert!(!terminal.finish().success());
        assert_eq!(fixture.index(), before);
    }
}

#[test]
fn short_assigned_secrets_block_the_commit_hook_and_are_masked_during_review() {
    let fixture = Fixture::new();
    let assignment = format!("API_KEY={}\n", 123);
    fixture.stage("test.txt", &assignment);
    fixture.install_hooks();
    let head = fixture.git(&["rev-parse", "HEAD"]).stdout;
    let before = fixture.git(&["ls-files", "--stage", "-z"]).stdout;
    let output = fixture
        .git_command(&["commit", "-m", "short secret"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("test.txt:1"), "{error}");
    assert!(error.contains("value"), "{error}");
    assert!(!error.contains(assignment.trim()));

    let mut terminal = Terminal::spawn(fixture.git_command(&["commit", "-m", "short secret"]));
    terminal.wait_for(PROMPT, 0);
    let after = terminal.send(b"i");
    terminal.wait_for("API_KEY=[redacted]", after);
    terminal.wait_for(PROMPT, after);
    terminal.send(b"q");
    assert!(!terminal.finish().success());
    assert!(!terminal.text().contains(assignment.trim()));
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).stdout, head);
    assert_eq!(fixture.git(&["ls-files", "--stage", "-z"]).stdout, before);
    assert!(
        !fixture
            .root
            .join(".git/dotfile/scan-approvals.json")
            .exists()
    );
}

#[test]
fn a_real_commit_hook_accepts_one_exact_blob_and_later_edits_require_review_again() {
    let fixture = Fixture::new();
    let first = token('T');
    let second = token('U');
    fixture.stage(
        "example.txt",
        &format!("known synthetic fixture\n{first}\n"),
    );
    fixture.write("example.txt", "UNSTAGED_WORKTREE_VERSION\n");
    fixture.install_hooks();
    let mut terminal = Terminal::spawn(fixture.git_command(&["commit", "-m", "review fixture"]));
    terminal.wait_for(PROMPT, 0);
    terminal.send(b"a");
    assert!(terminal.finish().success(), "{}", terminal.text());
    assert!(!terminal.text().contains(&first));
    assert_eq!(
        fs::read_to_string(fixture.root.join("example.txt")).unwrap(),
        "UNSTAGED_WORKTREE_VERSION\n"
    );
    let approvals = fixture.root.join(".git/dotfile/scan-approvals.json");
    let stored = fs::read(&approvals).unwrap();
    assert!(!String::from_utf8_lossy(&stored).contains(&first));
    assert_eq!(
        fs::metadata(approvals).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        fixture.scan(&["--staged"]).status.success(),
        "approved unchanged blob must pass headlessly"
    );
    let history = fixture.scan(&["--commits", "HEAD"]);
    assert!(
        history.status.success(),
        "identical committed content should retain approval: {}",
        String::from_utf8_lossy(&history.stderr)
    );
    fixture.stage(
        "example.txt",
        &format!("changed synthetic fixture\n{second}\n"),
    );
    assert!(
        !fixture.scan(&["--staged"]).status.success(),
        "approval must not extend to changed bytes"
    );
    fixture.stage(
        "copied-example.txt",
        &format!("known synthetic fixture\n{first}\n"),
    );
    let copied = fixture.scan(&["--staged"]);
    assert!(
        String::from_utf8_lossy(&copied.stderr).contains("copied-example.txt"),
        "approval must remain tied to the reviewed path"
    );
}

#[test]
fn aborting_a_later_group_discards_all_pending_approvals() {
    let fixture = Fixture::new();
    fixture.stage("first.txt", &token('V'));
    fixture.stage("second.txt", &token('W'));
    let before = fixture.index();
    let mut terminal = Terminal::spawn(fixture.command(&["--staged", "--review"]));
    terminal.wait_for(PROMPT, 0);
    let after = terminal.send(b"a");
    terminal.wait_for(PROMPT, after);
    terminal.send(b"q");
    assert!(!terminal.finish().success());
    assert_eq!(fixture.index(), before);
    let output = fixture.scan(&["--staged"]);
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(report.contains("first.txt"), "{report}");
    assert!(report.contains("second.txt"), "{report}");
}

#[test]
fn changing_the_index_during_review_rejects_approval_of_the_old_snapshot() {
    let fixture = Fixture::new();
    let original = token('X');
    fixture.stage("example.txt", &original);
    let mut terminal = Terminal::spawn(fixture.command(&["--staged", "--review"]));
    terminal.wait_for(PROMPT, 0);
    fixture.stage("example.txt", &token('Y'));
    let changed = fixture.index();
    terminal.send(b"a");
    assert!(!terminal.finish().success(), "{}", terminal.text());
    assert_eq!(fixture.index(), changed);
    fixture.stage("example.txt", &original);
    assert!(!fixture.scan(&["--staged"]).status.success());
}

#[test]
fn changing_working_tree_content_during_review_rejects_the_old_snapshot() {
    let fixture = Fixture::new();
    let original = token('E');
    fixture.stage("example.txt", &original);
    let before = fixture.index();
    let mut terminal = Terminal::spawn(fixture.command(&["--review", "example.txt"]));
    terminal.wait_for(PROMPT, 0);
    fixture.write("example.txt", &token('F'));
    terminal.send(b"a");
    assert!(!terminal.finish().success(), "{}", terminal.text());
    assert_eq!(fixture.index(), before);
    fixture.write("example.txt", &original);
    assert!(!fixture.scan(&["example.txt"]).status.success());
}

#[test]
fn canaries_and_encryption_invariants_never_offer_acceptance() {
    for canary in [
        None,
        Some((
            "private-fixture-host.invalid",
            "private-fixture-host.invalid",
        )),
        Some((
            "prİvate-fixture-host.invalid",
            "PRİVATE-FIXTURE-HOST.INVALID",
        )),
    ] {
        let fixture = Fixture::new();
        let plaintext = "PRIVATE_UNSTRUCTURED_CONTENT_SHOULD_NEVER_BE_DISPLAYED";
        if let Some((needle, value)) = canary {
            fixture.canary(needle);
            fixture.stage("example.txt", value);
        } else {
            fixture.stage("shared/private/.secret", "");
            fixture.stage("shared/private/plain.txt", plaintext);
        }
        let before = fixture.index();
        let mut terminal = Terminal::spawn(fixture.command(&["--staged", "--review"]));
        terminal.wait_for(PROMPT, 0);
        assert!(
            !terminal.text().contains("[a] Accept"),
            "{}",
            terminal.text()
        );
        let after = terminal.send(b"i");
        terminal.wait_for(
            if canary.is_some() {
                "Inspection"
            } else {
                "Withheld"
            },
            after,
        );
        terminal.wait_for(PROMPT, after);
        assert!(!terminal.text().contains(plaintext));
        if let Some((_, value)) = canary {
            assert!(!terminal.text().contains(value));
        } else {
            assert!(terminal.text().contains("Contents withheld"));
        }
        terminal.send(b"aq");
        assert!(!terminal.finish().success());
        if let Some((_, value)) = canary {
            assert!(!terminal.text().contains(value));
        }
        assert_eq!(fixture.index(), before);
        assert!(!fixture.scan(&["--staged"]).status.success());
    }
}

#[test]
fn remembered_pattern_approval_cannot_override_new_canaries_or_encryption_rules() {
    for add_canary in [false, true] {
        let fixture = Fixture::new();
        let secret = token('L');
        fixture.stage("shared/private/example.txt", &secret);
        let mut initial = Terminal::spawn(fixture.command(&["--staged", "--review"]));
        initial.wait_for(PROMPT, 0);
        initial.send(b"a");
        assert!(initial.finish().success(), "{}", initial.text());
        assert!(fixture.scan(&["--staged"]).status.success());
        if add_canary {
            fixture.canary(&secret);
        } else {
            fixture.stage("shared/private/.secret", "");
        }
        assert!(!fixture.scan(&["--staged"]).status.success());
        let mut terminal = Terminal::spawn(fixture.command(&["--staged", "--review"]));
        terminal.wait_for(PROMPT, 0);
        assert!(!terminal.text().contains("[a] Accept"));
        terminal.send(b"q");
        assert!(!terminal.finish().success());
        assert!(!terminal.text().contains(&secret));
    }
}

#[test]
fn a_real_two_ref_push_reviews_both_stream_records_without_consuming_hook_stdin() {
    let fixture = Fixture::outgoing_branches([
        ("first-example.txt", token('A')),
        ("second-example.txt", token('B')),
    ]);
    let mut terminal = Terminal::spawn(fixture.git_command(&["push", "origin", "first", "second"]));
    terminal.wait_for(PROMPT, 0);
    let after = terminal.send(b"a");
    terminal.wait_for(PROMPT, after);
    terminal.send(b"a");
    assert!(terminal.finish().success(), "{}", terminal.text());
    assert!(terminal.text().contains("first-example.txt"));
    assert!(terminal.text().contains("second-example.txt"));
    assert!(terminal.text().contains("1/2  first-example.txt"));
    assert!(terminal.text().contains("2/2  second-example.txt"));
    assert!(!terminal.text().contains(&token('A')));
    assert!(!terminal.text().contains(&token('B')));
    let remote_heads = fixture.git(&["ls-remote", "--heads", "origin"]);
    let heads = String::from_utf8(remote_heads.stdout).unwrap();
    assert!(heads.contains("refs/heads/first"));
    assert!(heads.contains("refs/heads/second"));
}

#[test]
fn aborting_the_second_push_group_saves_no_approvals_or_remote_refs() {
    let fixture = Fixture::outgoing_branches([
        ("first-example.txt", token('G')),
        ("second-example.txt", token('H')),
    ]);
    let mut terminal = Terminal::spawn(fixture.git_command(&["push", "origin", "first", "second"]));
    terminal.wait_for(PROMPT, 0);
    let after = terminal.send(b"a");
    terminal.wait_for(PROMPT, after);
    terminal.send(b"q");
    assert!(!terminal.finish().success(), "{}", terminal.text());
    assert!(
        fixture
            .git(&["ls-remote", "--heads", "origin"])
            .stdout
            .is_empty()
    );
    assert!(
        !fixture
            .root
            .join(".git/dotfile/scan-approvals.json")
            .exists()
    );
    let output = fixture.scan(&["--commits", "first", "--commits", "second"]);
    assert!(!output.status.success());
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(report.contains("first-example.txt"), "{report}");
    assert!(report.contains("second-example.txt"), "{report}");
}

#[test]
fn identical_content_shared_by_outgoing_refs_is_reviewed_once() {
    let secret = token('J');
    let fixture = Fixture::outgoing_branches([
        ("example.txt", secret.clone()),
        ("example.txt", secret.clone()),
    ]);
    let mut terminal = Terminal::spawn(fixture.git_command(&["push", "origin", "first", "second"]));
    terminal.wait_for(PROMPT, 0);
    terminal.send(b"a");
    assert!(terminal.finish().success(), "{}", terminal.text());
    let output = terminal.text();
    assert_eq!(output.matches(PROMPT).count(), 1, "{output}");
    assert!(output.contains("1/1  example.txt"), "{output}");
    assert!(!output.contains(&secret));
    let heads = fixture.git(&["ls-remote", "--heads", "origin"]);
    let heads = String::from_utf8_lossy(&heads.stdout);
    assert!(heads.contains("refs/heads/first"));
    assert!(heads.contains("refs/heads/second"));
}
