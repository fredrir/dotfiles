//! Black-box checks on the flags, output, prompts and exit codes callers
//! depend on.
//!
//! The tools themselves are stood in for by shell scripts on a `PATH` this
//! harness controls, so what is checked here is what this binary does — which
//! programs it runs, in which directory, with which arguments — and not
//! whether ruff happens to be installed on the machine running the tests.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// A run of the binary. `DOTFILE_ROOT` is always cleared first: the test
/// binary lives inside this repository, so a run that did not clear it would
/// find the real checkout by climbing out of `target/debug`.
fn format(args: &[&str], answers: &str, environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile-format"));
    command
        .args(args)
        .env("NO_COLOR", "1")
        .env("COLUMNS", "80")
        .env_remove("DOTFILE_ROOT");
    for (name, value) in environment {
        command.env(name, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dotfile-format runs");
    child
        .stdin
        .take()
        .expect("stdin is a pipe")
        .write_all(answers.as_bytes())
        .expect("the answers are read");
    child.wait_with_output().expect("dotfile-format finishes")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn tree(lines: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for line in lines {
        let (path, contents) = line.split_once('=').unwrap_or((line, ""));
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
    }
    root
}

fn at(root: &tempfile::TempDir, path: &str) -> String {
    root.path().join(path).display().to_string()
}

/// A stand-in for one of the providers, written into a directory that is
/// about to become the whole of `PATH`.
fn stub(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A stand-in that records the directory it ran in and the arguments it was
/// given, one line per invocation.
fn recorder(bin: &Path, name: &str) {
    stub(
        bin,
        name,
        &format!(r#"printf '%s|%s|%s\n' "{name}" "$PWD" "$*" >> "$DFF_LOG""#),
    );
}

/// A `PATH` holding only what a test put in it, plus git — which the walk
/// asks about ignored files and which nothing else here needs.
fn only(names: &[&str]) -> tempfile::TempDir {
    let bin = tempfile::tempdir().unwrap();
    let real = String::from_utf8(
        Command::new("/bin/sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    stub(bin.path(), "git", &format!(r#"exec {} "$@""#, real.trim()));
    for name in names {
        recorder(bin.path(), name);
    }
    bin
}

fn log(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A directory that satisfies the marker predicate, holding its own copies of
/// the configs so an assertion can tell them from the real ones.
fn checkout() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("environment")).unwrap();
    fs::create_dir_all(root.path().join("config")).unwrap();
    fs::write(root.path().join("config/targets.dotfile"), "").unwrap();
    let tools = root.path().join("shared/tools");
    fs::create_dir_all(&tools).unwrap();
    for name in [
        "dotfile.dotfile",
        "ruff.toml",
        "biome.global.json",
        "stylua.toml",
        "rustfmt.toml",
        ".taplo.toml",
        ".yamllint.yaml",
        ".sqlfluff",
        ".editorconfig",
    ] {
        fs::write(tools.join(name), format!("live {name}\n")).unwrap();
    }
    root
}

// ------------------------------------------------------------------- shape

#[test]
fn no_arguments_prints_the_help_on_stdout_and_succeeds() {
    let output = format(&[], "", &[]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage:"), "{}", stdout(&output));
    assert_eq!(stderr(&output), "");
}

/// clap hands a flattened struct's documentation to the command it lands in,
/// so the shared `--completions` flag can quietly become this tool's `about`.
#[test]
fn the_help_describes_this_tool_and_not_the_flags_it_shares() {
    let output = format(&["--help"], "", &[]);
    assert!(
        stdout(&output).starts_with("Format a tree with the tool that owns each language"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn completions_need_no_target() {
    let output = format(&["--completions", "zsh"], "", &[]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef dotfile-format"));
}

#[test]
fn asking_to_check_and_to_add_at_once_is_a_usage_error() {
    assert_eq!(
        format(&["--check", "--add", "."], "", &[]).status.code(),
        Some(2)
    );
    assert_eq!(
        format(&["--add", "--sync", "."], "", &[]).status.code(),
        Some(2)
    );
}

#[test]
fn a_target_that_is_not_there_is_an_error() {
    let output = format(&["/no/such/place/at/all"], "", &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
}

// ------------------------------------------------------------------ the run

/// The one that matters most on a machine with none of these tools installed:
/// a missing program is a fact in the report and never an exit code.
#[test]
fn with_only_git_on_the_path_a_python_tree_succeeds_and_names_ruff() {
    let root = tree(&["a.py=x = 1\n"]);
    let bin = only(&[]);
    let output = format(
        &[&at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stderr(&output).contains("ruff not installed"),
        "{}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "");
}

#[test]
fn a_check_run_writes_nothing_to_stdout() {
    let root = tree(&["a.py=x = 1\n", "b.lua=x\n"]);
    let bin = only(&["ruff", "stylua"]);
    let logged = root.path().join("log");
    let output = format(
        &["--check", &at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(!stderr(&output).is_empty());
}

/// Every child runs in the root with relative paths, which is how taplo finds
/// `.taplo.toml` and biome finds `biome.json` by their own upward search.
#[test]
fn each_tool_runs_in_the_root_and_is_given_relative_paths() {
    let root = tree(&["src/a.py=x\n", "deep/down/b.lua=x\n"]);
    let bin = only(&["ruff", "stylua"]);
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert!(output.status.success());
    let mut lines = log(&logged);
    lines.sort();
    let real = fs::canonicalize(root.path()).unwrap().display().to_string();
    assert_eq!(
        lines,
        [
            format!("ruff|{real}|format src/a.py"),
            format!("stylua|{real}|deep/down/b.lua"),
        ]
    );
}

/// Both rewrite the same file, so the order is not an implementation detail.
#[test]
fn the_programs_of_one_language_run_in_the_order_the_table_gives() {
    let root = tree(&["m.go=package main\n"]);
    let bin = only(&["goimports", "gofmt"]);
    let logged = root.path().join("log");
    format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    let order: Vec<String> = log(&logged)
        .iter()
        .map(|line| line.split('|').next().unwrap().to_string())
        .collect();
    assert_eq!(order, ["goimports", "gofmt"]);
}

#[test]
fn a_check_run_lints_and_a_write_run_does_not() {
    let root = tree(&["a.py=x\n"]);
    let bin = only(&["ruff"]);
    let logged = root.path().join("log");
    let environment = [
        ("PATH", bin.path().display().to_string()),
        ("DFF_LOG", logged.display().to_string()),
    ];
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();

    format(&[&at(&root, "")], "", &environment);
    assert_eq!(
        log(&logged)
            .iter()
            .map(|line| line.rsplit('|').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["format a.py"]
    );

    fs::remove_file(&logged).unwrap();
    format(&["--check", &at(&root, "")], "", &environment);
    assert_eq!(
        log(&logged)
            .iter()
            .map(|line| line.rsplit('|').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["format --check a.py", "check a.py"]
    );
}

#[test]
fn a_tool_reporting_drift_makes_a_check_run_exit_one() {
    let root = tree(&["a.py=x\n"]);
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'would reformat a.py' >&2; exit 1");
    let output = format(
        &["--check", &at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("findings"), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("would reformat a.py"),
        "{}",
        stderr(&output)
    );
}

/// The same drift in a write run is the run doing its job.
#[test]
fn a_write_run_succeeds_whatever_it_changed() {
    let root = tree(&["a.py=x\n"]);
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'reformatted a.py' >&2; exit 0");
    let output = format(
        &[&at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn a_tool_that_cannot_do_its_job_makes_a_write_run_exit_one() {
    let root = tree(&["a.py=x\n"]);
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'a.py: syntax error' >&2; exit 2");
    let output = format(
        &[&at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("failed"), "{}", stderr(&output));
}

/// `gofmt -l` exits 0 whether or not the files are formatted, so its silence
/// is the only thing that says they are.
#[test]
fn gofmt_naming_a_file_is_drift_even_though_it_exits_zero() {
    let root = tree(&["m.go=package main\n"]);
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "goimports", "exit 0");
    stub(bin.path(), "gofmt", "echo m.go; exit 0");
    let output = format(
        &["--check", &at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("findings"), "{}", stderr(&output));
}

/// The environment has to reach the child, and only that child.
#[test]
fn taplo_is_run_with_its_logging_turned_down_and_other_tools_are_not() {
    let root = tree(&["a.toml=x\n", "b.py=x\n"]);
    let bin = tempfile::tempdir().unwrap();
    for name in ["taplo", "ruff"] {
        stub(
            bin.path(),
            name,
            &format!(r#"printf '{name}=%s\n' "${{RUST_LOG:-unset}}" >> "$DFF_LOG""#),
        );
    }
    let logged = root.path().join("log");
    let output = format(
        &["--check", &at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
            // An inherited value must not survive into taplo.
            ("RUST_LOG", "debug"),
        ],
    );
    assert!(output.status.success());
    let mut lines = log(&logged);
    lines.sort();
    lines.dedup();
    assert_eq!(lines, ["ruff=debug", "taplo=warn"]);
}

/// The findings are what a person opened the report for. Echoing the file
/// list first buries them under thousands of characters on one line.
#[test]
fn a_check_run_names_the_command_without_repeating_every_path() {
    let files: Vec<String> = (0..40).map(|nth| format!("src/f{nth}.py=x\n")).collect();
    let root = tree(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'would reformat' >&2; exit 1");
    let environment = [("PATH", bin.path().display().to_string())];
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();

    let said = stderr(&format(&["--check", &at(&root, "")], "", &environment));
    assert!(
        said.contains("$ ruff format --check … (40 files)"),
        "{said}"
    );
    assert!(!said.contains("src/f7.py"), "{said}");
    // The tool's own words are what make a finding actionable, so they stay.
    assert!(said.contains("would reformat"), "{said}");

    // Seeing exactly what ran is what --verbose is for.
    let loud = stderr(&format(
        &["--check", "-v", &at(&root, "")],
        "",
        &environment,
    ));
    assert!(loud.contains("src/f7.py"), "{loud}");
}

/// Chunking is an `ARG_MAX` detail, not something a reader needs to see: 513
/// files is two command lines per step, and each step is still named once.
#[test]
fn a_step_that_took_several_command_lines_is_still_named_once() {
    let files: Vec<String> = (0..513).map(|nth| format!("f{nth}.py=x\n")).collect();
    let root = tree(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'would reformat' >&2; exit 1");
    let said = stderr(&format(
        &["--check", &at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    ));
    let named: Vec<&str> = said.lines().filter(|line| line.starts_with("$ ")).collect();
    assert_eq!(
        named,
        [
            "$ ruff format --check … (513 files)",
            "$ ruff check … (513 files)",
        ],
        "{said}"
    );
}

#[test]
fn a_quiet_run_says_nothing_anywhere() {
    let root = tree(&["a.py=x\n"]);
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'reformatted' >&2; exit 0");
    let output = format(
        &["-q", &at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "");
}

#[test]
fn a_file_named_outright_is_the_only_file_the_run_touches() {
    let root = tree(&["a.py=x\n", "b.py=y\n"]);
    let bin = only(&["ruff"]);
    let logged = root.path().join("log");
    format(
        &[&at(&root, "a.py")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    let lines = log(&logged);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with("|format a.py"), "{}", lines[0]);
}

#[test]
fn a_tree_holding_nothing_any_provider_owns_succeeds_and_runs_nothing() {
    let root = tree(&["README.md=hello\n", "LICENSE=text\n"]);
    let bin = only(&["ruff"]);
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert!(output.status.success());
    assert!(stderr(&output).contains("nothing to format"));
    assert!(log(&logged).is_empty());
}

/// A lockfile is machine-written; leaving it alone is the rule, and `-v` is
/// where the rule is visible rather than silent.
#[test]
fn a_lockfile_is_never_handed_to_a_tool_and_verbose_names_it() {
    let root = tree(&["app.json=x\n", "package-lock.json=y\n"]);
    let bin = only(&["biome"]);
    let logged = root.path().join("log");
    let output = format(
        &["-v", &at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    let lines = log(&logged);
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].ends_with("|format --write app.json"),
        "{}",
        lines[0]
    );
    assert!(
        stderr(&output).contains("1 generated lockfile left alone: package-lock.json"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn files_git_ignores_are_left_out() {
    let root = tree(&["kept.py=x\n", "built.py=y\n", ".gitignore=built.py\n"]);
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap();
    let bin = only(&["ruff"]);
    let logged = root.path().join("log");
    format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    let lines = log(&logged);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].ends_with("|format kept.py"), "{}", lines[0]);
}

// -------------------------------------------------------------- add and sync

fn root_of(checkout: &tempfile::TempDir) -> String {
    checkout.path().display().to_string()
}

#[test]
fn add_answering_yes_copies_the_config_and_answering_no_copies_nothing() {
    let repo = checkout();
    let project = tree(&["a.py=x\n"]);
    let output = format(
        &["--add", &at(&project, "")],
        "y\n",
        &[("DOTFILE_ROOT", &root_of(&repo))],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).contains("copy ruff.toml? [Y/n/a]"),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        fs::read_to_string(project.path().join("ruff.toml")).unwrap(),
        "live ruff.toml\n"
    );

    let other = tree(&["a.py=x\n"]);
    let output = format(
        &["--add", &at(&other, "")],
        "n\n",
        &[("DOTFILE_ROOT", &root_of(&repo))],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(!other.path().join("ruff.toml").exists());
}

/// A non-interactive `--add` copies nothing rather than everything, and that
/// is not a failure.
#[test]
fn add_with_no_answers_at_all_copies_nothing_and_succeeds() {
    let repo = checkout();
    let project = tree(&["a.py=x\n", "b.lua=x\n"]);
    let output = format(
        &["--add", &at(&project, "")],
        "",
        &[("DOTFILE_ROOT", &root_of(&repo))],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(!project.path().join("ruff.toml").exists());
    assert!(
        stderr(&output).contains("nothing copied"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn add_offers_a_config_only_for_a_language_the_project_uses() {
    let repo = checkout();
    let project = tree(&["a.py=x\n"]);
    let output = format(
        &["--add", &at(&project, "")],
        "a\n",
        &[("DOTFILE_ROOT", &root_of(&repo))],
    );
    assert!(output.status.success());
    assert!(project.path().join("ruff.toml").exists());
    assert!(!project.path().join("stylua.toml").exists());
    assert!(!project.path().join(".sqlfluff").exists());
}

/// A typo in the variable must not quietly become somebody else's checkout.
#[test]
fn add_with_a_dotfile_root_that_is_not_the_repository_exits_one_naming_the_variable() {
    let elsewhere = tempfile::tempdir().unwrap();
    let project = tree(&["a.py=x\n"]);
    let output = format(
        &["--add", &at(&project, "")],
        "",
        &[("DOTFILE_ROOT", &elsewhere.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("DOTFILE_ROOT"),
        "{}",
        stderr(&output)
    );
    assert!(!project.path().join("ruff.toml").exists());
}

#[test]
fn sync_never_asks_and_replaces_only_what_is_already_there() {
    let repo = checkout();
    let project = tree(&["a.py=x\n", "b.lua=x\n", "ruff.toml=stale\n"]);
    let output = format(
        &["--sync", &at(&project, "")],
        "",
        &[("DOTFILE_ROOT", &root_of(&repo))],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "");
    assert_eq!(
        fs::read_to_string(project.path().join("ruff.toml")).unwrap(),
        "live ruff.toml\n"
    );
    assert!(!project.path().join("stylua.toml").exists());
    assert!(
        stderr(&output).contains("replaced ruff.toml"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_marker_file_offers_a_config_the_walk_found_no_files_for() {
    let repo = checkout();
    let project = tree(&["go.mod=module x\n", "Cargo.toml=[package]\n"]);
    let output = format(
        &["--add", &at(&project, "")],
        "a\n",
        &[("DOTFILE_ROOT", &root_of(&repo))],
    );
    assert!(output.status.success());
    assert!(project.path().join("rustfmt.toml").exists());
    // Cargo.toml is a marker for Rust and a .toml file for taplo, and both
    // configs are wanted; nothing configures gofmt, so Go brings none.
    assert!(project.path().join(".taplo.toml").exists());
}

/// Run from outside any checkout, with a home that has no `dotfiles` in it,
/// every place to look comes up empty and the copies compiled in are all
/// there is — which is also what proves they are the repository's own bytes.
#[test]
fn with_no_checkout_to_be_found_the_copies_in_the_binary_are_the_source() {
    let away = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let project = tree(&["a.py=x\n"]);
    let moved = away.path().join("dotfile-format");
    fs::copy(env!("CARGO_BIN_EXE_dotfile-format"), &moved).unwrap();

    let mut child = Command::new(&moved)
        .args(["--add", &at(&project, "")])
        .current_dir(away.path())
        .env("NO_COLOR", "1")
        .env("COLUMNS", "80")
        .env("HOME", home.path())
        .env_remove("DOTFILE_ROOT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"y\n").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    assert!(
        stderr(&output).contains("built into this binary"),
        "{}",
        stderr(&output)
    );
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../shared/tools/ruff.toml")
        .canonicalize()
        .unwrap();
    assert_eq!(
        fs::read_to_string(project.path().join("ruff.toml")).unwrap(),
        fs::read_to_string(repo).unwrap()
    );
}
