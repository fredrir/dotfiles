use crate::context::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Approval {
    pub path: PathBuf,
    pub sha256: String,
    pub labels: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    approvals: Vec<Approval>,
}

#[derive(Debug, Default)]
pub(super) struct Store {
    entries: BTreeMap<PathBuf, BTreeMap<String, BTreeSet<String>>>,
}

impl Store {
    pub fn load(context: &Context) -> Result<Self, String> {
        Self::read(&storage_path(context)?)
    }

    pub fn contains(&self, path: &Path, sha256: &str, label: &str) -> bool {
        self.entries
            .get(path)
            .and_then(|contents| contents.get(sha256))
            .is_some_and(|labels| labels.contains(label))
    }

    pub fn save(
        &self,
        context: &Context,
        pending: &[Approval],
        validate_snapshot: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if pending.is_empty() {
            return Ok(());
        }
        for approval in pending {
            validate(approval)?;
        }
        let path = storage_path(context)?;
        let parent = path.parent().ok_or("scan approval path has no parent")?;
        prepare_directory(parent)?;
        let _lock = lock(&parent.join("scan-approvals.lock"))?;
        // Reload after locking: another reviewed scan may have saved meanwhile.
        let mut current = Self::read(&path)?;
        for approval in pending {
            current.insert(approval);
        }
        let approvals = current
            .entries
            .into_iter()
            .flat_map(|(path, contents)| {
                contents.into_iter().map(move |(sha256, labels)| Approval {
                    path: path.clone(),
                    sha256,
                    labels,
                })
            })
            .collect();
        let mut bytes = serde_json::to_vec_pretty(&Document {
            version: 1,
            approvals,
        })
        .map_err(|e| format!("encode scan approvals: {e}"))?;
        bytes.push(b'\n');
        if bytes.len() > LIMIT {
            return Err("scan approvals exceed 2 MiB".into());
        }
        validate_snapshot()?;
        crate::fs::write_private(&path, &bytes).map(|_| ())
    }

    fn read(path: &Path) -> Result<Self, String> {
        let parent = path.parent().ok_or("scan approval path has no parent")?;
        if !directory_exists(parent)? {
            return Ok(Self::default());
        }
        let mut options = OpenOptions::new();
        options.read(true);
        nofollow(&mut options);
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A dangling symlink must not be treated as an absent store.
                if fs::symlink_metadata(path).is_ok() {
                    return Err("scan approvals must be a regular file".into());
                }
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("read scan approvals: {error}")),
        };
        let metadata = file
            .metadata()
            .map_err(|e| format!("inspect scan approvals: {e}"))?;
        if !metadata.is_file()
            || fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err("scan approvals must be a regular file".into());
        }
        if metadata.len() > LIMIT as u64 {
            return Err("scan approvals exceed 2 MiB".into());
        }
        let mut bytes = Vec::new();
        file.take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("read scan approvals: {e}"))?;
        if bytes.len() > LIMIT {
            return Err("scan approvals exceed 2 MiB".into());
        }
        let document: Document =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid scan approvals: {e}"))?;
        if document.version != 1 {
            return Err("unsupported scan approval version".into());
        }
        let mut store = Self::default();
        for approval in document.approvals {
            validate(&approval)?;
            store.insert(&approval);
        }
        Ok(store)
    }

    fn insert(&mut self, approval: &Approval) {
        self.entries
            .entry(approval.path.clone())
            .or_default()
            .entry(approval.sha256.to_ascii_lowercase())
            .or_default()
            .extend(approval.labels.iter().cloned());
    }
}

fn validate(approval: &Approval) -> Result<(), String> {
    if approval.path.as_os_str().is_empty()
        || approval.path.to_str().is_none()
        || approval
            .path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("scan approval path must be a relative UTF-8 path without traversal".into());
    }
    if approval.sha256.len() != 64 || !approval.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("scan approval needs a 64-digit SHA-256 hash".into());
    }
    if approval.labels.is_empty()
        || approval.labels.iter().any(|label| {
            label != "value"
                && !crate::secret::patterns::TOKENS
                    .iter()
                    .any(|(known, _)| label == known)
        })
    {
        return Err("scan approvals support known pattern labels only".into());
    }
    Ok(())
}

fn storage_path(context: &Context) -> Result<PathBuf, String> {
    // --git-path resolves existing symlinks; retain a literal path under Git's
    // own directory so neither the store nor its parent can escape through one.
    let git = super::git(context, &["rev-parse", "--absolute-git-dir"])?;
    let git = super::path_from_bytes(git.strip_suffix(b"\n").unwrap_or(&git));
    if !git.is_absolute() {
        return Err("Git returned an invalid scan approval directory".into());
    }
    Ok(git.join("dotfile/scan-approvals.json"))
}

