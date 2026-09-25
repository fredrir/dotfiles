#![forbid(unsafe_code)]
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
                "[workspace]\nmembers = ['crates/ui/file-explorer', 'crates/git/gget', 'crates/dotfile']\n",
            ),
            (
                "scripts/rust/crates/ui/file-explorer/Cargo.toml",
                "[package]\nname = 'ui-file-explorer'\n",
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

    fn commit(&self) {
        for arguments in [
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .current_dir(self.root.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
}

#[test]
fn default_test_covers_every_language_without_python_routing() {
    let sandbox = Sandbox::new();
    let ran = sandbox.preview(&["test"]);
    assert!(ran.success(), "{}", ran.stderr);
    assert_eq!(ran.stdout.lines().count(), 4);
    for expected in [
        "rust build:",
        "rust test:",
        "python test:",
        "javascript test:",
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
        "ui-file-explorer,gget",
        "--pkg",
        "dotfile,dotfile-cli",
    ]);
    assert!(ran.success(), "{}", ran.stderr);
    assert_eq!(ran.stdout.lines().count(), 2);
    for name in ["ui-file-explorer", "gget", "dotfile-cli"] {
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
    let invalid = sandbox.preview(&["test", "--lang", "python", "--pkg", "ui-file-explorer"]);
    assert!(!invalid.success());
    assert!(invalid.stderr.contains("unknown package"));
}

#[test]
fn dry_run_renders_worker_limits_and_linter_selection() {
    let sandbox = Sandbox::new();
    let ran = sandbox.preview(&[
        "lint",
        "--pkg",
        "ui-file-explorer",
        "--jobs",
        "6",
        "--concurrency",
        "2",
    ]);
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("CARGO_BUILD_JOBS='6'"));
    assert!(ran.stdout.contains(
        "'clippy' '--locked' '--package' 'ui-file-explorer' '--all-targets' '--' '-D' 'warnings'"
    ));
    let shell = sandbox.preview(&["lint", "--lang", "shell"]);
    assert!(shell.success(), "{}", shell.stderr);
    assert!(
        shell
            .stdout
            .contains("shucked 'check' '--output-format' 'concise'")
    );
    assert!(shell.stdout.contains("'setup.sh'"));
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
            "ui-file-explorer",
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
            "ui-file-explorer",
            "--concurrency",
            "4",
            "--jobs",
            "4",
        ])
        .run();
    assert_eq!(ran.code(), Some(7));
    assert_eq!(
        fs::read_to_string(sandbox.root.path().join("log")).unwrap(),
        "clippy\nnextest\nnextest\n"
    );
    assert!(ran.stderr.contains("3 passed, 1 failed"));
    assert!(ran.stderr.contains("rust lint: exit 7"));
}

#[test]
fn every_action_supports_verbose_commands_and_output() {
    let sandbox = Sandbox::new();
    sandbox.tool("node", "printf 'node output\\n'");
    sandbox.tool("biome", "printf 'biome output\\n'");
    for action in ["test", "lint", "check"] {
        for flag in ["-v", "--verbose"] {
            let ran = sandbox
                .bin()
                .args([action, flag, "--lang", "javascript"])
                .run();
            assert!(ran.success(), "{}", ran.stderr);
            assert!(ran.stderr.contains("RAYON_NUM_THREADS="));
            if action != "lint" {
                assert_eq!(ran.stdout.matches("node output").count(), 1);
                assert!(ran.stderr.contains("javascript test"));
            }
            if action != "test" {
                assert!(ran.stdout.contains("biome output"));
            }
        }
    }
}

