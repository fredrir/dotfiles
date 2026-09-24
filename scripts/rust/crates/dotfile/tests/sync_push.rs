#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use testkit::{Bin, Ran, TempDir, executable};

/// A local machine, a peer reached through a stand-in `ssh`, and a bare origin.
/// The peer's origin URL leads nowhere, so a pull from it fails the sync.
struct Machines {
    temporary: TempDir,
}

impl Machines {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let machines = Self { temporary };
        fs::write(machines.path("gitconfig"), "").unwrap();
        for directory in ["stubs", "peer-bin", "local/.config", "peer/.config"] {
            fs::create_dir_all(machines.path(directory)).unwrap();
        }
        executable(
            &machines.path("stubs/ssh"),
            "#!/bin/sh\n\
             while [ $# -gt 0 ]; do case \"$1\" in\n\
               -o) shift 2 ;;\n\
               --) shift; break ;;\n\
               -*) shift ;;\n\
               *) break ;;\n\
             esac; done\n\
             shift\n\
             cd \"$PEER_HOME\" && HOME=\"$PEER_HOME\" XDG_CONFIG_HOME=\"$PEER_HOME/.config\" exec sh -c \"$*\"\n",
        );
        executable(
            &machines.path("peer-bin/dotfile"),
            &format!(
                "#!/bin/sh\n\
                 [ \"$2\" = --wire-probe ] && exit 0\n\
                 DOTFILE_ROOT=\"$HOME/dotfiles\" exec '{}' \"$@\"\n",
                env!("CARGO_BIN_EXE_dotfile")
            ),
        );
        machines.git(
            &machines.path(""),
            &["init", "--quiet", "--bare", "origin.git"],
        );
        let local = machines.local();
        for (path, content) in [
            (
                "config/targets.dotfile",
                "shared/git/.gitconfig = ~/.gitconfig\n",
            ),
            ("config/packages.dotfile", "shared {\n  git\n}\n"),
            (
                "config/hosts.dotfile",
                "macie {\n  hostnames = macie\n}\n\narchie {\n  hostnames = archie\n}\n",
            ),
            ("PACKAGES.md", "\n## `shared`\n\n- `git`\n"),
            ("environment/test/manifest", "shared\n"),
            ("shared/git/.gitconfig", "[user]\nname = Test\n"),
            (
                ".gitignore",
                "config/sync\nconfig/profile\nconfig/links\nconfig/overrides\nconfig/*.lock\n.cache/\n.bin/\n",
            ),
        ] {
            write(&local.join(path), content);
        }
        fs::create_dir_all(local.join(".githooks")).unwrap();
        executable(
            &local.join(".githooks/pre-push"),
            "#!/bin/sh\nprintf 'pre-push\\n' >> \"$PUSH_LOG\"\n",
        );
        let docs = machines
            .dotfile(&local, &machines.path("local"))
            .args(["docs", "--only", "keybinds"])
            .run();
        assert!(docs.success(), "{}", docs.stderr);
        machines.git(&local, &["init", "--quiet", "."]);
        machines.commit(&local, "seed");
        let origin = machines.path("origin.git");
        machines.git(
            &local,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        machines.git(
            &local,
            &["push", "--quiet", "--no-verify", "-u", "origin", "HEAD"],
        );
        let peer = machines.peer();
        machines.git(
            &machines.path("peer"),
            &["clone", "--quiet", origin.to_str().unwrap(), "dotfiles"],
        );
        write(&peer.join("config/profile"), "test\n");
        machines.git(
            &peer,
            &[
                "remote",
                "set-url",
                "origin",
                machines.path("nowhere.git").to_str().unwrap(),
            ],
        );
        machines
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.temporary.path().join(relative)
    }

    fn local(&self) -> PathBuf {
        self.path("local/dotfiles")
    }

    fn peer(&self) -> PathBuf {
        self.path("peer/dotfiles")
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env("PUSH_LOG", self.path("push.log"));
        command
    }

