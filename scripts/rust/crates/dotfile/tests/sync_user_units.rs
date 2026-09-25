#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use testkit::{Bin, Ran, TempDir, executable, tree_pairs};

const SYSTEMCTL: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
[ "$2" = show ] || exit 0
shift 4
for unit in "$@"; do
  case " $STALE " in *" $unit "*) stale=yes ;; *) stale=no ;; esac
  case " $RUNNING " in *" $unit "*) state=active ;; *) state=inactive ;; esac
  printf 'NeedDaemonReload=%s\nId=%s\nActiveState=%s\n\n' "$stale" "$unit" "$state"
done
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
                "shared/units = ~/.config/systemd/user\n",
            ),
            ("repo/config/packages.dotfile", "shared {\n  git\n}\n"),
            ("repo/PACKAGES.md", "\n## `shared`\n\n- `git`\n"),
            ("repo/environment/test/manifest", "shared\n"),
            (
                "repo/shared/units/app.service",
                "[Service]\nExecStart=/bin/true\n",
            ),
            (
                "repo/shared/units/idle.service",
                "[Service]\nExecStart=/bin/true\n",
            ),
            (
                "repo/shared/units/fresh.service",
                "[Service]\nExecStart=/bin/true\n",
            ),
            ("home/.config/systemd/user/local.service", "[Service]\n"),
            ("bin/", ""),
        ]);
        let root = temporary.path().join("repo");
        let home = temporary.path().join("home");
        executable(&temporary.path().join("bin/systemctl"), SYSTEMCTL);
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

    fn sync(&self, arguments: &[&str], stale: &str, running: &str) -> Ran {
        Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .args(["sync", "test"])
            .args(arguments)
            .env("DOTFILE_ROOT", &self.root)
            .env("DOTFILE_REEXECED", "1")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin",
                    self.temporary.path().join("bin").display()
                ),
            )
            .env("SYSTEMCTL_LOG", self.log())
            .env("STALE", stale)
            .env("RUNNING", running)
            .env("CI", "1")
            .env("TERM", "dumb")
            .run()
    }

    fn log(&self) -> PathBuf {
        self.temporary.path().join("systemctl.log")
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn stale_linked_units_are_reloaded_and_only_running_ones_restarted() {
    let sandbox = Sandbox::new();

    let ran = sandbox.sync(
        &[],
        "app.service idle.service local.service",
        "app.service fresh.service local.service",
    );

    assert!(ran.success(), "{}\n{}", ran.stdout, ran.stderr);
    assert_eq!(
        sandbox.calls(),
        [
            "--user show --property=Id,NeedDaemonReload,ActiveState -- app.service fresh.service idle.service",
            "--user daemon-reload",
            "--user try-restart -- app.service",
        ]
    );
}

#[test]
fn current_units_are_left_alone() {
    let sandbox = Sandbox::new();

    let ran = sandbox.sync(&[], "", "app.service");

    assert!(ran.success(), "{}\n{}", ran.stdout, ran.stderr);
    assert_eq!(sandbox.calls().len(), 1);
}

#[test]
fn dry_run_only_inspects_units() {
    let sandbox = Sandbox::new();
    let user = sandbox.home.join(".config/systemd/user");
    for unit in ["app.service", "idle.service", "fresh.service"] {
        std::os::unix::fs::symlink(
            sandbox.root.join("shared/units").join(unit),
            user.join(unit),
        )
        .unwrap();
    }

    let ran = sandbox.sync(&["--dry-run"], "app.service", "app.service");

    assert!(ran.success(), "{}\n{}", ran.stdout, ran.stderr);
    let calls = sandbox.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].starts_with("--user show "), "{calls:?}");
}
