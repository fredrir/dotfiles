use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Repository {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    binary: PathBuf,
}

impl Repository {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("environment/test")).unwrap();
        fs::create_dir_all(root.join("shared")).unwrap();
        fs::create_dir_all(home.join(".config/dotfile")).unwrap();
        fs::create_dir_all(home.join(".ssh")).unwrap();
        fs::write(root.join("config/targets.dotfile"), "").unwrap();
        fs::write(root.join("environment/test/manifest"), "shared\n").unwrap();
        fs::write(home.join(".config/dotfile/profile"), "test\n").unwrap();
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_dotfile"));
        let repository = Self {
            _temporary: temporary,
            root,
            home,
            binary,
        };
        repository.git(&["init", "-q"]);
        repository.git(&["config", "user.email", "secret-test@example.invalid"]);
        repository.git(&["config", "user.name", "secret test"]);
        repository
    }
    fn command(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .arg("secret")
            .env("DOTFILE_ROOT", &self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env_remove("SOPS_AGE_KEY")
            .env_remove("SOPS_AGE_KEY_CMD")
            .env("NO_COLOR", "1");
        command
    }
    fn run(&self, arguments: &[&str]) -> Output {
        self.command().args(arguments).output().unwrap()
    }
    fn ok(&self, arguments: &[&str]) -> Output {
        let output = self.run(arguments);
        assert!(
            output.status.success(),
            "{arguments:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn git(&self, arguments: &[&str]) -> Output {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn init(&self) {
        self.ok(&["init"]);
        self.ok(&["enroll", "machine"]);
    }
    fn identity(&self) -> PathBuf {
        self.home.join(".config/dotfile/age/keys.txt")
    }
    fn recovery(&self) -> PathBuf {
        let path = self._temporary.path().join("recovery.txt");
        let status = Command::new("age-keygen")
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
        assert!(status.status.success());
        let public = Command::new("age-keygen")
            .arg("-y")
            .arg(&path)
            .output()
            .unwrap();
        let key = String::from_utf8(public.stdout).unwrap();
        self.ok(&["enroll", "recovery", key.trim()]);
        path
    }
    fn add(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.home.join(".ssh").join(name);
        fs::write(&path, data).unwrap();
        self.ok(&["add", path.to_str().unwrap(), "--pkg", "ssh"]);
        path
    }
    fn decrypts(&self, file: &Path, identity: &Path) -> bool {
        Command::new("sops")
            .arg("-d")
            .arg(file)
            .env("SOPS_AGE_KEY_FILE", identity)
            .env_remove("SOPS_AGE_KEY")
            .env_remove("SOPS_AGE_KEY_CMD")
            .output()
            .unwrap()
            .status
            .success()
    }
}

#[test]
fn real_sops_roundtrip_drift_force_clean_and_permissions() {
    let repo = Repository::new();
    repo.init();
    let bytes = b"hello\0\xff\nprivate fixture";
    let live = repo.add("config", bytes);
    let before = fs::read(&live).unwrap();
    repo.ok(&["apply", "--dry-run"]);
    assert_eq!(fs::read(&live).unwrap(), before);
    repo.ok(&["clean"]);
    assert!(!live.exists());
    repo.ok(&["apply"]);
    assert_eq!(fs::read(&live).unwrap(), bytes);
    let modified = fs::metadata(&live).unwrap().modified().unwrap();
    repo.ok(&["apply"]);
    assert_eq!(fs::metadata(&live).unwrap().modified().unwrap(), modified);
    fs::write(&live, b"local edit").unwrap();
    assert!(!repo.run(&["apply"]).status.success());
    assert!(!repo.run(&["clean"]).status.success());
    assert_eq!(fs::read(&live).unwrap(), b"local edit");
    repo.ok(&["apply", "--force"]);
    assert_eq!(fs::read(&live).unwrap(), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&live).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(live.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn real_sops_rotation_rekeys_and_revocation_excludes_old_key() {
    let repo = Repository::new();
    repo.init();
    let recovery = repo.recovery();
    repo.add("one", b"unchanged content");
    repo.add("two", b"other unchanged content");
    let encrypted = repo.root.join("shared/ssh/one.enc");
    let before = fs::read(&encrypted).unwrap();
    repo.ok(&["rekey"]);
    assert_ne!(fs::read(&encrypted).unwrap(), before);
    assert!(repo.decrypts(&encrypted, &recovery));
    let old_identity = repo._temporary.path().join("old.txt");
    fs::copy(repo.identity(), &old_identity).unwrap();
    repo.ok(&["roll", "machine"]);
    assert!(repo.decrypts(&encrypted, &repo.identity()));
    assert!(!repo.decrypts(&encrypted, &old_identity));
    assert!(repo.decrypts(&encrypted, &recovery));
    repo.ok(&["revoke", "recovery"]);
    assert!(!repo.decrypts(&encrypted, &recovery));
    assert!(repo.decrypts(&encrypted, &repo.identity()));
    assert!(
        !repo
            .home
            .join(".config/dotfile/secret-transaction")
            .exists()
    );
}

#[test]
fn unreadable_ciphertext_aborts_recipient_changes_without_partial_writes() {
    let repo = Repository::new();
    repo.init();
    let _recovery = repo.recovery();
    repo.add("one", b"secret fixture");
    fs::write(
        repo.root.join("shared/ssh/broken.enc"),
        "invalid ciphertext",
    )
    .unwrap();
    repo.git(&["add", "--", "shared/ssh/broken.enc"]);
    let before = ["config/keys.dotfile", ".sops.yaml", "shared/ssh/one.enc"]
        .map(|p| fs::read(repo.root.join(p)).unwrap());
    let identity = fs::read(repo.identity()).unwrap();
    assert!(!repo.run(&["revoke", "recovery"]).status.success());
    assert!(!repo.run(&["roll", "machine"]).status.success());
    for (index, path) in ["config/keys.dotfile", ".sops.yaml", "shared/ssh/one.enc"]
        .iter()
        .enumerate()
    {
        assert_eq!(fs::read(repo.root.join(path)).unwrap(), before[index]);
    }
    assert_eq!(fs::read(repo.identity()).unwrap(), identity);
}

#[test]
fn staged_scan_checks_index_and_never_prints_token() {
    let repo = Repository::new();
    let token = format!("ghp_{}", "A".repeat(30));
    fs::write(repo.root.join("file.txt"), &token).unwrap();
    repo.git(&["add", "file.txt"]);
    fs::write(repo.root.join("file.txt"), "clean worktree").unwrap();
    let output = repo.run(&["scan", "--staged", "--no-canaries"]);
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("github-token"));
    assert!(!error.contains(&token));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("github-token"));
}

#[test]
fn encrypted_marker_does_not_hide_plaintext_token_and_fake_envelope_fails() {
    let repo = Repository::new();
    let token = format!("ghp_{}", "B".repeat(30));
    fs::write(
        repo.root.join("fake.enc"),
        format!("ENC[AES256_GCM\n{token}"),
    )
    .unwrap();
    repo.git(&["add", "fake.enc"]);
    let output = repo.run(&["scan", "--no-canaries"]);
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    assert!(error.contains("not-encrypted"));
    assert!(error.contains("github-token"));
    assert!(!error.contains(&token));
}

#[test]
fn historical_scan_handles_unusual_paths_and_same_blob_in_different_directories() {
    let repo = Repository::new();
    fs::create_dir_all(repo.root.join("private")).unwrap();
    fs::write(repo.root.join("private/.secret"), "").unwrap();
    fs::write(repo.root.join("ordinary.txt"), "same plain content").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "base"]);
    fs::write(
        repo.root.join("private/odd\nname.txt"),
        "same plain content",
    )
    .unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "new path"]);
    let output = repo.run(&["scan", "--commits", "HEAD", "--no-canaries"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("plaintext"));
}

