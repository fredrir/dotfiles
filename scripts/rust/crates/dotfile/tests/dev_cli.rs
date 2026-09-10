#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use testkit::{Bin, TempDir, executable, tree_pairs};

struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = tree_pairs(&[
            ("config/targets.dotfile", ""),
            (
                "scripts/rust/Cargo.toml",
                "[workspace]\nmembers = ['crates/file-explorer', 'crates/git/gget', 'crates/dotfile']\n",
            ),
            (
                "scripts/rust/crates/file-explorer/Cargo.toml",
                "[package]\nname = 'file-explorer'\n",
            ),
            (
                "scripts/rust/crates/git/gget/Cargo.toml",
                "[package]\nname = 'gget'\n",
            ),
            (
                "scripts/rust/crates/dotfile/Cargo.toml",
                "[package]\nname = 'dotfile-cli'\n",
            ),
            ("scripts/python/tests/theme/test_theme.py", ""),
            ("scripts/python/src/tools/theme/__init__.py", ""),
            (
                "shared/obsidian/plugins/agent-transcripts/plugin.test.js",
                "",
            ),
            ("shared/wezterm/tests/tmux-workspace.lua", ""),
            ("shared/zsh/check.zsh", ""),
            ("setup.sh", "#!/bin/sh\n"),
            ("bin/", ""),
        ]);
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success()
        );
        Self { root }
    }

    fn bin(&self) -> Bin {
        Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .arg("dev")
            .env("DOTFILE_ROOT", self.root.path())
            .env("DOTFILE_PYTHON", "/missing-backend")
            .env("TMPDIR", self.root.path())
            .env("DEV_LOG", self.root.path().join("log"))
            .env("DEV_LOCK", self.root.path().join("running"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.path().join("bin").display()),
            )
            .current_dir(self.root.path())
    }

    fn tool(&self, name: &str, body: &str) {
        executable(
            &self.root.path().join("bin").join(name),
            &format!("#!/bin/sh\n{body}\n"),
        );
    }

    fn preview(&self, arguments: &[&str]) -> testkit::Ran {
        self.bin().args(arguments).arg("--dry-run").run()
    }
}

#[test]
fn default_test_covers_every_suite_without_python_routing() {
    let sandbox = Sandbox::new();
    let ran = sandbox.preview(&["test"]);
    assert!(ran.success(), "{}", ran.stderr);
    assert_eq!(ran.stdout.lines().count(), 7);
    for expected in [
        "rust test:",
        "python test:",
        "javascript test:",
        "lua test linux native-splits:",
        "lua test mac hwire-splits:",
    ] {
        assert!(ran.stdout.contains(expected), "{}", ran.stdout);
    }
    assert!(!sandbox.root.path().join("log").exists());
}

#[test]
fn package_names_aliases_and_nested_crates_resolve_once() {
    let sandbox = Sandbox::new();
    let ran = sandbox.preview(&[
        "test",
        "--pkg",
        "file-explorer,gget",
        "--pkg",
        "dotfile,dotfile-cli",
    ]);
    assert!(ran.success(), "{}", ran.stderr);
    assert_eq!(ran.stdout.lines().count(), 1);
    for name in ["file-explorer", "gget", "dotfile-cli"] {
        assert!(ran.stdout.contains(&format!("'--package' '{name}'")));
    }
    assert_eq!(ran.stdout.matches("'dotfile-cli'").count(), 1);
    assert!(!ran.stdout.contains("--workspace"));
}

#[test]
fn language_and_package_filters_apply_to_both_check_phases() {
    let sandbox = Sandbox::new();
    let ran = sandbox.preview(&["check", "--lang", "python", "--pkg", "theme"]);
    assert!(ran.success(), "{}", ran.stderr);
    assert_eq!(ran.stdout.lines().count(), 2);
    assert!(
        ran.stdout
            .contains("'ruff' 'check' 'tests/theme' 'src/tools/theme'")
    );
    assert!(ran.stdout.contains("'pytest' 'tests/theme'"));
    assert!(!ran.stdout.contains("cargo"));
    let invalid = sandbox.preview(&["test", "--lang", "python", "--pkg", "file-explorer"]);
    assert!(!invalid.success());
    assert!(invalid.stderr.contains("unknown package"));
}

