use super::*;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn context(root: &Path) -> Context {
    Context::new(root.join("repo"), root.join("home"), root.join("state")).unwrap()
}
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for path in ["repo/config", "repo/shared", "home"] {
        fs::create_dir_all(temp.path().join(path)).unwrap();
    }
    fs::write(temp.path().join("repo/config/item"), "original\n").unwrap();
    fs::write(temp.path().join("home/source"), "adopted\n").unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(temp.path().join("repo"))
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(temp.path().join("repo"))
            .args(["add", "config/item"])
            .status()
            .unwrap()
            .success()
    );
    temp
}
fn crash(root: &Path, mode: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fs::transaction::tests::crash_child",
            "--nocapture",
        ])
        .env("DOTFILE_TRANSACTION_CHILD", root)
        .env("DOTFILE_TRANSACTION_MODE", mode)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !root.join("ready").exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "transaction child timed out"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "transaction child exited before checkpoint"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
}
#[test]
fn crash_child() {
    let Some(root) = std::env::var_os("DOTFILE_TRANSACTION_CHILD").map(PathBuf::from) else {
        return;
    };
    let context = context(&root);
    let mut transaction = Transaction::new(&context).unwrap();
    match std::env::var("DOTFILE_TRANSACTION_MODE").unwrap().as_str() {
        "mutated" => {
            transaction
                .write(&context.root.join("config/item"), b"replacement\n")
                .unwrap();
            transaction
                .move_node(
                    &context.home.join("source"),
                    &context.root.join("shared/adopted"),
                )
                .unwrap();
            transaction
                .symlink(
                    &context.root.join("shared/adopted"),
                    &context.home.join("source"),
                )
                .unwrap();
            transaction
                .stage(
                    &context,
                    &[
                        PathBuf::from("config/item"),
                        PathBuf::from("shared/adopted"),
                    ],
                )
                .unwrap();
        }
        "intent" => {
            let staging = transaction
                .staging(&fs::canonicalize(&context.root).unwrap())
                .unwrap();
            let staged = staging.join("file");
            fs::write(&staged, "mine\n").unwrap();
            fs::File::open(&staged).unwrap().sync_all().unwrap();
            transaction.journal.undo.push(Undo {
                source: normalized(&context.root.join("config/new")).unwrap(),
                destination: staged.clone(),
                identity: Identity::read(&staged).unwrap(),
                empty_directory: false,
                tree_digest: None,
                original: None,
            });
            save(&transaction.directory, &transaction.journal).unwrap();
        }
        "committed" => {
            transaction
                .write(&context.root.join("config/item"), b"committed\n")
                .unwrap();
            transaction.journal.committed = true;
            save(&transaction.directory, &transaction.journal).unwrap();
        }
        other => panic!("invalid crash mode {other}"),
    }
    fs::write(root.join("ready"), "ready").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
#[test]
fn killed_transaction_recovers_files_links_git_index_and_original_timestamps() {
    let fixture = fixture();
    let context = context(fixture.path());
    let index = fs::read(context.root.join(".git/index")).unwrap();
    let modified = fs::metadata(context.root.join("config/item"))
        .unwrap()
        .modified()
        .unwrap();
    crash(fixture.path(), "mutated");
    assert_eq!(
        fs::read(context.root.join("config/item")).unwrap(),
        b"replacement\n"
    );
    recover(&context).unwrap();
    recover(&context).unwrap();
    assert_eq!(
        fs::read(context.root.join("config/item")).unwrap(),
        b"original\n"
    );
    assert_eq!(
        fs::metadata(context.root.join("config/item"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert_eq!(fs::read(context.home.join("source")).unwrap(), b"adopted\n");
    assert!(
        !fs::symlink_metadata(context.home.join("source"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!context.root.join("shared/adopted").exists());
    assert_eq!(fs::read(context.root.join(".git/index")).unwrap(), index);
    assert!(!fs::read_dir(&context.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(PREFIX)
    }));
}
#[test]
fn unapplied_intent_preserves_unrelated_occupants_and_recovers_idempotently() {
    let fixture = fixture();
    let context = context(fixture.path());
    crash(fixture.path(), "intent");
    let occupant = context.root.join("config/new");
    fs::write(&occupant, "unrelated\n").unwrap();
    assert!(
        recover(&context)
            .unwrap_err()
            .contains("ambiguous occupants")
    );
    assert_eq!(fs::read(&occupant).unwrap(), b"unrelated\n");
    fs::remove_file(occupant).unwrap();
    recover(&context).unwrap();
    recover(&context).unwrap();
}
#[test]
fn durable_commit_marker_prevents_rollback_after_a_crash_during_cleanup() {
    let fixture = fixture();
    let context = context(fixture.path());
    crash(fixture.path(), "committed");
    recover(&context).unwrap();
    assert_eq!(
        fs::read(context.root.join("config/item")).unwrap(),
        b"committed\n"
    );
}
#[test]
fn rollback_refuses_symlink_replacement_and_retains_original_bytes() {
    let fixture = fixture();
    let context = context(fixture.path());
    let path = context.root.join("config/item");
    let mut transaction = Transaction::new(&context).unwrap();
    transaction.write(&path, b"replacement\n").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&context.home.join("source"), &path).unwrap();
    let journal = transaction.directory.clone();
    drop(transaction);
    assert_eq!(fs::read(context.home.join("source")).unwrap(), b"adopted\n");
    assert!(journal.exists());
    assert!(recover(&context).is_err());
}

#[test]
fn atomic_replacements_never_expose_a_missing_or_partial_file() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = fixture();
    let context = context(fixture.path());
    let path = context.root.join("config/item");
    let mut transaction = Transaction::new(&context).unwrap();
    let observing = AtomicBool::new(true);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            while observing.load(Ordering::Acquire) {
                let bytes = fs::read(&path).expect("replacement must always remain readable");
                assert!(bytes == b"original\n" || bytes == b"next\n" || bytes == b"last\n");
            }
        });
        let result = (|| {
            for _ in 0..8 {
                transaction.write(&path, b"next\n")?;
                transaction.write(&path, b"last\n")?;
            }
            Ok::<(), String>(())
        })();
        observing.store(false, Ordering::Release);
        reader.join().unwrap();
        result.unwrap();
    });
    drop(transaction);
    assert_eq!(fs::read(path).unwrap(), b"original\n");
}

