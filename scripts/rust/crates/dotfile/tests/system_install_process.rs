#![cfg(target_os = "linux")]
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
    bin: PathBuf,
    destination: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        let bin = temp.path().join("bin");
        let destination = temp.path().join("etc");
        for directory in [
            root.join("config"),
            root.join("environment/test"),
            root.join("shared/service/etc"),
            home.join(".config"),
            bin.clone(),
            destination.clone(),
            temp.path().join("staging"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(root.join("shared/service/.system"), "").unwrap();
        fs::write(root.join("shared/service/etc/first.conf"), "first\n").unwrap();
        fs::write(root.join("shared/service/etc/second.conf"), "second\n").unwrap();
        fs::write(root.join("environment/test/manifest"), "shared\n").unwrap();
        fs::write(root.join("config/profile"), "test\n").unwrap();
        fs::write(
            root.join("config/targets.dotfile"),
            format!("shared/service/etc = {}\n", destination.display()),
        )
        .unwrap();
        Self {
            temp,
            root,
            home,
            bin,
            destination,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
        command
            .args(["system", "install", "--yes"])
            .env("DOTFILE_ROOT", &self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("TMPDIR", self.temp.path().join("staging"))
            .env("INSTALL_LOG", self.temp.path().join("install.log"))
            .env("NO_COLOR", "1")
            .current_dir(&self.root);
        command
    }
}

#[test]
fn failed_system_install_reports_partial_result_and_nonzero_status() {
    let fixture = Fixture::new();
    executable(
        &fixture.bin.join("sudo"),
        "#!/bin/sh\nwhile [ \"$#\" -gt 2 ]; do shift; done\nprintf '%s\\n' \"$2\" >> \"$INSTALL_LOG\"\ncase \"$2\" in */first.conf) exit 42;; esac\ncp \"$1\" \"$2\"\n",
    );
    let output = fixture.command().output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("installed 1 of 2"), "{stdout}\n{stderr}");
    assert!(stderr.contains("failed"), "{stderr}");
    assert!(!fixture.destination.join("first.conf").exists());
    assert_eq!(
        fs::read_to_string(fixture.destination.join("second.conf")).unwrap(),
        "second\n"
    );
    assert_eq!(
        fs::read_dir(fixture.temp.path().join("staging"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn install_enables_units_that_a_tracked_preset_names() {
    let fixture = Fixture::new();
    let preset = fixture
        .root
        .join("shared/service/etc/systemd/system-preset/50-service.preset");
    fs::create_dir_all(preset.parent().unwrap()).unwrap();
    fs::write(&preset, "enable fan.service\nenable on.timer\nenable *\n").unwrap();
    executable(
        &fixture.bin.join("systemctl"),
        "#!/bin/sh\ncase \"$3\" in fan.service) echo disabled; exit 1;; on.timer) echo enabled;; esac\n",
    );
    executable(
        &fixture.bin.join("sudo"),
        "#!/bin/sh\nif [ \"$1\" = systemctl ]; then printf '%s\\n' \"$*\" >> \"$INSTALL_LOG\"; exit 0; fi\nwhile [ \"$#\" -gt 2 ]; do shift; done\nmkdir -p \"$(dirname \"$2\")\"\ncp \"$1\" \"$2\"\n",
    );
    let output = fixture.command().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("enabled fan.service"), "{stdout}");
    assert_eq!(
        fs::read_to_string(fixture.temp.path().join("install.log")).unwrap(),
        "systemctl enable --now -- fan.service\n"
    );
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn cancelled_system_install_stops_sudo_and_restores_terminal() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    for terminal_interrupt in [false, true] {
        let fixture = Fixture::new();
        executable(
            &fixture.bin.join("sudo"),
            "#!/bin/sh\nprintf '%s\\n' started >> \"$INSTALL_LOG\"\nprintf 'sudo-awaiting-input\\n'\nexec sleep 30\n",
        );
        let (mut master, slave, _) = open_pty(24, 100);
        let before = terminal_state(&master);
        let (input, output, error) = stdio(&slave);
        let mut command = fixture.command();
        command.stdin(input).stdout(output).stderr(error);
        take_controlling_terminal(&mut command);
        let mut process = Process(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("sudo-awaiting-input") {
            read_available(&master, &mut output, 20);
            assert!(
                Instant::now() < deadline,
                "sudo did not start: {}",
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
                "cancelled install did not exit: {}",
                String::from_utf8_lossy(&output)
            );
        };
        assert_eq!(
            status.code(),
            Some(if terminal_interrupt { 130 } else { 143 }),
            "{}",
            String::from_utf8_lossy(&output)
        );
        assert_eq!(
            fs::read_to_string(fixture.temp.path().join("install.log")).unwrap(),
            "started\n"
        );
        assert_eq!(
            fs::read_dir(fixture.temp.path().join("staging"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
        let after = terminal_state(&master);
        assert_eq!(before.c_lflag, after.c_lflag);
        assert_eq!(before.c_iflag, after.c_iflag);
        assert_eq!(before.c_oflag, after.c_oflag);
    }
}