#[test]
fn preparation_uses_the_full_budget_then_python_and_rust_overlap_with_bounded_workers() {
    for (jobs, python, rust, processes) in [("6", "2", "4", "2"), ("2", "4", "1", "0")] {
        let sandbox = Sandbox::new();
        sandbox.tool("cargo", "if [ \"$2\" = list ]; then printf '%s' \"$CARGO_BUILD_JOBS\" > \"$DOTFILE_ROOT/build-workers\"; exit 0; fi\n[ \"$RAYON_NUM_THREADS\" = 1 ] || exit 81\nprevious=''; for arg do if [ \"$previous\" = --test-threads ]; then printf '%s' \"$arg\" > \"$DOTFILE_ROOT/rust-workers\"; fi; previous=$arg; done\ntouch \"$DOTFILE_ROOT/rust-ready\"\ni=0; while [ ! -f \"$DOTFILE_ROOT/python-ready\" ] && [ \"$i\" -lt 200 ]; do sleep 0.01; i=$((i + 1)); done\n[ -f \"$DOTFILE_ROOT/python-ready\" ]");
        sandbox.tool("uv", "[ -f \"$DOTFILE_ROOT/build-workers\" ] || exit 82\n[ \"$RAYON_NUM_THREADS\" = 1 ] || exit 83\nprevious=''; for arg do if [ \"$previous\" = --numprocesses ]; then printf '%s' \"$arg\" > \"$DOTFILE_ROOT/python-workers\"; fi; previous=$arg; done\ntouch \"$DOTFILE_ROOT/python-ready\"\ni=0; while [ ! -f \"$DOTFILE_ROOT/rust-ready\" ] && [ \"$i\" -lt 200 ]; do sleep 0.01; i=$((i + 1)); done\n[ -f \"$DOTFILE_ROOT/rust-ready\" ]");
        let ran = sandbox
            .bin()
            .args([
                "test",
                "--lang",
                "rust,python",
                "--jobs",
                jobs,
                "--python-workers",
                python,
            ])
            .run();
        assert!(ran.success(), "{}", ran.stderr);
        for (name, expected) in [("build", jobs), ("python", processes), ("rust", rust)] {
            assert_eq!(
                fs::read_to_string(sandbox.root.path().join(format!("{name}-workers"))).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn short_doctests_do_not_permanently_reduce_nextest_workers() {
    let sandbox = Sandbox::new();
    let library = sandbox
        .root
        .path()
        .join("scripts/rust/crates/ui/file-explorer/src");
    fs::create_dir_all(&library).unwrap();
    fs::write(library.join("lib.rs"), "").unwrap();
    let preview = sandbox.preview(&["test", "--lang", "rust"]);
    let docs = preview
        .stdout
        .lines()
        .find(|line| line.starts_with("rust test doctests:"))
        .unwrap();
    assert!(docs.contains("'--workspace'"), "{docs}");
    sandbox.tool("cargo", "if [ \"$2\" = list ]; then exit 0; fi\nif [ \"$1\" = test ]; then sleep 0.1; touch \"$DOTFILE_ROOT/docs-finished\"; exit 0; fi\n[ -f \"$DOTFILE_ROOT/docs-finished\" ] || exit 80\nprevious=''; for arg do if [ \"$previous\" = --test-threads ]; then [ \"$arg\" = 2 ] || exit 81; fi; previous=$arg; done");
    let ran = sandbox
        .bin()
        .args(["test", "--pkg", "ui-file-explorer", "-j", "2"])
        .run();
    assert!(ran.success(), "{}", ran.stderr);
}

#[test]
fn real_nextest_accepts_prebuilt_metadata_and_doctests_keep_the_package_selection() {
    if !Command::new("cargo")
        .args(["nextest", "--version"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return;
    }
    let root = tree_pairs(&[
        ("config/targets.dotfile", ""),
        ("scripts/python/tests/", ""),
        (
            "scripts/rust/Cargo.toml",
            "[workspace]\nmembers = ['crates/demo']\nresolver = '2'\n",
        ),
        (
            "scripts/rust/Cargo.lock",
            "version = 4\n[[package]]\nname = 'demo'\nversion = '0.1.0'\n",
        ),
        (
            "scripts/rust/crates/demo/Cargo.toml",
            "[package]\nname = 'demo'\nversion = '0.1.0'\nedition = '2024'\n",
        ),
        (
            "scripts/rust/crates/demo/src/lib.rs",
            "#[test]\nfn fixture() { assert_eq!(std::env::var(\"RUST_TEST_THREADS\").unwrap(), \"1\"); }\n",
        ),
    ]);
    let ran = Bin::new(env!("CARGO_BIN_EXE_dotfile"))
        .args(["dev", "test", "--lang", "rust", "-j", "2", "-v"])
        .env("DOTFILE_ROOT", root.path())
        .env("CARGO_TARGET_DIR", root.path().join("target"))
        .current_dir(root.path())
        .run();
    assert!(ran.success(), "{}\n{}", ran.stdout, ran.stderr);
    assert!(ran.stderr.contains("3 passed"), "{}", ran.stderr);
    assert!(ran.stdout.contains("demo fixture"), "{}", ran.stdout);
}

#[test]
fn failed_build_skips_dependents_and_keeps_independent_suites_running() {
    let sandbox = Sandbox::new();
    sandbox.tool("cargo", "printf 'build failed\\n' >&2; exit 7");
    sandbox.tool("uv", "exit 0");
    let ran = sandbox.bin().args(["test", "--lang", "rust,python"]).run();
    assert_eq!(ran.code(), Some(7));
    assert!(
        ran.stderr.contains("1 passed, 1 failed, 1 skipped"),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains("rust build: exit 7"));
}

#[test]
fn python_receives_fresh_build_artifacts_before_running() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.root.path().join("scripts/python/tests/hyprland")).unwrap();
    sandbox.tool(
        "cargo",
        "[ \"$1\" = build ] || exit 9\nprintf 'prepared binary manifest\\n'",
    );
    sandbox.tool("uv", "[ -f \"$DOTFILE_DEV_BUILD_MANIFEST\" ] || exit 8\n[ \"$(cat \"$DOTFILE_DEV_BUILD_MANIFEST\")\" = 'prepared binary manifest' ]");
    let ran = sandbox
        .bin()
        .args(["test", "--lang", "python", "--pkg", "hyprland"])
        .run();
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stderr.contains("2 passed"));
}

#[test]
fn rust_package_selection_includes_owned_python_suites_and_one_build() {
    let sandbox = Sandbox::new();
    for suite in ["dotfile", "hyprland", "transcript", "hwtune"] {
        fs::create_dir_all(sandbox.root.path().join("scripts/python/tests").join(suite)).unwrap();
    }
    let selected = sandbox.preview(&["test", "--pkg", "dotfile-cli", "--lang", "python"]);
    assert!(selected.success(), "{}", selected.stderr);
    for suite in ["dotfile", "hyprland", "transcript"] {
        assert!(
            selected.stdout.contains(&format!("'tests/{suite}'")),
            "{}",
            selected.stdout
        );
    }
    assert!(!selected.stdout.contains("'tests/hwtune'"));
    assert!(!selected.stdout.contains("'tests/theme'"));
    assert_eq!(selected.stdout.matches("python build:").count(), 1);
    assert_eq!(
        selected.stdout.matches("'--package' 'dotfile-cli'").count(),
        1
    );
}

#[test]
fn hwtune_terminal_suite_prepares_its_native_binaries() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.root.path().join("scripts/python/tests/hwtune")).unwrap();
    fs::write(
        sandbox.root.path().join("scripts/rust/Cargo.toml"),
        "[workspace]\nmembers = ['crates/hwtune', 'crates/bench-workloads']\n",
    )
    .unwrap();
    for package in ["hwtune", "bench-workloads"] {
        let directory = sandbox
            .root
            .path()
            .join("scripts/rust/crates")
            .join(package);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("Cargo.toml"),
            format!("[package]\nname = '{package}'\n"),
        )
        .unwrap();
    }
    let selected = sandbox.preview(&["test", "--pkg", "hwtune", "--lang", "python"]);
    assert!(selected.success(), "{}", selected.stderr);
    assert!(selected.stdout.contains("'tests/hwtune'"));
    assert!(selected.stdout.contains("'--package' 'hwtune'"));
    assert!(selected.stdout.contains("'--package' 'bench-workloads'"));
    assert!(!selected.stdout.contains("'workstation-sysinfo'"));
    assert_eq!(selected.stdout.matches("python build:").count(), 1);
}

