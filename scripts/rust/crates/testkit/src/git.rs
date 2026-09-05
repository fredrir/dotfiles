use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use crate::Bin;

pub struct GitSandbox {
    home: TempDir,
}

impl GitSandbox {
    pub fn plain() -> GitSandbox {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("gitconfig"), "").unwrap();
        fs::create_dir(home.path().join("work")).unwrap();
        GitSandbox { home }
    }

    pub fn committed() -> GitSandbox {
        let sandbox = GitSandbox::plain();
        let work = sandbox.work();
        sandbox.git(&work, &["init", "--quiet", "."]);
        sandbox.write("mod.txt", "one\ntwo\n");
        sandbox.write("del.txt", "gone\n");
        sandbox.write("keep.txt", "keep\n");
        sandbox.write(".gitignore", "ignored.log\nignored_dir/\n");
        sandbox.git(&work, &["add", "-A"]);
        sandbox.git(&work, &["commit", "--quiet", "-m", "init"]);
        sandbox
    }

    pub fn with_origin() -> GitSandbox {
        let sandbox = GitSandbox::plain();
        let work = sandbox.work();
        let origin = sandbox.origin();
        sandbox.git(sandbox.home(), &["init", "--quiet", "--bare", "origin.git"]);
        sandbox.git(&work, &["init", "--quiet", "."]);
        sandbox.write("seed", "seed\n");
        sandbox.git(&work, &["add", "."]);
        sandbox.git(&work, &["commit", "--quiet", "-m", "seed"]);
        sandbox.git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        sandbox.git(&work, &["push", "--quiet", "-u", "origin", "HEAD"]);
        sandbox
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn work(&self) -> PathBuf {
        self.home().join("work")
    }

    pub fn origin(&self) -> PathBuf {
        self.home().join("origin.git")
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.work().join(relative)
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.path(relative).symlink_metadata().is_ok()
    }

    pub fn write(&self, relative: &str, contents: &str) {
        self.write_in(&self.work(), relative, contents);
    }

    pub fn write_in(&self, root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    pub fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path(relative)).expect("the file is here")
    }

    pub fn command(&self, program: impl AsRef<OsStr>, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command.current_dir(cwd).envs(self.environment());
        command
    }

    pub fn bin(&self, program: impl AsRef<OsStr>, cwd: &Path) -> Bin {
        Bin::new(program).current_dir(cwd).envs(self.environment())
    }

    pub fn git(&self, cwd: &Path, arguments: &[&str]) -> String {
        let output = self
            .command("git", cwd)
            .args(["-c", "init.defaultBranch=main"])
            .args(arguments)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    }

    pub fn status(&self) -> String {
        self.git(&self.work(), &["status", "--porcelain"])
    }

    fn environment(&self) -> [(&'static str, OsString); 9] {
        [
            ("HOME", self.home().into()),
            ("GIT_CONFIG_GLOBAL", self.home().join("gitconfig").into()),
            ("GIT_CONFIG_SYSTEM", "/dev/null".into()),
            ("GIT_TERMINAL_PROMPT", "0".into()),
            ("GIT_AUTHOR_NAME", "test".into()),
            ("GIT_AUTHOR_EMAIL", "test@example.invalid".into()),
            ("GIT_COMMITTER_NAME", "test".into()),
            ("GIT_COMMITTER_EMAIL", "test@example.invalid".into()),
            ("NO_COLOR", "1".into()),
        ]
    }
}

#[cfg(test)]
#[path = "../tests/unit/git_tests.rs"]
mod tests;