fn directory_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err("scan approval directory must not be a symlink or non-directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("inspect scan approval directory: {error}")),
    }
}

fn prepare_directory(path: &Path) -> Result<(), String> {
    if !directory_exists(path)? {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                directory_exists(path)?;
            }
            Err(error) => return Err(format!("create scan approval directory: {error}")),
        }
    }
    #[cfg(unix)]
    crate::secret::vault::set_mode(path, 0o700)?;
    Ok(())
}

fn nofollow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    let _ = options;
}

#[cfg(unix)]
fn lock(path: &Path) -> Result<nix::fcntl::Flock<File>, String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::time::{Duration, Instant};
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600);
    nofollow(&mut options);
    let mut file = options
        .open(path)
        .map_err(|e| format!("lock scan approvals: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("scan approval lock must be a regular file".into());
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        crate::cancel::check()?;
        match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(lock) => return Ok(lock),
            Err((returned, error)) if error == nix::errno::Errno::EWOULDBLOCK => {
                file = returned;
                if Instant::now() >= deadline {
                    return Err("scan approval storage is busy; try again".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err((_, error)) => return Err(format!("lock scan approvals: {error}")),
        }
    }
}

#[cfg(not(unix))]
fn lock(_path: &Path) -> Result<File, String> {
    Err("scan approval saving requires Unix file locking".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> (tempfile::TempDir, Context) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repository");
        let output = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .output()
            .unwrap();
        assert!(output.status.success());
        let home = temporary.path().join("home");
        let context = Context::new(
            root.clone(),
            home.clone(),
            root.join("config"),
            home.join(".config"),
        )
        .unwrap();
        (temporary, context)
    }

    fn approval(path: &str, label: &str) -> Approval {
        Approval {
            path: path.into(),
            sha256: "a".repeat(64),
            labels: BTreeSet::from([label.into()]),
        }
    }

    #[test]
    fn absent_store_load_does_not_create_state() {
        let (_temporary, context) = repository();
        assert!(!Store::load(&context).unwrap().contains(
            Path::new("fixture"),
            &"a".repeat(64),
            "value"
        ));
        assert!(!context.root.join(".git/dotfile").exists());
        assert!(!context.root_config.exists());
    }

    #[cfg(unix)]
    #[test]
    fn saves_exact_content_and_merges_independent_reviewers() {
        use std::os::unix::fs::PermissionsExt;
        let (_temporary, context) = repository();
        fs::write(context.root.join("staged"), "unchanged index").unwrap();
        super::super::git(&context, &["add", "staged"]).unwrap();
        let index = fs::read(context.root.join(".git/index")).unwrap();
        let stale = Store::load(&context).unwrap();
        stale
            .save(
                &context,
                &[approval("some path/fixture\nname", "value")],
                || Ok(()),
            )
            .unwrap();
        stale
            .save(
                &context,
                &[approval("some path/fixture\nname", "github-token")],
                || Ok(()),
            )
            .unwrap();
        let store = Store::load(&context).unwrap();
        let path = Path::new("some path/fixture\nname");
        assert!(store.contains(path, &"a".repeat(64), "value"));
        assert!(store.contains(path, &"a".repeat(64), "github-token"));
        assert!(!store.contains(path, &"b".repeat(64), "value"));
        assert!(!store.contains(Path::new("other"), &"a".repeat(64), "value"));
        assert!(!store.contains(path, &"a".repeat(64), "api-key"));
        assert_eq!(
            fs::metadata(storage_path(&context).unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(fs::read(context.root.join(".git/index")).unwrap(), index);
        assert!(!context.root_config.join("scan.dotfile").exists());
    }

    #[cfg(unix)]
    #[test]
    fn linked_worktrees_use_their_own_git_directory() {
        let (temporary, context) = repository();
        super::super::git(
            &context,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        )
        .unwrap();
        let worktree = temporary.path().join("linked worktree");
        super::super::git(
            &context,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "linked",
                worktree.to_str().unwrap(),
            ],
        )
        .unwrap();
        let linked = Context::new(
            worktree,
            context.home.clone(),
            context.root_config.clone(),
            context.external_config.clone(),
        )
        .unwrap();
        assert!(linked.root.join(".git").is_file());
        Store::default()
            .save(&linked, &[approval("fixture", "value")], || Ok(()))
            .unwrap();
        assert!(Store::load(&linked).unwrap().contains(
            Path::new("fixture"),
            &"a".repeat(64),
            "value"
        ));
        assert!(!Store::load(&context).unwrap().contains(
            Path::new("fixture"),
            &"a".repeat(64),
            "value"
        ));
        assert!(!context.root.join(".git/dotfile").exists());
    }

    #[test]
    fn malformed_or_non_pattern_approvals_fail_closed() {
        let (_temporary, context) = repository();
        let path = storage_path(&context).unwrap();
        fs::create_dir(path.parent().unwrap()).unwrap();
        let valid = approval("fixture", "value");
        for invalid in [
            serde_json::json!({"version":2,"approvals":[valid]}),
            serde_json::json!({"version":1,"approvals":[],"unknown":true}),
            serde_json::json!({"version":1,"approvals":[approval("../escape", "value")]}),
            serde_json::json!({"version":1,"approvals":[approval("/absolute", "value")]}),
            serde_json::json!({"version":1,"approvals":[approval("fixture", "plaintext")]}),
            serde_json::json!({"version":1,"approvals":[approval("fixture", "canary")]}),
            serde_json::json!({"version":1,"approvals":[{"path":"fixture","sha256":"bad","labels":["value"]}]}),
        ] {
            fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(Store::load(&context).is_err());
        }
        fs::write(&path, vec![b' '; LIMIT + 1]).unwrap();
        assert!(Store::load(&context).unwrap_err().contains("2 MiB"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_storage_and_lock_are_rejected_without_touching_target() {
        let (temporary, context) = repository();
        let path = storage_path(&context).unwrap();
        fs::create_dir(path.parent().unwrap()).unwrap();
        let target = temporary.path().join("target");
        fs::write(&target, b"untouched").unwrap();
        for name in [&path, &path.with_file_name("scan-approvals.lock")] {
            if name.exists() {
                fs::remove_file(name).unwrap();
            }
            std::os::unix::fs::symlink(&target, name).unwrap();
            assert!(
                Store::default()
                    .save(&context, &[approval("fixture", "value")], || Ok(()))
                    .is_err()
            );
            fs::remove_file(name).unwrap();
            assert_eq!(fs::read(&target).unwrap(), b"untouched");
        }
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(temporary.path(), path.parent().unwrap()).unwrap();
        assert!(Store::load(&context).is_err());
        assert!(
            Store::default()
                .save(&context, &[approval("fixture", "value")], || Ok(()))
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_saves_preserve_both_approvals() {
        let (_temporary, context) = repository();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for name in ["first", "second"] {
                let barrier = barrier.clone();
                let context = &context;
                scope.spawn(move || {
                    let store = Store::load(context).unwrap();
                    barrier.wait();
                    store
                        .save(context, &[approval(name, "value")], || Ok(()))
                        .unwrap();
                });
            }
        });
        let store = Store::load(&context).unwrap();
        for name in ["first", "second"] {
            assert!(store.contains(Path::new(name), &"a".repeat(64), "value"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn validation_after_lock_wait_rejects_a_changed_git_index_before_writing() {
        let (_temporary, context) = repository();
        let staged = context.root.join("staged");
        fs::write(&staged, "reviewed bytes").unwrap();
        super::super::git(&context, &["add", "staged"]).unwrap();
        let snapshot = super::super::git(&context, &["ls-files", "--stage", "-z"]).unwrap();
        Store::default()
            .save(&context, &[approval("existing", "value")], || Ok(()))
            .unwrap();
        let path = storage_path(&context).unwrap();
        let original = fs::read(&path).unwrap();
        let held = lock(&path.with_file_name("scan-approvals.lock")).unwrap();
        let (called, observed) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let save = scope.spawn(|| {
                Store::default().save(&context, &[approval("staged", "value")], || {
                    called.send(()).unwrap();
                    if super::super::git(&context, &["ls-files", "--stage", "-z"])? == snapshot {
                        Ok(())
                    } else {
                        Err("staged files changed during review".into())
                    }
                })
            });
            assert!(
                observed
                    .recv_timeout(std::time::Duration::from_millis(50))
                    .is_err()
            );
            fs::write(&staged, "new unreviewed bytes").unwrap();
            super::super::git(&context, &["add", "staged"]).unwrap();
            drop(held);
            assert!(
                save.join()
                    .unwrap()
                    .unwrap_err()
                    .contains("staged files changed")
            );
        });
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!Store::load(&context).unwrap().contains(
            Path::new("staged"),
            &"a".repeat(64),
            "value"
        ));
    }
}
