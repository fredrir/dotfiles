#![forbid(unsafe_code)]
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
struct Sandbox {
    directory: tempfile::TempDir,
    oracle: Value,
}
impl Sandbox {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let oracle: Value =
            serde_json::from_str(include_str!("fixtures/theme/oracle.json")).unwrap();
        for (path, body) in oracle["sources"].as_object().unwrap() {
            let target = directory.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, body.as_str().unwrap()).unwrap();
        }
        fs::write(directory.path().join("config/targets.dotfile"), "").unwrap();
        fs::create_dir_all(directory.path().join("home")).unwrap();
        fs::create_dir_all(directory.path().join("empty-bin")).unwrap();
        Self { directory, oracle }
    }
    fn root(&self) -> &Path {
        self.directory.path()
    }
    fn path(&self, p: &str) -> PathBuf {
        self.root().join(p)
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_dotfile"));
        c.arg("theme")
            .current_dir(self.root())
            .env("DOTFILE_ROOT", self.root())
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("home/.config"))
            .env("PATH", self.path("empty-bin"))
            .env("DOTFILE_PYTHON", self.path("python-must-not-run"))
            .env("NO_COLOR", "1");
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn read(&self, p: &str) -> String {
        fs::read_to_string(self.path(p)).unwrap()
    }
    fn outputs(&self) -> Vec<String> {
        self.oracle["expected"]["latte"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }
    fn assert_success(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}
#[test]
fn all_commands_execute_without_python_and_readonly_commands_leave_outputs_untouched() {
    let s = Sandbox::new();
    let originals = s
        .outputs()
        .iter()
        .map(|p| {
            (
                p.clone(),
                fs::read(s.path(p)).unwrap(),
                fs::metadata(s.path(p)).unwrap().modified().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    s.assert_success(&["check"]);
    s.assert_success(&["dry"]);
    s.assert_success(&["status"]);
    let preview = s.assert_success(&["preview", "latte"]);
    assert!(String::from_utf8_lossy(&preview.stdout).contains("PALETTE"));
    assert!(String::from_utf8_lossy(&preview.stdout).contains("ROLES"));
    let profiles = s.assert_success(&["profiles"]);
    assert_eq!(
        String::from_utf8(profiles.stdout).unwrap(),
        "latte\nmidnight-blue\nmocha\nsexy-purple\n"
    );
    let outputs = s.assert_success(&["outputs"]);
    assert_eq!(
        String::from_utf8(outputs.stdout).unwrap().lines().count(),
        40
    );
    let staged = s.assert_success(&["outputs", "--staged"]);
    let staged = String::from_utf8(staged.stdout).unwrap();
    assert_eq!(staged.lines().count(), 38);
    assert!(!staged.contains("linux/kde/plasma/"));
    let contrast = s.assert_success(&["contrast", "latte"]);
    assert_eq!(
        String::from_utf8(contrast.stdout).unwrap(),
        s.oracle["expected"]["latte"]["theme/contrast/latte.md"]
            .as_str()
            .unwrap()
    );
    let palette = s.assert_success(&["palette", "--json"]);
    let palette: Value = serde_json::from_slice(&palette.stdout).unwrap();
    assert_eq!(palette["version"], 1);
    assert_eq!(palette["profile"], "sexy-purple");
    assert!(
        palette["roles"]["section_system"]
            .as_str()
            .unwrap()
            .starts_with('#')
    );
    for (p, body, mtime) in originals {
        assert_eq!(fs::read(s.path(&p)).unwrap(), body, "{p}");
        assert_eq!(
            fs::metadata(s.path(&p)).unwrap().modified().unwrap(),
            mtime,
            "{p}"
        );
    }
}
#[test]
fn dry_reports_drift_sync_repairs_it_and_noop_preserves_mtime_and_permissions() {
    let s = Sandbox::new();
    let path = "shared/tmux/theme.conf";
    fs::write(s.path(path), "drift\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(s.path(path), fs::Permissions::from_mode(0o640)).unwrap();
    }
    let dry = s.run(&["dry"]);
    assert_eq!(dry.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&dry.stdout).contains(path));
    assert_eq!(s.read(path), "drift\n");
    s.assert_success(&["sync"]);
    assert_eq!(
        s.read(path),
        s.oracle["expected"]["sexy-purple"][path].as_str().unwrap()
    );
    let mtime = fs::metadata(s.path(path)).unwrap().modified().unwrap();
    s.assert_success(&["sync"]);
    assert_eq!(
        fs::metadata(s.path(path)).unwrap().modified().unwrap(),
        mtime
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(s.path(path)).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
#[test]
fn scoped_switch_changes_only_assigned_package_and_global_clears_overrides() {
    let s = Sandbox::new();
    let before = s.read("shared/tmux/theme.conf");
    s.assert_success(&["switch", "latte", "shared/zsh"]);
    assert_eq!(
        s.read("shared/zsh/conf.d/03-theme.zsh"),
        s.oracle["expected"]["latte"]["shared/zsh/conf.d/03-theme.zsh"]
            .as_str()
            .unwrap()
    );
    assert_eq!(s.read("shared/tmux/theme.conf"), before);
    assert!(s.read("config/profiles.dotfile").contains("zsh = latte"));
    s.assert_success(&["switch", "mocha", "linux/kde"]);
    s.assert_success(&["switch", "latte", "global"]);
    let selection = s.read("config/profiles.dotfile");
    assert!(!selection.contains("zsh ="));
    assert!(!selection.contains("linux/kde"));
    for path in s.outputs() {
        assert_eq!(
            s.read(&path),
            s.oracle["expected"]["latte"][&path].as_str().unwrap(),
            "{path}"
        );
    }
    s.assert_success(&["dry"]);
}
#[test]
fn invalid_profile_scope_and_late_render_failure_make_no_partial_switch() {
    let s = Sandbox::new();
    let selection = s.read("config/profiles.dotfile");
    for args in [
        vec!["switch", "missing"],
        vec!["switch", "latte", "unknown/scope"],
        vec!["switch"],
    ] {
        assert!(!s.run(&args).status.success());
        assert_eq!(s.read("config/profiles.dotfile"), selection);
    }
    let target = "linux/common/quicklaunch/config.toml";
    fs::write(s.path(target), "[broken]\n").unwrap();
    let before = s
        .outputs()
        .iter()
        .map(|p| (p.clone(), s.read(p)))
        .collect::<Vec<_>>();
    let out = s.run(&["switch", "latte", "global"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("marker"));
    assert_eq!(s.read("config/profiles.dotfile"), selection);
    for (path, body) in before {
        assert_eq!(s.read(&path), body, "{path}");
    }
}
#[test]
fn validation_rejects_bad_schema_and_collects_missing_roles() {
    let s = Sandbox::new();
    let path = s.path("theme/profiles/latte.toml");
    let source = fs::read_to_string(&path).unwrap();
    fs::write(&path, source.replace("dark = false", "dark = \"false\"")).unwrap();
    let out = s.run(&["check"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("dark must be true or false"));
}

#[cfg(unix)]
mod terminal {
    use super::*;
    use std::{
        fs::File,
        io::Write,
        process::{Child, ExitStatus},
        time::{Duration, Instant},
    };
    use testkit::pty::{
        open_pty, read_available, stdio, take_controlling_terminal, terminal_state,
    };
    struct Run {
        child: Child,
        master: File,
        output: Vec<u8>,
    }
    impl Run {
        fn start(s: &Sandbox, args: &[&str]) -> Self {
            let (master, slave, _) = open_pty(24, 110);
            let (input, output, error) = stdio(&slave);
            let mut command = s.command();
            command
                .args(args)
                .env("TERM", "xterm-256color")
                .env_remove("NO_COLOR")
                .stdin(input)
                .stdout(output)
                .stderr(error);
            take_controlling_terminal(&mut command);
            let child = command.spawn().unwrap();
            Self {
                child,
                master,
                output: Vec::new(),
            }
        }
        fn until(&mut self, label: &str) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !String::from_utf8_lossy(&self.output).contains(label) {
                read_available(&self.master, &mut self.output, 20);
                assert!(
                    Instant::now() < deadline,
                    "missing {label}: {}",
                    String::from_utf8_lossy(&self.output)
                );
            }
        }
        fn send(&mut self, keys: &[u8]) {
            self.master.write_all(keys).unwrap();
        }
        fn finish(&mut self) -> ExitStatus {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                read_available(&self.master, &mut self.output, 20);
                if let Some(status) = self.child.try_wait().unwrap() {
                    read_available(&self.master, &mut self.output, 0);
                    return status;
                }
                assert!(
                    Instant::now() < deadline,
                    "theme menu timed out: {}",
                    String::from_utf8_lossy(&self.output)
                );
            }
        }
    }
    impl Drop for Run {
        fn drop(&mut self) {
            if self.child.try_wait().unwrap().is_none() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
    #[test]
    fn cascade_navigation_and_cancel_restore_terminal_without_writes() {
        let s = Sandbox::new();
        let before = s.read("config/profiles.dotfile");
        let mut run = Run::start(&s, &[]);
        let original = terminal_state(&run.master);
        run.until("menu");
        run.send(b"j\r");
        run.until("scope");
        run.send(b"\r");
        run.until("profile");
        run.until("eza");
        run.send(b"\x1b[D");
        run.send(b"\x1b");
        assert!(run.finish().success());
        let after = terminal_state(&run.master);
        assert_eq!(original.c_lflag, after.c_lflag);
        assert_eq!(original.c_iflag, after.c_iflag);
        assert_eq!(original.c_oflag, after.c_oflag);
        let text = String::from_utf8_lossy(&run.output);
        assert!(text.contains("\x1b[?1049h"));
        assert!(text.contains("\x1b[?1049l"));
        assert_eq!(s.read("config/profiles.dotfile"), before);
    }
    #[test]
    fn raw_control_c_returns_130_and_restores_terminal() {
        let sandbox = Sandbox::new();
        let mut run = Run::start(&sandbox, &["preview"]);
        let before = terminal_state(&run.master);
        run.until("profile");
        run.send(b"\x03");
        assert_eq!(run.finish().code(), Some(130));
        let after = terminal_state(&run.master);
        assert_eq!(before.c_lflag, after.c_lflag);
        assert_eq!(before.c_iflag, after.c_iflag);
        assert_eq!(before.c_oflag, after.c_oflag);
    }

    #[test]
    fn switch_picker_completes_and_signal_teardown_restores_terminal() {
        let s = Sandbox::new();
        let mut run = Run::start(&s, &["switch"]);
        run.until("scope");
        run.send(b"\r");
        run.until("profile");
        run.send(b"\x1b[H\r");
        assert!(
            run.finish().success(),
            "{}",
            String::from_utf8_lossy(&run.output)
        );
        assert_eq!(
            s.read("shared/tmux/theme.conf"),
            s.oracle["expected"]["latte"]["shared/tmux/theme.conf"]
                .as_str()
                .unwrap()
        );
        for signal in ["-TERM", "-INT"] {
            let mut run = Run::start(&s, &["preview"]);
            let before = terminal_state(&run.master);
            run.until("profile");
            let status = std::process::Command::new("/bin/kill")
                .args([signal, &run.child.id().to_string()])
                .status()
                .unwrap();
            assert!(status.success());
            let status = run.finish();
            assert_eq!(
                status.code(),
                Some(if signal == "-TERM" { 143 } else { 130 })
            );
            let after = terminal_state(&run.master);
            assert_eq!(before.c_lflag, after.c_lflag);
            assert_eq!(before.c_iflag, after.c_iflag);
            assert_eq!(before.c_oflag, after.c_oflag);
        }
    }
}

#[test]
#[ignore = "release performance measurement"]
fn benchmark_theme_commands() {
    let sandbox = Sandbox::new();
    for arguments in [
        vec!["profiles"],
        vec!["palette", "--json"],
        vec!["check"],
        vec!["dry"],
        vec!["status"],
    ] {
        let start = std::time::Instant::now();
        sandbox.assert_success(&arguments);
        let first = start.elapsed();
        let mut samples = Vec::new();
        for _ in 0..20 {
            let start = std::time::Instant::now();
            sandbox.assert_success(&arguments);
            samples.push(start.elapsed());
        }
        samples.sort();
        println!(
            "theme {}: first process {:.3} ms, repeated p50 {:.3} ms, p95 {:.3} ms",
            arguments.join(" "),
            first.as_secs_f64() * 1000.,
            samples[10].as_secs_f64() * 1000.,
            samples[18].as_secs_f64() * 1000.
        );
    }
}

#[cfg(unix)]
#[test]
fn generated_tmux_themes_load_and_reload_on_an_isolated_server() {
    use std::ffi::OsString;
    let program = std::env::var_os("TMUX_BINARY").unwrap_or_else(|| OsString::from("tmux"));
    match Command::new(&program).arg("-V").output() {
        Ok(out) if out.status.success() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("tmux unavailable; isolated runtime check skipped");
            return;
        }
        other => panic!("tmux unavailable: {other:?}"),
    }
    let sandbox = Sandbox::new();
    let socket = sandbox.path("socket");
    struct Server {
        program: OsString,
        socket: PathBuf,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = Command::new(&self.program)
                .arg("-S")
                .arg(&self.socket)
                .arg("kill-server")
                .env_remove("TMUX")
                .output();
        }
    }
    let _server = Server {
        program: program.clone(),
        socket: socket.clone(),
    };
    let run = |args: &[&str]| {
        let output = Command::new(&program)
            .arg("-S")
            .arg(&socket)
            .args(args)
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(&[
        "-f",
        "/dev/null",
        "new-session",
        "-d",
        "-s",
        "theme",
        "sleep 60",
    ]);
    for profile in ["latte", "midnight-blue", "mocha", "sexy-purple"] {
        sandbox.assert_success(&["switch", profile, "shared/tmux"]);
        let path = sandbox.path("shared/tmux/theme.conf");
        run(&["source-file", path.to_str().unwrap()]);
        let options = run(&["show-options", "-g"]);
        run(&["source-file", path.to_str().unwrap()]);
        assert_eq!(run(&["show-options", "-g"]), options);
        assert_eq!(run(&["show-options", "-gv", "@theme_name"]).trim(), profile);
    }
}

#[cfg(unix)]
#[test]
fn final_selection_write_failure_rolls_back_every_generated_output() {
    let sandbox = Sandbox::new();
    let selection = sandbox.path("config/profiles.dotfile");
    let original = sandbox.read("config/profiles.dotfile");
    let backing = sandbox.path("selection-backing");
    fs::write(&backing, &original).unwrap();
    fs::remove_file(&selection).unwrap();
    std::os::unix::fs::symlink(&backing, &selection).unwrap();
    let outputs = sandbox
        .outputs()
        .iter()
        .map(|path| (path.clone(), sandbox.read(path)))
        .collect::<Vec<_>>();
    let result = sandbox.run(&["switch", "latte", "global"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("non-file"));
    assert_eq!(sandbox.read("selection-backing"), original);
    assert!(
        fs::symlink_metadata(&selection)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    for (path, body) in outputs {
        assert_eq!(sandbox.read(&path), body, "{path}");
    }
    assert!(!fs::read_dir(sandbox.root()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".dotfile-transaction-")
    }));
}

#[cfg(unix)]
#[test]
fn stalled_formatter_times_out_and_keeps_switch_usable() {
    let sandbox = Sandbox::new();
    testkit::executable(
        &sandbox.path("empty-bin/dotfmt"),
        "#!/bin/sh\n/bin/sleep 10\n",
    );
    let started = std::time::Instant::now();
    sandbox.assert_success(&["switch", "latte", "shared/zsh"]);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(
        sandbox.read("shared/zsh/conf.d/03-theme.zsh"),
        sandbox.oracle["expected"]["latte"]["shared/zsh/conf.d/03-theme.zsh"]
            .as_str()
            .unwrap()
    );
}

#[test]
fn malformed_fonts_yazi_contracts_and_selection_are_rejected_before_writes() {
    let sandbox = Sandbox::new();
    let fonts = sandbox.read("theme/fonts.toml");
    fs::write(
        sandbox.path("theme/fonts.toml"),
        fonts
            .replace("Hack Nerd Font Mono", "Hack, Bold")
            .replace("interface = 13", ""),
    )
    .unwrap();
    let result = sandbox.run(&["check"]);
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("must not contain a comma"), "{error}");
    assert!(error.contains("interface"), "{error}");
    fs::write(sandbox.path("theme/fonts.toml"), fonts).unwrap();
    let yazi = sandbox.read("theme/maps/yazi.toml");
    for (source, expected) in [
        (yazi.replace("[mgr]", "[manager]"), "unknown Yazi section"),
        (
            yazi.replace(
                "cwd            = { fg = \"info_text_on_canvas\" }",
                "cwd = { fg = \"info_text_on_canvas\", reversed = true }",
            ),
            "uses reversed",
        ),
        (yazi.replace("[help]", "[old_help]"), "Yazi [help] misses"),
    ] {
        fs::write(sandbox.path("theme/maps/yazi.toml"), source).unwrap();
        let result = sandbox.run(&["check"]);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fs::write(sandbox.path("theme/maps/yazi.toml"), yazi).unwrap();
    for source in [
        "shared {\n theme=missing\n}\n",
        "unknown {\n theme=latte\n}\n",
        "shared {\n missing=latte\n theme=mocha\n}\n",
        "shared {\n zsh=latte\n}\n",
        "shared {\n nested {\n theme=latte\n}\n}\n",
    ] {
        fs::write(sandbox.path("config/profiles.dotfile"), source).unwrap();
        assert!(!sandbox.run(&["dry"]).status.success());
        assert_eq!(sandbox.read("config/profiles.dotfile"), source);
    }
}

#[test]
fn switching_to_current_profile_preserves_selection_bytes_and_mtime_even_without_final_newline() {
    let sandbox = Sandbox::new();
    let source = "shared {\n  theme = sexy-purple\n}";
    let path = sandbox.path("config/profiles.dotfile");
    fs::write(&path, source).unwrap();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    sandbox.assert_success(&["switch", "sexy-purple"]);
    assert_eq!(sandbox.read("config/profiles.dotfile"), source);
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);
}
