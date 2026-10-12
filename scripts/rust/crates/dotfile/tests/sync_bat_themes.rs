#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use testkit::{Bin, Ran, TempDir, executable, tree_pairs};

const BAT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$BAT_LOG"
while [ $# -gt 0 ]; do
  [ "$1" = --target ] && target=$2
  shift
done
mkdir -p "$target" && : > "$target/themes.bin"
"#;

struct Sandbox {
    temporary: TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let temporary = tree_pairs(&[
            (
                "repo/config/targets.dotfile",
                "shared/bat = ~/.config/bat\n",
            ),
            ("repo/environment/test/manifest", "shared\n"),
            ("repo/shared/bat/themes/Test.tmTheme", "<plist/>\n"),
            ("home/", ""),
            ("bin/", ""),
        ]);
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        executable(&temporary.path().join("bin/bat"), BAT);
        let docs = Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .args(["docs", "--only", "keybinds"])
            .env("DOTFILE_ROOT", &root)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .run();
        assert!(docs.success(), "{}", docs.stderr);
        Self {
            temporary,
            root,
            home,
        }
    }

    fn sync(&self, arguments: &[&str]) -> Ran {
        Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .args(["sync", "test"])
            .args(arguments)
            .env("DOTFILE_ROOT", &self.root)
            .env("DOTFILE_REEXECED", "1")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.cache())
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin",
                    self.temporary.path().join("bin").display()
                ),
            )
            .env("BAT_LOG", self.log())
            .env("CI", "1")
            .env("TERM", "dumb")
            .run()
    }

    fn cache(&self) -> PathBuf {
        self.temporary.path().join("cache")
    }

    fn log(&self) -> PathBuf {
        self.temporary.path().join("bat.log")
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn build(&self) -> String {
        format!(
            "cache --build --source {} --target {}",
            self.root.join("shared/bat").display(),
            self.cache().join("bat").display()
        )
    }
}

#[test]
fn the_bat_cache_is_built_once_and_again_when_a_theme_changes() {
    let sandbox = Sandbox::new();

    let first = sandbox.sync(&[]);
    assert!(first.success(), "{}\n{}", first.stdout, first.stderr);
    assert_eq!(sandbox.calls(), [sandbox.build()]);

    let unchanged = sandbox.sync(&[]);
    assert!(unchanged.success(), "{}", unchanged.stderr);
    assert_eq!(sandbox.calls(), [sandbox.build()]);

    File::options()
        .write(true)
        .open(sandbox.root.join("shared/bat/themes/Test.tmTheme"))
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let edited = sandbox.sync(&[]);
    assert!(edited.success(), "{}", edited.stderr);
    assert_eq!(sandbox.calls(), [sandbox.build(), sandbox.build()]);
}

#[test]
fn a_dry_run_leaves_the_bat_cache_alone() {
    let sandbox = Sandbox::new();

    let ran = sandbox.sync(&["--dry-run"]);

    assert!(ran.success(), "{}\n{}", ran.stdout, ran.stderr);
    assert!(sandbox.calls().is_empty());
    assert!(!sandbox.cache().join("bat/themes.bin").exists());
}