#[test]
fn changed_rust_dependencies_select_the_same_python_suites() {
    let sandbox = Sandbox::new();
    for suite in ["dotfile", "transcript"] {
        fs::create_dir_all(sandbox.root.path().join("scripts/python/tests").join(suite)).unwrap();
    }
    sandbox.tool(
        "cargo",
        "printf '%s' '{\"packages\":[{\"name\":\"dotfile-cli\",\"dependencies\":[]}]}'",
    );
    sandbox.commit();
    fs::write(
        sandbox
            .root
            .path()
            .join("scripts/rust/crates/dotfile/new.rs"),
        "fixture",
    )
    .unwrap();
    let selected = sandbox.preview(&["test", "--changed", "--lang", "python"]);
    assert!(selected.success(), "{}", selected.stderr);
    for suite in ["dotfile", "transcript"] {
        assert!(
            selected.stdout.contains(&format!("'tests/{suite}'")),
            "{}",
            selected.stdout
        );
    }
    assert!(!selected.stdout.contains("'tests/theme'"));
    assert_eq!(selected.stdout.matches("python build:").count(), 1);
}

#[test]
fn changed_selection_follows_transitive_dependents_and_intersects_explicit_packages() {
    let sandbox = Sandbox::new();
    sandbox.tool("cargo", "printf '%s' '{\"packages\":[{\"name\":\"ui-file-explorer\",\"dependencies\":[]},{\"name\":\"gget\",\"dependencies\":[{\"name\":\"ui-file-explorer\"}]},{\"name\":\"dotfile-cli\",\"dependencies\":[{\"name\":\"gget\"}]}]}'");
    sandbox.commit();
    fs::write(
        sandbox
            .root
            .path()
            .join("scripts/rust/crates/ui/file-explorer/new.rs"),
        "fixture",
    )
    .unwrap();
    let ran = sandbox.preview(&["test", "--changed", "--lang", "rust"]);
    assert!(ran.success(), "{}", ran.stderr);
    for name in ["ui-file-explorer", "gget", "dotfile-cli"] {
        assert!(
            ran.stdout.contains(&format!("'--package' '{name}'")),
            "{}",
            ran.stdout
        );
    }
    let focused = sandbox.preview(&["test", "--changed", "--pkg", "gget", "--lang", "rust"]);
    assert!(focused.success(), "{}", focused.stderr);
    assert!(focused.stdout.contains("'--package' 'gget'"));
    assert!(!focused.stdout.contains("'--package' 'ui-file-explorer'"));
}

