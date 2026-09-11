#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Repository {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Repository {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(root.join("build.jsonl"), "").unwrap();
        Self {
            _temp: temp,
            root,
            home,
        }
    }

    fn put(&self, name: &str, text: &str) {
        let path = self.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dotfile"))
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("DOTFILE_ROOT", &self.root)
            .env("DOTFILE_DEV_BUILD_MANIFEST", self.root.join("build.jsonl"))
            .env("PATH", "")
            .env_remove("HWTUNE_BENCHMARKS")
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn docs_checks_and_plans_are_read_only_and_writes_are_idempotent() {
    let repo = Repository::new();
    repo.put("shared/tmux/keys.conf", "bind r refresh-client\n");
    let args = ["docs", "--only", "keybinds"];
    assert_eq!(
        repo.run(&[&args[..], &["--check"]].concat()).status.code(),
        Some(1)
    );
    assert!(!repo.root.join("docs").exists());
    assert!(
        repo.run(&[&args[..], &["--dry-run"]].concat())
            .status
            .success()
    );
    assert!(!repo.root.join("docs").exists());
    assert!(repo.run(&args).status.success());
    let page = repo.root.join("docs/keybinds/tmux.md");
    let before = fs::metadata(&page).unwrap().modified().unwrap();
    assert!(repo.run(&args).status.success());
    assert!(
        repo.run(&[&args[..], &["--check"]].concat())
            .status
            .success()
    );
    assert_eq!(fs::metadata(page).unwrap().modified().unwrap(), before);
    assert_eq!(
        repo.run(&["docs", "--check", "--dry-run"]).status.code(),
        Some(2)
    );
}

#[test]
fn reference_updates_preserve_authored_text_and_noop_mtime() {
    let repo = Repository::new();
    let args = ["docs", "--only", "cli"];
    assert!(repo.run(&args).status.success());
    let path = repo.root.join("docs/cli/dotfile.md");
    let text = fs::read_to_string(&path).unwrap();
    let authored = format!("intro\n{text}\nclosing\n");
    fs::write(&path, &authored).unwrap();
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(repo.run(&args).status.success());
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    let stale = authored.replace("Manages this repository", "stale");
    assert!(stale.contains("stale"));
    fs::write(&path, &stale).unwrap();
    assert_eq!(
        repo.run(&[&args[..], &["--check"]].concat()).status.code(),
        Some(1)
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), stale);
    assert!(repo.run(&args).status.success());
    assert_eq!(fs::read_to_string(path).unwrap(), authored);
}

#[test]
fn json_diff_reports_exact_changes_without_writing() {
    let repo = Repository::new();
    repo.put("shared/tmux/keys.conf", "bind r refresh-client\n");
    let result = repo.run(&["docs", "--only", "keybinds", "--diff", "--json"]);
    assert!(result.status.success());
    assert!(!repo.root.join("docs").exists());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["version"], 1);
    assert_eq!(report["mode"], "diff");
    let changes = report["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 9);
    assert!(changes.iter().all(|change| {
        change["action"] == "create"
            && change["diff"]
                .as_str()
                .unwrap()
                .starts_with("--- /dev/null\n+++ b/docs/keybinds/")
    }));
    let second = repo.run(&["docs", "--only", "keybinds", "--diff", "--json"]);
    assert_eq!(second.stdout, result.stdout);
}

#[test]
fn keybinding_parse_failure_cannot_partially_update_cli_reference() {
    let repo = Repository::new();
    repo.put(
        "shared/nvim/lua/keys.lua",
        "vim.keymap.set('n', 'a', function( end)",
    );
    let result = repo.run(&["docs", "--only", "cli,keybinds"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("keys.lua"));
    assert!(!repo.root.join("docs").exists());
}

#[test]
fn malformed_managed_blocks_preserve_authored_content_and_other_outputs() {
    let repo = Repository::new();
    let path = "docs/cli/dotfile.md";
    let original = "# Authored\n<!-- cli:commands:start -->\nunfinished\n";
    repo.put(path, original);
    let result = repo.run(&["docs", "--only", "cli,keybinds"]);
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(repo.root.join(path)).unwrap(), original);
    assert!(!repo.root.join("docs/cli/_INDEX.md").exists());
    assert!(!repo.root.join("docs/keybinds").exists());
}

#[test]
fn missing_native_metadata_fails_check_without_launching_interpreters() {
    let repo = Repository::new();
    fs::create_dir_all(repo.root.join("scripts/rust/crates/count")).unwrap();
    repo.put("docs/cli/count.md", "retained\n");
    let result = repo.run(&["docs", "--only", "cli", "--check"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("count: command metadata unavailable")
    );
    assert!(!repo.root.join("docs/cli/dotfile.md").exists());
    assert!(repo.run(&["docs", "--only", "cli"]).status.success());
    assert_eq!(
        fs::read_to_string(repo.root.join("docs/cli/count.md")).unwrap(),
        "retained\n"
    );
}

#[test]
fn declared_tool_completion_needs_only_data_and_does_not_execute_the_tool() {
    let repo = Repository::new();
    repo.put("config/command-surface.json", r#"{"version":2,"commands":{"transcript":{"path":["transcript"],"help":"Archive","hidden":false,"params":[],"children":[{"path":["transcript","capture"],"help":"Capture","hidden":false,"params":[{"kind":"option","name":"provider","opts":["--provider"],"metavar":"PROVIDER","help":"Provider","multiple":false,"required":false,"hidden":false,"completion":{"kind":"call","source":"providers"}}],"children":[]}]}}}"#);
    let result = repo.run(&["completions", "--program", "transcript", "--shell", "zsh"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let script = String::from_utf8(result.stdout).unwrap();
    assert!(script.starts_with("#compdef transcript\n"));
    assert!(script.contains("transcript __complete providers"));
    assert!(!script.contains("dotfile __complete providers"));
    assert!(!script.contains("format:Format"));
    assert!(!repo.root.join("scripts/python").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_output_is_rejected_before_other_outputs_are_created() {
    use std::os::unix::fs::symlink;
    let repo = Repository::new();
    repo.put("outside.md", "private\n");
    fs::create_dir_all(repo.root.join("docs/keybinds")).unwrap();
    symlink(
        repo.root.join("outside.md"),
        repo.root.join("docs/keybinds/tmux.md"),
    )
    .unwrap();
    let result = repo.run(&["docs", "--only", "cli,keybinds"]);
    assert!(!result.status.success());
    assert_eq!(
        fs::read_to_string(repo.root.join("outside.md")).unwrap(),
        "private\n"
    );
    assert!(!repo.root.join("docs/cli").exists());
}

#[test]
fn benchmark_document_uses_stored_rust_records_without_running_benchmarks() {
    use hwtune::bench::{
        record::{Metric, Run},
        store::Store,
    };
    let repo = Repository::new();
    let store = Store::new(repo.root.join("benchmarks"));
    store
        .save_run(&Run {
            run_id: "sample".into(),
            host: "workstation".into(),
            started: "2026-01-01".into(),
            grade: "clean".into(),
            metrics: vec![Metric {
                key: "cpu.hash".into(),
                scale: "MB/s".into(),
                samples: vec![100.0, 102.0, 101.0],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
    let result = repo.run(&["docs", "--only", "benchmarks"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let path = Path::new("benchmarks/BENCHMARKS.md");
    let page = fs::read_to_string(repo.root.join(path)).unwrap();
    assert!(page.contains("`cpu.hash`"));
    assert!(page.contains("101.0"));
    assert!(
        repo.run(&["docs", "--only", "benchmarks", "--check"])
            .status
            .success()
    );
}