#[test]
fn scanner_rejects_oversized_encrypted_blob_without_reading_its_contents() {
    let repo = Repository::new();
    fs::write(repo.root.join("large.enc"), vec![b'x'; 3 * 1024 * 1024]).unwrap();
    repo.git(&["add", "large.enc"]);
    let output = repo.run(&["scan", "--staged", "--no-canaries"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not-encrypted"));
}

#[test]
fn redaction_protocol_streams_without_exporting_private_values() {
    let repo = Repository::new();
    let secret = "violet-private-fixture-value";
    fs::write(
        repo.home.join(".config/dotfile/canaries"),
        format!("fixture={secret}\n"),
    )
    .unwrap();
    let mut child = repo
        .command()
        .arg("__redact")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let text = format!("this includes {secret}");
    let mut input = child.stdin.take().unwrap();
    serde_json::to_writer(&mut input, &text).unwrap();
    writeln!(input).unwrap();
    serde_json::to_writer(&mut input, "second input").unwrap();
    writeln!(input).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(secret));
    assert!(stdout.contains("[redacted:private]"));
    assert_eq!(stdout.lines().count(), 2);
}

#[test]
fn vars_templates_and_real_editor_roundtrip() {
    let repo = Repository::new();
    repo.init();
    let editor = repo._temporary.path().join("editor.sh");
    fs::write(
        &editor,
        "#!/bin/sh\nprintf 'host: fixture.private.example\\nopen:\\n  color: purple\\n' > \"$1\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = repo
        .command()
        .args(["edit", "vars"])
        .env("EDITOR", &editor)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::create_dir_all(repo.root.join("shared/demo")).unwrap();
    fs::write(
        repo.root.join("shared/demo/config.tmpl"),
        "Host {{ host }}\nColor {{ open.color }}\n",
    )
    .unwrap();
    repo.ok(&["apply"]);
    assert_eq!(
        fs::read_to_string(repo.home.join(".config/demo/config")).unwrap(),
        "Host fixture.private.example\nColor purple\n"
    );
    let output = repo.ok(&["vars"]);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("host"));
    assert!(!text.contains("fixture.private.example"));
    fs::write(repo.root.join("leak.txt"), "fixture.private.example").unwrap();
    repo.git(&["add", "leak.txt"]);
    let output = repo.run(&["scan", "--staged"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("canary"));
}

#[cfg(unix)]
#[test]
fn apply_preserves_symlink_target_even_with_force() {
    let repo = Repository::new();
    repo.init();
    let live = repo.add("config", b"original");
    fs::remove_file(&live).unwrap();
    let other = repo.home.join("other");
    fs::write(&other, b"must survive").unwrap();
    std::os::unix::fs::symlink(&other, &live).unwrap();
    assert!(!repo.run(&["apply", "--force"]).status.success());
    assert_eq!(fs::read(&other).unwrap(), b"must survive");
}

#[test]
fn interrupted_secret_transaction_restores_configuration_and_identity() {
    let repo = Repository::new();
    repo.init();
    let paths = [
        repo.root.join("config/keys.dotfile"),
        repo.root.join(".sops.yaml"),
        repo.identity(),
    ];
    let originals: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let journal = repo.home.join(".config/dotfile/secret-transaction");
    fs::create_dir(&journal).unwrap();
    let mut manifest = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        fs::write(journal.join(index.to_string()), &originals[index]).unwrap();
        manifest.push(serde_json::json!({"path":path,"existed":true,"mode":if index == 2 { 384 } else { 420 }}));
        fs::write(path, "interrupted replacement").unwrap();
    }
    fs::write(
        journal.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let output = repo.ok(&["sync"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("restored files"));
    for (index, path) in paths.iter().enumerate() {
        assert_eq!(fs::read(path).unwrap(), originals[index]);
    }
    assert!(!journal.exists());
    repo.ok(&["keys"]);
}

#[test]
fn recipient_commit_preflights_all_destination_types() {
    let repo = Repository::new();
    repo.init();
    let context = dotfile_cli::context::Context::new(
        repo.root.clone(),
        repo.home.clone(),
        repo.home.join(".config/dotfile"),
    )
    .unwrap();
    let first = repo.root.join("config/keys.dotfile");
    let original = fs::read(&first).unwrap();
    let blocker = repo.root.join("not-a-directory");
    fs::write(&blocker, b"block").unwrap();
    let result = dotfile_cli::secret::recipients::commit(
        &context,
        vec![
            (first.clone(), b"replacement".to_vec()),
            (blocker.join("file"), b"cannot install".to_vec()),
        ],
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&first).unwrap(), original);
    assert!(!context.state.join("secret-transaction").exists());
}

#[cfg(unix)]
#[test]
fn cancellation_stops_sops_child_group_promptly() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repository::new();
    fs::create_dir_all(repo.home.join(".config/dotfile/age")).unwrap();
    fs::write(repo.identity(), "test identity placeholder").unwrap();
    fs::create_dir_all(repo.root.join("shared/test")).unwrap();
    fs::write(
        repo.root.join("shared/test/file.enc"),
        "ciphertext placeholder",
    )
    .unwrap();
    let tools = repo._temporary.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let marker = repo._temporary.path().join("child.pid");
    let stub = tools.join("sops");
    fs::write(
        &stub,
        "#!/bin/sh\necho $$ > \"$DOTFILE_SECRET_CHILD_PID\"\nsleep 30\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap());
    let child = repo
        .command()
        .arg("status")
        .env("PATH", path)
        .env("DOTFILE_SECRET_CHILD_PID", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_cancelled_child_group(child, &marker);
}

#[cfg(unix)]
fn assert_cancelled_child_group(mut child: std::process::Child, marker: &Path) {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(3);
    let pid = loop {
        if let Some(pid) = fs::read_to_string(marker)
            .ok()
            .and_then(|text| text.trim().parse::<i32>().ok())
        {
            break pid;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("subprocess fixture did not start");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut status = None;
    while Instant::now() < deadline {
        status = child.try_wait().unwrap();
        if status.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(
        status.is_some_and(|s| !s.success()),
        "cancelled secret command did not stop promptly"
    );
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
}

#[cfg(unix)]
#[test]
fn cancellation_stops_git_batch_child_group_promptly() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repository::new();
    fs::write(repo.root.join("item"), "ordinary text").unwrap();
    repo.git(&["add", "."]);
    let tools = repo._temporary.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let marker = repo._temporary.path().join("git.pid");
    let stub = tools.join("git");
    fs::write(&stub, "#!/bin/sh\nfor arg do\n if [ \"$arg\" = cat-file ]; then\n  echo $$ > \"$DOTFILE_SECRET_CHILD_PID\"\n  sleep 30\n  exit 1\n fi\ndone\nPATH=\"$DOTFILE_SECRET_REAL_PATH\" exec git \"$@\"\n").unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let real_path = std::env::var("PATH").unwrap();
    let child = repo
        .command()
        .args(["scan", "--staged", "--no-canaries"])
        .env("PATH", format!("{}:{real_path}", tools.display()))
        .env("DOTFILE_SECRET_REAL_PATH", real_path)
        .env("DOTFILE_SECRET_CHILD_PID", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_cancelled_child_group(child, &marker);
}

#[test]
fn new_branch_pre_push_revision_selector_is_supported() {
    let repo = Repository::new();
    fs::write(repo.root.join("ordinary"), "public fixture").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "base"]);
    repo.ok(&["scan", "--commits", "HEAD --not --remotes", "--no-canaries"]);
}

#[test]
fn historical_marker_policy_comes_from_the_scanned_tree() {
    let repo = Repository::new();
    fs::create_dir_all(repo.root.join("private")).unwrap();
    fs::write(repo.root.join("private/.secret"), "").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "marker"]);
    fs::write(
        repo.root.join("private/leak.txt"),
        "private package plaintext",
    )
    .unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "leak"]);
    repo.git(&["rm", "private/.secret"]);
    repo.git(&["commit", "-qm", "remove marker"]);
    let output = repo.run(&["scan", "--commits", "HEAD~2..HEAD", "--no-canaries"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("plaintext"));
    fs::write(
        repo.root.join("private/public.txt"),
        "intentionally public after marker removal",
    )
    .unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "public file"]);
    repo.ok(&["scan", "--commits", "HEAD~1..HEAD", "--no-canaries"]);
}