#[test]
fn changed_python_helpers_lint_the_shared_files_and_manifest() {
    let sandbox = Sandbox::new();
    let python = sandbox.root.path().join("scripts/python");
    fs::write(python.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
    fs::write(python.join("tests/conftest.py"), "").unwrap();
    sandbox.commit();
    fs::write(python.join("tests/conftest.py"), "changed").unwrap();
    let ran = sandbox.preview(&["lint", "--changed", "--lang", "python,toml"]);
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("'ruff' 'check' '.'"), "{}", ran.stdout);
    assert!(
        ran.stdout.contains("'scripts/python/pyproject.toml'"),
        "{}",
        ran.stdout
    );
    assert!(!ran.stdout.contains("rust lint"));
}

#[test]
fn changed_selection_handles_clean_trees_deletions_untracked_files_and_invalid_refs() {
    let sandbox = Sandbox::new();
    sandbox.commit();
    let clean = sandbox.preview(&["test", "--changed"]);
    assert!(clean.success(), "{}", clean.stderr);
    assert!(clean.stdout.is_empty());
    fs::remove_file(
        sandbox
            .root
            .path()
            .join("scripts/python/tests/theme/test_theme.py"),
    )
    .unwrap();
    let deleted = sandbox.preview(&["test", "--changed"]);
    assert!(deleted.success(), "{}", deleted.stderr);
    assert!(deleted.stdout.contains("'tests/theme'"));
    assert!(!deleted.stdout.contains("rust test"));
    fs::write(
        sandbox
            .root
            .path()
            .join("shared/obsidian/plugins/agent-transcripts/new.test.js"),
        "",
    )
    .unwrap();
    let added = sandbox.preview(&["test", "--changed"]);
    assert!(added.stdout.contains("javascript test"));
    let invalid = sandbox.preview(&["test", "--changed=missing-reference"]);
    assert!(!invalid.success());
    assert!(invalid.stdout.is_empty());
}

#[test]
fn changed_reference_includes_branch_commits_and_shared_inputs_expand_selection() {
    let sandbox = Sandbox::new();
    sandbox.commit();
    fs::write(
        sandbox
            .root
            .path()
            .join("scripts/python/tests/theme/test_theme.py"),
        "changed",
    )
    .unwrap();
    sandbox.commit();
    let ran = sandbox.preview(&["test", "--changed=HEAD~1"]);
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("'tests/theme'"));
    fs::write(sandbox.root.path().join("setup.sh"), "changed").unwrap();
    let shared = sandbox.preview(&["test", "--changed"]);
    assert!(shared.success(), "{}", shared.stderr);
    assert!(shared.stdout.contains("rust test"));
    assert!(shared.stdout.contains("python test"));
    assert!(shared.stdout.contains("javascript test"));
}

#[test]
fn interrupt_kills_descendants_and_cancels_pending_tasks() {
    let sandbox = Sandbox::new();
    let stubborn = "trap '' INT TERM\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"$DEV_LOG\"\nwait";
    sandbox.tool("cargo", stubborn);
    sandbox.tool("uv", stubborn);
    sandbox.tool("node", stubborn);
    let mut child = sandbox.bin().args(["test", "--concurrency", "1"]).spawn();
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