#[test]
fn dry_run_renders_worker_limits_and_linter_selection() {
    let sandbox = Sandbox::new();
    let ran = sandbox.preview(&[
        "lint",
        "--pkg",
        "file-explorer",
        "--jobs",
        "6",
        "--concurrency",
        "2",
    ]);
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("CARGO_BUILD_JOBS='3'"));
    assert!(ran.stdout.contains(
        "'clippy' '--locked' '--package' 'file-explorer' '--all-targets' '--' '-D' 'warnings'"
    ));
    let shell = sandbox.preview(&["lint", "--lang", "shell"]);
    assert!(shell.success(), "{}", shell.stderr);
    assert!(shell.stdout.contains("zsh '-n' 'shared/zsh/check.zsh'"));
    assert!(shell.stdout.contains("shellcheck 'setup.sh'"));
}

#[test]
fn forwarding_preserves_arguments_without_shell_evaluation() {
    let sandbox = Sandbox::new();
    sandbox.tool(
        "cargo",
        "printf '%s\\n' \"$@\"; printf 'threads=%s\\n' \"$RUST_TEST_THREADS\"",
    );
    let ran = sandbox
        .bin()
        .args([
            "test",
            "--verbose",
            "--pkg",
            "file-explorer",
            "--jobs",
            "3",
            "--concurrency",
            "1",
            "--",
            "name with spaces;$(touch bad)",
            "--",
            "--nocapture",
        ])
        .run();
    assert!(ran.success(), "{}", ran.stderr);
    assert!(
        ran.stdout
            .contains("name with spaces;$(touch bad)\n--\n--nocapture\nthreads=3")
    );
    assert!(!sandbox.root.path().join("scripts/rust/bad").exists());
    let ambiguous = sandbox.bin().args(["test", "--", "--nocapture"]).run();
    assert!(!ambiguous.success());
    assert!(ambiguous.stderr.contains("one runner"));
}

#[test]
fn check_serializes_cargo_and_continues_after_lint_failure() {
    let sandbox = Sandbox::new();
    sandbox.tool("taplo", "exit 0");
    sandbox.tool("cargo", "mkdir \"$DEV_LOCK\" || exit 99\nprintf '%s\\n' \"$1\" >> \"$DEV_LOG\"\nsleep 0.1\nrmdir \"$DEV_LOCK\"\n[ \"$1\" != clippy ] || exit 7");
    let ran = sandbox
        .bin()
        .args([
            "check",
            "--pkg",
            "file-explorer",
            "--concurrency",
            "4",
            "--jobs",
            "4",
        ])
        .run();
    assert_eq!(ran.code(), Some(7));
    assert_eq!(
        fs::read_to_string(sandbox.root.path().join("log")).unwrap(),
        "clippy\ntest\n"
    );
    assert!(ran.stderr.contains("2 passed, 1 failed"));
    assert!(ran.stderr.contains("rust lint: exit 7"));
}

#[test]
fn missing_tools_are_failures_and_other_suites_still_run() {
    let sandbox = Sandbox::new();
    sandbox.tool("lua", "printf 'lua passed\\n'");
    let ran = sandbox.bin().args(["test", "--lang", "python,lua"]).run();
    assert_eq!(ran.code(), Some(127));
    assert!(ran.stderr.contains("python test"));
    assert!(ran.stderr.contains("uv:"));
    assert!(ran.stderr.contains("4 passed, 1 failed"));
}

#[test]
fn concurrency_is_bounded_and_independent_tasks_overlap() {
    let sandbox = Sandbox::new();
    sandbox.tool(
        "lua",
        "printf 'start\\n' >> \"$DEV_LOG\"\nsleep 0.15\nprintf 'end\\n' >> \"$DEV_LOG\"",
    );
    let ran = sandbox
        .bin()
        .args(["test", "--lang", "lua", "--jobs", "2", "--concurrency", "2"])
        .run();
    assert!(ran.success(), "{}", ran.stderr);
    let mut active = 0;
    let mut peak = 0;
    for event in fs::read_to_string(sandbox.root.path().join("log"))
        .unwrap()
        .lines()
    {
        active += if event == "start" { 1 } else { -1 };
        peak = peak.max(active);
        assert!((0..=2).contains(&active));
    }
    assert_eq!(peak, 2);
    assert_eq!(active, 0);
}