    fn git(&self, cwd: &Path, arguments: &[&str]) -> String {
        let output = self
            .command("git", cwd)
            .args(["-c", "init.defaultBranch=main"])
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit(&self, repository: &Path, message: &str) -> String {
        write(&repository.join("notes").join(message), message);
        self.git(repository, &["add", "-A"]);
        self.git(
            repository,
            &["commit", "--quiet", "--no-verify", "-m", message],
        );
        self.git(repository, &["rev-parse", "HEAD"])
    }

    fn dotfile(&self, root: &Path, home: &Path) -> Bin {
        let path = format!(
            "{}:{}",
            self.path("stubs").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let git = self.command("git", root);
        Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .envs(
                git.get_envs()
                    .filter_map(|(key, value)| Some((key, value?))),
            )
            .env("DOTFILE_ROOT", root)
            .env("DOTFILE_REEXECED", "1")
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("SYSINFO_HOST", "macie")
            .env("PEER_HOME", self.path("peer"))
            .env("DOTFILES_COMPILED", self.path("peer-bin"))
            .env("PATH", path)
            .env("CI", "1")
            .env("TERM", "dumb")
    }

    fn sync_push(&self, extra: &[&str]) -> Ran {
        self.dotfile(&self.local(), &self.path("local"))
            .args(["sync", "test", "--push", "--to", "archie"])
            .args(extra)
            .run()
    }

    fn peer_state(&self) -> (String, String, String) {
        let peer = self.peer();
        (
            self.git(&peer, &["rev-parse", "HEAD"]),
            fs::read_to_string(peer.join("notes/seed")).unwrap(),
            self.git(&peer, &["stash", "list"]),
        )
    }

    fn pushes(&self) -> usize {
        fs::read_to_string(self.path("push.log"))
            .map(|log| log.lines().count())
            .unwrap_or_default()
    }

    fn origin_head(&self) -> String {
        self.git(&self.path("origin.git"), &["rev-parse", "main"])
    }
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn new_commits_reach_github_once_and_the_peer_directly() {
    let machines = Machines::new();
    let head = machines.commit(&machines.local(), "change");

    let ran = machines.sync_push(&[]);

    assert!(ran.success(), "{ran:?}");
    assert_eq!(machines.origin_head(), head);
    assert_eq!(machines.git(&machines.peer(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        machines.git(&machines.peer(), &["rev-parse", "origin/main"]),
        head,
        "the peer tracks what GitHub now holds"
    );
    assert_eq!(
        machines.pushes(),
        1,
        "only the GitHub push runs the pre-push scan"
    );
}

#[test]
fn nothing_to_push_skips_github_and_still_advances_the_peer() {
    let machines = Machines::new();
    let head = machines.commit(&machines.local(), "change");
    machines.git(&machines.local(), &["push", "--quiet", "--no-verify"]);

    let ran = machines.sync_push(&[]);

    assert!(ran.success(), "{ran:?}");
    assert_eq!(machines.pushes(), 0, "no git push to GitHub was attempted");
    assert_eq!(machines.git(&machines.peer(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn a_github_rejection_stops_before_the_peer_moves() {
    let machines = Machines::new();
    let before = machines.git(&machines.peer(), &["rev-parse", "HEAD"]);
    let elsewhere = machines.path("elsewhere");
    machines.git(
        &machines.path(""),
        &[
            "clone",
            "--quiet",
            machines.path("origin.git").to_str().unwrap(),
            "elsewhere",
        ],
    );
    machines.commit(&elsewhere, "elsewhere");
    machines.git(&elsewhere, &["push", "--quiet", "--no-verify"]);
    machines.commit(&machines.local(), "change");

    let ran = machines.sync_push(&[]);

    assert!(!ran.success(), "{ran:?}");
    assert!(ran.stderr.contains("pull --ff-only or rebase"), "{ran:?}");
    assert_eq!(
        machines.git(&machines.peer(), &["rev-parse", "HEAD"]),
        before
    );
}

#[test]
fn a_peer_ahead_of_this_machine_reports_it_as_behind() {
    let machines = Machines::new();
    let peer = machines.peer();
    let origin = machines.path("origin.git");
    machines.git(
        &peer,
        &["remote", "set-url", "origin", origin.to_str().unwrap()],
    );
    let newer = machines.commit(&peer, "peer");
    machines.git(&peer, &["push", "--quiet", "--no-verify"]);

    let ran = machines.sync_push(&[]);

    assert!(!ran.success(), "{ran:?}");
    assert!(ran.stderr.contains("is behind origin/main"), "{ran:?}");
    assert_eq!(machines.git(&peer, &["rev-parse", "HEAD"]), newer);
    assert_eq!(machines.pushes(), 0);
}

#[test]
fn peer_changes_that_do_not_conflict_are_kept_without_asking() {
    for extra in [&[][..], &["--force"][..]] {
        let machines = Machines::new();
        let head = machines.commit(&machines.local(), "change");
        write(&machines.peer().join("notes/seed"), "peer edit");
        write(&machines.peer().join("notes/scratch"), "untracked");

        let ran = machines.sync_push(extra);

        assert!(ran.success(), "{extra:?}: {ran:?}");
        assert_eq!(
            machines.peer_state(),
            (head, "peer edit".to_string(), String::new()),
            "{extra:?}"
        );
        assert!(machines.peer().join("notes/scratch").is_file());
    }
}

#[test]
fn conflicting_peer_changes_are_restored_and_reported() {
    let machines = Machines::new();
    write(&machines.local().join("notes/seed"), "local");
    machines.commit(&machines.local(), "change");
    write(&machines.peer().join("notes/seed"), "peer");
    let before = machines.peer_state();

    let ran = machines.sync_push(&[]);

    assert!(!ran.success(), "{ran:?}");
    assert!(ran.stderr.contains("conflict with 'main'"), "{ran:?}");
    assert_eq!(machines.peer_state(), before);
}

#[test]
fn force_discards_peer_changes_only_once_they_conflict() {
    let machines = Machines::new();
    write(&machines.local().join("notes/seed"), "local");
    let head = machines.commit(&machines.local(), "change");
    write(&machines.peer().join("notes/seed"), "peer");

    let ran = machines.sync_push(&["--force"]);

    assert!(ran.success(), "{ran:?}");
    assert_eq!(
        machines.peer_state(),
        (head, "local".to_string(), String::new())
    );
}

#[test]
fn peer_commits_are_rebased_onto_incoming_ones() {
    let machines = Machines::new();
    machines.commit(&machines.peer(), "peer-only");
    let head = machines.commit(&machines.local(), "change");

    let ran = machines.sync_push(&[]);

    assert!(ran.success(), "{ran:?}");
    assert_eq!(
        machines.git(&machines.peer(), &["rev-parse", "HEAD^"]),
        head
    );
    assert_eq!(
        machines.git(&machines.peer(), &["log", "-1", "--format=%s"]),
        "peer-only"
    );
}

#[test]
fn conflicting_peer_commits_leave_the_peer_untouched() {
    let machines = Machines::new();
    write(&machines.peer().join("notes/seed"), "peer");
    machines.commit(&machines.peer(), "peer-local");
    write(&machines.peer().join("notes/peer-local"), "dirty");
    write(&machines.local().join("notes/seed"), "local");
    machines.commit(&machines.local(), "change");
    let before = machines.peer_state();

    let ran = machines.sync_push(&["--force"]);

    assert!(!ran.success(), "{ran:?}");
    assert!(ran.stderr.contains("local commits conflict"), "{ran:?}");
    assert_eq!(machines.peer_state(), before);
    assert_eq!(
        fs::read_to_string(machines.peer().join("notes/peer-local")).unwrap(),
        "dirty"
    );
}
