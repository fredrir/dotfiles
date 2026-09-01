use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use tempfile::TempDir;

struct Environment(Vec<(OsString, Option<OsString>)>);

impl Environment {
    fn set(values: &[(&str, OsString)]) -> Self {
        let previous = values
            .iter()
            .map(|(name, _)| (OsString::from(name), std::env::var_os(name)))
            .collect::<Vec<_>>();
        for (name, value) in values {
            unsafe { std::env::set_var(name, value) };
        }
        Self(previous)
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..).rev() {
            match value {
                Some(value) => unsafe { std::env::set_var(name, value) },
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

struct Sandbox {
    _temporary: TempDir,
    root: PathBuf,
    home: PathBuf,
    backend: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        let backend = temporary.path().join("dotfile-py");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("environment/test")).unwrap();
        fs::create_dir_all(root.join("shared/git")).unwrap();
        fs::create_dir_all(home.join(".config")).unwrap();
        fs::write(
            root.join("config/targets.dotfile"),
            "shared/git/.gitconfig = ~/.gitconfig\n",
        )
        .unwrap();
        fs::write(root.join("config/packages.dotfile"), "shared {\n  git\n}\n").unwrap();
        fs::write(root.join("PACKAGES.md"), "\n## `shared`\n\n- `git`\n").unwrap();
        fs::write(root.join("environment/test/manifest"), "shared\n").unwrap();
        fs::write(root.join("shared/git/.gitconfig"), "[user]\nname = Test\n").unwrap();
        fs::write(&backend, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&backend).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&backend, permissions).unwrap();
        }
        Self {
            _temporary: temporary,
            root,
            home,
            backend,
        }
    }

    fn command(&self, verbose: bool) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
        command
            .args(["sync", "test", "--dry-run"])
            .env("DOTFILE_ROOT", &self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("DOTFILE_PYTHON", &self.backend)
            .env("CI", "1")
            .env("TERM", "dumb")
            .env_remove("NO_COLOR");
        if verbose {
            command.arg("--verbose");
        }
        command.output().unwrap()
    }
}

fn lock_environment() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(base: &Path, path: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = fs::read_dir(path)
            .map(|entries| entries.flatten().collect::<Vec<_>>())
            .unwrap_or_default();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child = entry.path();
            let relative = child.strip_prefix(base).unwrap().to_path_buf();
            let metadata = fs::symlink_metadata(&child).unwrap();
            if metadata.is_dir() {
                found.insert(relative.clone(), b"directory".to_vec());
                collect(base, &child, found);
            } else if metadata.file_type().is_symlink() {
                found.insert(
                    relative,
                    fs::read_link(&child)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec(),
                );
            } else {
                found.insert(relative, fs::read(&child).unwrap());
            }
        }
    }
    let mut found = BTreeMap::new();
    collect(path, path, &mut found);
    found
}

#[test]
fn dry_run_is_read_only_quiet_by_default_and_detailed_only_when_verbose() {
    let _lock = lock_environment();
    let sandbox = Sandbox::new();
    let _environment = Environment::set(&[("DOTFILE_REEXECED", OsString::from("1"))]);
    let before = snapshot(sandbox._temporary.path());

    let compact = sandbox.command(false);

    assert!(
        compact.status.success(),
        "{}",
        String::from_utf8_lossy(&compact.stderr)
    );
    let compact_stdout = String::from_utf8_lossy(&compact.stdout);
    let compact_stderr = String::from_utf8_lossy(&compact.stderr);
    assert_eq!(compact_stdout.lines().count(), 1);
    assert!(compact_stdout.contains("changes pending"));
    assert!(!compact_stdout.contains(".gitconfig"));
    assert!(!compact_stdout.contains("linking profile"));
    assert!(!compact_stdout.contains("\u{1b}["));
    assert!(compact_stderr.is_empty(), "{compact_stderr}");
    assert_eq!(snapshot(sandbox._temporary.path()), before);

    let verbose = sandbox.command(true);

    assert!(
        verbose.status.success(),
        "{}",
        String::from_utf8_lossy(&verbose.stderr)
    );
    let verbose_stdout = String::from_utf8_lossy(&verbose.stdout);
    assert_eq!(verbose_stdout.matches(".gitconfig").count(), 1);
    assert!(verbose_stdout.contains("link ~/.gitconfig"));
    assert_eq!(snapshot(sandbox._temporary.path()), before);
}

#[cfg(unix)]
#[test]
fn warm_verbose_sync_still_lists_current_managed_links() {
    use std::os::unix::fs::symlink;

    let _lock = lock_environment();
    let sandbox = Sandbox::new();
    let _environment = Environment::set(&[("DOTFILE_REEXECED", OsString::from("1"))]);
    symlink(
        sandbox.root.join("shared/git/.gitconfig"),
        sandbox.home.join(".gitconfig"),
    )
    .unwrap();
    let before = snapshot(sandbox._temporary.path());

    let verbose = sandbox.command(true);

    assert!(
        verbose.status.success(),
        "{}",
        String::from_utf8_lossy(&verbose.stderr)
    );
    let stdout = String::from_utf8_lossy(&verbose.stdout);
    assert_eq!(stdout.matches("link ~/.gitconfig").count(), 1);
    assert_eq!(snapshot(sandbox._temporary.path()), before);
}