#[test]
fn tree_digest_frames_file_contents_and_symlink_targets() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    // Without framing the file length, moving a following sibling's entire header
    // across a content boundary could yield the same serialized hash input.
    let mut header = 1_u64.to_le_bytes().to_vec();
    header.extend_from_slice(b"b");
    header.push(3);
    #[cfg(unix)]
    header.extend_from_slice(&0o100644_u32.to_le_bytes());
    fs::write(first.path().join("a"), []).unwrap();
    fs::write(first.path().join("b"), &header).unwrap();
    fs::write(second.path().join("a"), &header).unwrap();
    fs::write(second.path().join("b"), []).unwrap();
    assert_ne!(
        tree_digest(first.path()).unwrap(),
        tree_digest(second.path()).unwrap()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn cross_filesystem_recovery_preserves_edits_until_the_copy_is_restored() {
    let fixture = fixture();
    let context = context(fixture.path());
    let external = tempfile::tempdir_in("/dev/shm").unwrap();
    if Identity::read(external.path()).unwrap().device
        == Identity::read(&context.root).unwrap().device
    {
        return;
    }
    let original = context.root.join("shared/directory");
    fs::create_dir(&original).unwrap();
    fs::write(original.join("value"), "original\n").unwrap();
    let destination = external.path().join("copied");
    let mut transaction = Transaction::new(&context).unwrap();
    transaction.move_node(&original, &destination).unwrap();
    fs::write(destination.join("value"), "new user edit\n").unwrap();
    drop(transaction);
    assert_eq!(
        fs::read(destination.join("value")).unwrap(),
        b"new user edit\n"
    );
    assert!(
        recover(&context)
            .unwrap_err()
            .contains("changed cross-filesystem contents")
    );
    fs::write(destination.join("value"), "original\n").unwrap();
    recover(&context).unwrap();
    assert_eq!(fs::read(original.join("value")).unwrap(), b"original\n");
    assert!(!destination.exists());
}
