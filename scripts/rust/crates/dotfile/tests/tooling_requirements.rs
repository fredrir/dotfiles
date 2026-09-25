#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal};
use testkit::{Bin, TempDir, tree_pairs};

struct Sandbox {
    temporary: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temporary = tree_pairs(&[
            (
                "repo/config/targets.dotfile",
                "shared/git/.gitconfig = ~/.gitconfig\n",
            ),
            ("repo/environment/test/manifest", "shared\n"),
            ("repo/shared/git/.gitconfig", "[user]\nname = Test\n"),
            ("repo/.sops.yaml", "creation_rules: []\n"),
            ("home/.config/", ""),
            ("bin/", ""),
        ]);
        let sandbox = Self { temporary };
        sandbox.write_manager();
        sandbox
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.temporary.path().join(relative)
    }

    fn write_manager(&self) {
        let manager = self.path("bin/brew");
        fs::write(
            &manager,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > {}\n",
                self.path("installed").display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&manager).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&manager, permissions).unwrap();
    }

    fn environment(&self) -> [(&'static str, PathBuf); 4] {
        [
            ("DOTFILE_ROOT", self.path("repo")),
            ("HOME", self.path("home")),
            ("XDG_CONFIG_HOME", self.path("home/.config")),
            // Only the sandbox bin: age, age-keygen and sops are installed on
            // this machine, and a real one on PATH would leave nothing missing
            // for the prompt to offer.
            ("PATH", self.path("bin")),
        ]
    }

    fn prompted(&self, slave: &File) -> Command {
        let (input, activity, errors) = stdio(slave);
        let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
        command
            .args(["sync", "test", "--dry-run"])
            .envs(self.environment())
            .env("TERM", "xterm-256color")
            .env_remove("CI")
            .env("NO_COLOR", "1")
            .stdin(input)
            .stdout(activity)
            .stderr(errors);
        take_controlling_terminal(&mut command);
        command
    }

    fn answer(&self, answer: &[u8]) -> String {
        let (master, slave, _) = open_pty(24, 80);
        let mut child = self.prompted(&slave).spawn().unwrap();
        drop(slave);
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut answered = false;
        loop {
            read_available(&master, &mut output, 100);
            if !answered && String::from_utf8_lossy(&output).contains("install with") {
                (&master).write_all(answer).unwrap();
                answered = true;
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "install prompt never settled: {:?}",
                String::from_utf8_lossy(&output)
            );
        }
        read_available(&master, &mut output, 0);
        assert!(
            answered,
            "install prompt never appeared: {:?}",
            String::from_utf8_lossy(&output)
        );
        String::from_utf8_lossy(&output).to_string()
    }
}

#[test]
fn a_missing_tool_offers_the_package_manager_instead_of_failing() {
    let sandbox = Sandbox::new();
    let rendered = sandbox.answer(b"y\n");
    assert!(rendered.contains("age, age-keygen, sops"), "{rendered}");
    assert_eq!(
        fs::read_to_string(sandbox.path("installed")).unwrap(),
        "install age sops\n"
    );
}

#[test]
fn a_declined_install_leaves_the_package_manager_alone() {
    let sandbox = Sandbox::new();
    let rendered = sandbox.answer(b"n\n");
    assert!(rendered.contains("install with"), "{rendered}");
    assert!(!sandbox.path("installed").exists());
}

#[test]
fn a_batch_run_names_the_install_command_without_asking() {
    let sandbox = Sandbox::new();
    let ran = Bin::new(env!("CARGO_BIN_EXE_dotfile"))
        .args(["sync", "test", "--dry-run"])
        .envs(sandbox.environment())
        .env("CI", "1")
        .run();
    assert!(
        ran.stderr.contains("install with brew install age sops"),
        "{}",
        ran.stderr
    );
    assert!(!sandbox.path("installed").exists());
}