#[test]
fn default_output_groups_successes_and_discards_tool_logs() {
    let sandbox = Sandbox::new();
    sandbox.tool("lua", "printf 'noisy tool output\\n'");
    let ran = sandbox.bin().args(["test", "--lang", "lua"]).run();
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stdout.is_empty(), "{}", ran.stdout);
    assert_eq!(ran.stderr.matches("lua test").count(), 1, "{}", ran.stderr);
    assert!(ran.stderr.contains("lua test (4 tasks)"));
    assert!(ran.stderr.contains("4 passed"));
    assert!(!ran.stderr.contains("noisy tool output"));
    assert!(!ran.stderr.contains("0 failed"));
    assert!(!ran.stderr.contains('\x1b'));
    assert!(ran.stderr.lines().count() <= 5, "{}", ran.stderr);
    assert!(fs::read_dir(sandbox.root.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".log")
    }));
}

#[test]
fn failure_output_is_bounded_and_preserves_the_complete_log() {
    let sandbox = Sandbox::new();
    sandbox.tool(
        "cargo",
        "i=0; while [ \"$i\" -lt 10000 ]; do printf 'line %s\\n' \"$i\"; i=$((i + 1)); done\nprintf 'useful failure\\n' >&2\nexit 9",
    );
    let ran = sandbox.bin().args(["test", "--lang", "rust"]).run();
    assert_eq!(ran.code(), Some(9));
    assert!(ran.stdout.is_empty());
    assert!(ran.stderr.contains("rust test: exit 9"));
    assert!(ran.stderr.contains("useful failure"));
    assert!(!ran.stderr.contains("line 0\n"));
    assert!(ran.stderr.lines().count() < 16, "{}", ran.stderr);
    let path = ran
        .stderr
        .lines()
        .find_map(|line| line.strip_prefix("  log: "))
        .unwrap();
    let log = fs::read_to_string(path).unwrap();
    assert!(log.starts_with("line 0\n"));
    assert!(log.ends_with("useful failure\n"));
    assert_eq!(log.lines().count(), 10001);
}

#[test]
fn every_action_supports_verbose_commands_and_output() {
    let sandbox = Sandbox::new();
    sandbox.tool("lua", "printf 'lua output\\n'");
    sandbox.tool("luacheck", "printf 'luacheck output\\n'");
    for action in ["test", "lint", "check"] {
        for flag in ["-v", "--verbose"] {
            let ran = sandbox.bin().args([action, flag, "--lang", "lua"]).run();
            assert!(ran.success(), "{}", ran.stderr);
            assert!(ran.stderr.contains("RAYON_NUM_THREADS="));
            if action != "lint" {
                assert_eq!(ran.stdout.matches("lua output").count(), 4);
                assert!(ran.stderr.contains("lua test mac hwire-splits"));
            }
            if action != "test" {
                assert!(ran.stdout.contains("luacheck output"));
            }
        }
    }
}

#[test]
fn interrupt_kills_descendants_and_cancels_pending_tasks() {
    let sandbox = Sandbox::new();
    sandbox.tool(
        "lua",
        "trap '' INT TERM\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"$DEV_LOG\"\nwait",
    );
    let mut child = sandbox
        .bin()
        .args(["test", "--lang", "lua", "--concurrency", "1"])
        .spawn();
    let log: PathBuf = sandbox.root.path().join("log");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !log.is_file()
        || fs::read_to_string(&log)
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let descendant = fs::read_to_string(&log).unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "cancellation timed out");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130));
    let mut alive = true;
    while Instant::now() < deadline {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", descendant.trim()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        alive =
            output.status.success() && !state.trim().is_empty() && !state.trim().starts_with('Z');
        if !alive {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!alive, "descendant survived cancellation");
}