#[cfg(unix)]
#[test]
fn type_changes_are_scanned_in_index_and_history() {
    let repo = Repository::new();
    fs::write(repo.root.join("item"), "ordinary").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-qm", "regular file"]);
    fs::remove_file(repo.root.join("item")).unwrap();
    let token = format!("ghp_{}", "Z".repeat(24));
    std::os::unix::fs::symlink(&token, repo.root.join("item")).unwrap();
    repo.git(&["add", "item"]);
    let staged = repo.run(&["scan", "--staged", "--no-canaries"]);
    assert!(!staged.status.success());
    assert!(String::from_utf8_lossy(&staged.stderr).contains("github-token"));
    repo.git(&["commit", "-qm", "type changed"]);
    let history = repo.run(&["scan", "--commits", "HEAD^..HEAD", "--no-canaries"]);
    assert!(!history.status.success());
    assert!(String::from_utf8_lossy(&history.stderr).contains("github-token"));
}

#[cfg(unix)]
#[test]
fn malformed_recovery_destinations_cannot_write_outside_repository() {
    let repo = Repository::new();
    repo.init();
    let outside = repo._temporary.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let victim = outside.join("victim");
    fs::write(&victim, b"must survive").unwrap();
    std::os::unix::fs::symlink(&outside, repo.root.join("escape")).unwrap();
    for destination in [
        repo.root.join("../outside/victim"),
        repo.root.join("escape/victim"),
    ] {
        let journal = repo.home.join(".config/dotfile/secret-transaction");
        fs::create_dir(&journal).unwrap();
        fs::write(journal.join("0"), b"malformed replacement").unwrap();
        let manifest = serde_json::json!([{"path":destination,"existed":true,"mode":420}]);
        fs::write(
            journal.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(!repo.run(&["sync"]).status.success());
        assert_eq!(fs::read(&victim).unwrap(), b"must survive");
        fs::remove_dir_all(journal).unwrap();
    }
}

#[test]
fn public_recipient_changes_need_no_private_identity_without_ciphertext() {
    let repo = Repository::new();
    let first = format!("age1{}", "q".repeat(58));
    let second = format!("age1{}", "p".repeat(58));
    repo.ok(&["enroll", "remote", &first]);
    repo.ok(&["rekey"]);
    repo.ok(&["roll", "remote", &second]);
    repo.ok(&["sync", "--rewrap"]);
    assert!(!repo.identity().exists());
    let policy = fs::read_to_string(repo.root.join(".sops.yaml")).unwrap();
    assert!(policy.contains(&second));
    assert!(!policy.contains(&first));
    let missing = repo._temporary.path().join("missing-key");
    let output = repo.run(&["rekey", "--using", missing.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no such identity file"));
}

#[test]
fn scanner_caps_report_memory_without_hiding_failure_or_total() {
    let repo = Repository::new();
    let token = format!("ghp_{}", "Z".repeat(24));
    fs::write(
        repo.root.join("many.txt"),
        format!("{token}\n").repeat(10_050),
    )
    .unwrap();
    repo.git(&["add", "many.txt"]);
    let output = repo.run(&["scan", "--all", "--no-canaries"]);
    assert!(!output.status.success());
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(report.contains("10050 findings"));
    assert!(report.contains("50 omitted by report size limit"));
    assert!(report.lines().count() < 10_020);
    assert!(!report.contains(&token));
}
