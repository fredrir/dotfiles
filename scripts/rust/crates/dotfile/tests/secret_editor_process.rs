#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal, terminal_state};
use testkit::{TempDir, executable};

struct Fixture {
    temp: TempDir,
    root: PathBuf,
    home: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(home.join(".config/dotfile")).unwrap();
        fs::create_dir_all(temp.path().join("staging")).unwrap();
        fs::write(root.join("config/targets.dotfile"), "").unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let fixture = Self { temp, root, home };
        for arguments in [&["init"][..], &["enroll", "test"][..]] {
            let output = fixture.command().args(arguments).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fixture
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
        command
            .arg("secret")
            .env("DOTFILE_ROOT", &self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("TMPDIR", self.temp.path().join("staging"))
            .env_remove("SOPS_AGE_KEY")
            .env_remove("SOPS_AGE_KEY_CMD")
            .current_dir(&self.root);
        command
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn cancelled_real_sops_editor_preserves_ciphertext_and_removes_private_staging() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    for existing in [false, true] {
        for terminal_interrupt in [false, true] {
            let fixture = Fixture::new();
            let editor = fixture.temp.path().join("editor");
            let ciphertext = fixture.root.join("vars.enc.yaml");
            if existing {
                executable(
                    &editor,
                    "#!/bin/sh\nprintf 'host: editor-fixture.private.example\\n' > \"$1\"\n",
                );
                let output = fixture
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
            }
            let before_ciphertext = fs::read(&ciphertext).ok();
            executable(
                &editor,
                "#!/bin/sh\nprintf 'editor-awaiting-input\\n'\nexec sleep 30\n",
            );
            let (mut master, slave, _) = open_pty(24, 100);
            let before_terminal = terminal_state(&master);
            let (input, output, error) = stdio(&slave);
            let mut command = fixture.command();
            command
                .args(["edit", "vars"])
                .env("EDITOR", &editor)
                .stdin(input)
                .stdout(output)
                .stderr(error);
            take_controlling_terminal(&mut command);
            let mut process = Process(command.spawn().unwrap());
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut output = Vec::new();
            while !String::from_utf8_lossy(&output).contains("editor-awaiting-input") {
                read_available(&master, &mut output, 20);
                assert!(
                    Instant::now() < deadline,
                    "SOPS editor did not start: {}",
                    String::from_utf8_lossy(&output)
                );
            }
            if terminal_interrupt {
                master.write_all(b"\x03").unwrap();
            } else {
                kill(Pid::from_raw(process.0.id() as i32), Signal::SIGTERM).unwrap();
            }
            let status = loop {
                read_available(&master, &mut output, 20);
                if let Some(status) = process.0.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "SOPS editor cancellation timed out: {}",
                    String::from_utf8_lossy(&output)
                );
            };
            if terminal_interrupt {
                // SOPS versions differ in their status for an interrupted editor.
                // Preserve the tool's failure instead of inferring a parent signal.
                assert!(!status.success(), "{}", String::from_utf8_lossy(&output));
            } else {
                assert_eq!(
                    status.code(),
                    Some(143),
                    "{}",
                    String::from_utf8_lossy(&output)
                );
            }
            assert_eq!(fs::read(&ciphertext).ok(), before_ciphertext);
            assert_eq!(
                fs::read_dir(fixture.temp.path().join("staging"))
                    .unwrap()
                    .count(),
                0
            );
            assert!(!String::from_utf8_lossy(&output).contains("editor-fixture.private.example"));
            let after_terminal = terminal_state(&master);
            assert_eq!(before_terminal.c_lflag, after_terminal.c_lflag);
            assert_eq!(before_terminal.c_iflag, after_terminal.c_iflag);
            assert_eq!(before_terminal.c_oflag, after_terminal.c_oflag);
        }
    }
}
