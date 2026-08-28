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

/// A stand-in for dotfmt that answers `--owns` by pattern and records every
/// other invocation the way the rest of the providers are recorded.
///
/// The protocol is the whole of what this crate knows about dotfmt's
/// selection rules: NUL-separated paths in, the owned subset NUL-separated
/// out.
fn owner(bin: &Path, pattern: &str) {
    stub(
        bin,
        "dotfmt",
        &format!(
            r#"PATH=/usr/bin:/bin
if [ "$1" = "--owns" ]; then tr '\0' '\n' | grep -E '{pattern}' | tr '\n' '\0'; exit 0; fi
printf '%s|%s|%s\n' "dotfmt" "$PWD" "$*" >> "$DFF_LOG""#
        ),
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
        "dotfmt.dotfile",
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

/// Every child runs in the root and is handed paths relative to it, which is
/// how the programs that resolve for themselves — shfmt's `.editorconfig`,
/// dotfmt's per-directory rules — find what they are looking for.
#[test]
fn each_tool_runs_in_the_root_and_is_given_relative_paths() {
    let repo = checkout();
    let root = tree(&["src/a.py=x\n", "deep/down/b.lua=x\n"]);
    let bin = only(&["ruff", "stylua"]);
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
            ("DOTFILE_ROOT", &root_of(&repo)),
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
            format!(
                "stylua|{real}|--config-path {} deep/down/b.lua",
                repo.path().join("shared/tools/stylua.toml").display()
            ),
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
    let environment = [("PATH", bin.path().display().to_string())];
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let output = format(&["--check", &at(&root, "")], "", &environment);
    assert_eq!(output.status.code(), Some(1));
    // The provider, the file it fell over on, and the count. Nothing else.
    assert_eq!(
        stderr(&output).trim_end(),
        "python  1 file  findings\n  a.py\n0 / 1 file clean"
    );
    // What ruff itself said is still a flag away.
    let loud = stderr(&format(
        &["--check", "-v", &at(&root, "")],
        "",
        &environment,
    ));
    assert!(loud.contains("would reformat a.py"), "{loud}");
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

/// The findings are what a person opened the report for. Echoing the command
/// and the file list first buries them under thousands of characters.
#[test]
fn a_check_run_reports_the_provider_and_not_the_command() {
    let files: Vec<String> = (0..40).map(|nth| format!("src/f{nth}.py=x\n")).collect();
    let root = tree(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'would reformat' >&2; exit 1");
    let environment = [("PATH", bin.path().display().to_string())];
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();

    // A run says which provider and how many files, and neither the command
    // nor a single path of the forty it was handed.
    // A provider that named no file at all still gets its first word shown,
    // because a finding nobody can act on needs a second run.
    let said = stderr(&format(&["--check", &at(&root, "")], "", &environment));
    assert_eq!(
        said.trim_end(),
        "python  40 files  findings\n  would reformat\n0 / 40 files clean"
    );

    // Seeing exactly what ran is what --verbose is for, and there the file
    // list stands in as a count until the flag asks for it in full.
    let some = stderr(&format(
        &["--check", "-v", &at(&root, "")],
        "",
        &environment,
    ));
    assert!(some.contains("$ ruff format --check"), "{some}");
    assert!(some.contains("src/f7.py"), "{some}");
    // The tool's own words are what make a finding actionable, so they stay.
    assert!(some.contains("would reformat"), "{some}");
}

/// Chunking is an `ARG_MAX` detail, not something a reader needs to see: 513
/// files is two command lines per step, and the report is the same three
/// words either way.
#[test]
fn a_step_that_took_several_command_lines_reads_as_one() {
    let files: Vec<String> = (0..513).map(|nth| format!("f{nth}.py=x\n")).collect();
    let root = tree(&files.iter().map(String::as_str).collect::<Vec<_>>());
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "ruff", "echo 'would reformat' >&2; exit 1");
    let said = stderr(&format(
        &["--check", &at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    ));
    assert_eq!(
        said.trim_end(),
        "python  513 files  findings\n  would reformat\n0 / 513 files clean"
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
    let repo = checkout();
    let root = tree(&["app.json=x\n", "package-lock.json=y\n"]);
    let bin = only(&["biome"]);
    let logged = root.path().join("log");
    let output = format(
        &["-v", &at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
            ("DOTFILE_ROOT", &repo.path().display().to_string()),
        ],
    );
    let lines = log(&logged);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0].rsplit('|').next().unwrap(),
        format!(
            "format --write --json-parse-allow-comments=true --config-path {} app.json",
            repo.path().join("shared/tools/biome.global.json").display()
        )
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

// ------------------------------------------------------------ dotfmt's row

/// The row is dotfmt's answer, not this crate's guess. `.conf` here is opted
/// in and `.dotfile` is not, which no extension list in this crate could have
/// produced — and that is the point: `dotfile format .` and `dotfmt .` cannot
/// disagree if only one of them decides.
#[test]
fn the_dotfmt_row_is_whatever_dotfmt_says_it_owns() {
    let root = tree(&["a.conf=x\n", "b.dotfile=x\n", "c.py=x\n"]);
    let bin = only(&["ruff"]);
    owner(bin.path(), r"\.conf$");
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let mut lines = log(&logged);
    lines.sort();
    let real = fs::canonicalize(root.path()).unwrap().display().to_string();
    assert_eq!(
        lines,
        [
            format!("dotfmt|{real}|a.conf"),
            format!("ruff|{real}|format c.py")
        ]
    );
}

/// dotfmt owns files by path as well as by extension, and a file with no
/// extension at all is the case the walk has to be able to offer.
#[test]
fn a_file_with_no_extension_reaches_dotfmt_when_dotfmt_claims_it() {
    let root = tree(&["ssh/config.d/10-work=x\n", "notes.md=x\n"]);
    let bin = only(&[]);
    owner(bin.path(), r"^ssh/");
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        log(&logged)
            .iter()
            .map(|line| line.rsplit('|').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["ssh/config.d/10-work"]
    );
}

/// Two providers rewriting one file is the race the whole design exists to
/// avoid, so a file dotfmt claims is a file no other row is handed.
#[test]
fn a_file_dotfmt_claims_is_given_to_nobody_else() {
    let root = tree(&["app.json=x\n"]);
    let bin = only(&["biome"]);
    owner(bin.path(), r"\.json$");
    let logged = root.path().join("log");
    format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert_eq!(
        log(&logged)
            .iter()
            .map(|line| line.split('|').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["dotfmt"]
    );
}

/// The same answer every other provider gets: a fact about this machine, not
/// an exit code, and nothing in the report to read past.
#[test]
fn dotfmt_missing_is_never_an_error_and_is_named_only_under_verbose() {
    let root = tree(&["a.conf=x\n", "b.py=x\n"]);
    let bin = only(&["ruff"]);
    let environment = [
        ("PATH", bin.path().display().to_string()),
        ("DFF_LOG", root.path().join("log").display().to_string()),
    ];
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();

    let output = format(&[&at(&root, "")], "", &environment);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stderr(&output).trim_end(), "1 / 1 file formatted");

    // The row is still there and still counts the files it would have had,
    // exactly the way a missing sqlfluff reads.
    let loud = stderr(&format(&["-v", &at(&root, "")], "", &environment));
    assert!(
        loud.contains("dotfmt  1 file   dotfmt not installed"),
        "{loud}"
    );
}

/// A dotfmt that cannot answer is a failure rather than a row that quietly
/// owns nothing: a run that formatted none of the `.conf` files while
/// reporting success is the bug this call was added to close.
#[test]
fn a_dotfmt_that_cannot_answer_owns_fails_the_run() {
    let root = tree(&["a.conf=x\n"]);
    let bin = only(&[]);
    stub(
        bin.path(),
        "dotfmt",
        "echo 'unexpected argument --owns' >&2; exit 2",
    );
    let output = format(
        &[&at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output).trim_end(),
        "dotfmt  1 file  failed\n  unexpected argument --owns\n0 / 1 file formatted"
    );
}

/// A dotfmt too old to know `--owns` still formats, so the row falls back to
/// the three extensions this crate has always used. An empty row would stop
/// formatting every `.conf` in the tree and read as a clean run.
#[test]
fn a_dotfmt_that_cannot_answer_owns_still_formats_by_extension() {
    let root = tree(&["a.conf=x\n", "b.config=x\n", "c.dotfile=x\n", "d.md=x\n"]);
    let bin = only(&[]);
    stub(
        bin.path(),
        "dotfmt",
        r#"PATH=/usr/bin:/bin
if [ "$1" = "--owns" ]; then echo "error: unexpected argument '--owns'" >&2; exit 2; fi
printf '%s|%s|%s\n' "dotfmt" "$PWD" "$*" >> "$DFF_LOG""#,
    );
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    // The files were still formatted...
    assert_eq!(
        log(&logged)
            .iter()
            .map(|line| line.rsplit('|').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["a.conf b.config c.dotfile"]
    );
    // ...and the run still says, loudly, that it guessed which ones.
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output).trim_end(),
        "dotfmt  3 files  failed\n  error: unexpected argument '--owns'\n0 / 3 files formatted"
    );
}

// ------------------------------------------------- files encrypted at rest

/// A document shaped the way SOPS really writes one.
fn sealed() -> String {
    "password: ENC[AES256_GCM,data:qWuPqA==,iv:wtj3wg=,tag:6rqTIQ==,type:str]\n\
     sops:\n    version: 3.13.3\n"
        .to_string()
}

/// The one that would have rewritten a secrets file. yamlfmt re-indents the
/// whole metadata block, which is a diff the size of the file at best and a
/// broken MAC at worst.
#[test]
fn no_provider_is_ever_handed_an_encrypted_file() {
    let root = tree(&["ci.yaml=a: 1\n", "app.json={}\n"]);
    fs::write(root.path().join("secrets.yaml"), sealed()).unwrap();
    fs::write(root.path().join("creds.json"), sealed()).unwrap();
    let bin = only(&["yamlfmt", "biome"]);
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let said: Vec<String> = log(&logged)
        .iter()
        .map(|line| line.rsplit('|').next().unwrap().to_string())
        .collect();
    // Both formatters ran, and neither was given the encrypted file that
    // would have landed in its row.
    assert!(said.contains(&"ci.yaml".to_string()), "{said:?}");
    assert!(
        said.iter()
            .any(|line| line.contains("app.json") && !line.contains("creds.json")),
        "{said:?}"
    );
    for line in &said {
        assert!(!line.contains("secrets.yaml"), "{said:?}");
        assert!(!line.contains("creds.json"), "{said:?}");
    }
}

/// Leaving one alone is the tool working as intended, so it reads like a
/// lockfile: a `--verbose` line rather than a warning.
#[test]
fn a_skipped_secret_is_named_under_verbose() {
    let root = tree(&["ci.yaml=a: 1\n"]);
    fs::write(root.path().join("secrets.yaml"), sealed()).unwrap();
    let bin = only(&["yamlfmt"]);
    let environment = [
        ("PATH", bin.path().display().to_string()),
        ("DFF_LOG", root.path().join("log").display().to_string()),
    ];
    let environment: Vec<(&str, &str)> = environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();

    let quiet = stderr(&format(&[&at(&root, "")], "", &environment));
    assert_eq!(quiet.trim_end(), "1 / 1 file formatted");

    let loud = stderr(&format(&["-v", &at(&root, "")], "", &environment));
    assert!(
        loud.contains("1 encrypted file left alone: secrets.yaml"),
        "{loud}"
    );
}

/// Naming one outright does not get past the rule either, and a run that
/// found nothing else to do says why rather than going quiet.
#[test]
fn naming_an_encrypted_file_outright_is_a_no_op_that_says_so() {
    let root = tree(&["ci.yaml=a: 1\n"]);
    fs::write(root.path().join("secrets.yaml"), sealed()).unwrap();
    let bin = only(&["yamlfmt"]);
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "secrets.yaml")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert!(output.status.success());
    assert!(log(&logged).is_empty(), "{:?}", log(&logged));
    assert!(
        stderr(&output).contains("1 encrypted file left alone: secrets.yaml"),
        "{}",
        stderr(&output)
    );
}

/// `.sops.yaml` is the configuration naming the keys to encrypt *with*. It is
/// not encrypted, it follows the naming convention anyway, and it must still
/// be formatted — which is why the rule reads the file rather than the name.
#[test]
fn the_sops_configuration_file_is_still_formatted() {
    let root = tree(&[".sops.yaml=creation_rules:\n  - age: age1qqq\n"]);
    let bin = only(&["yamlfmt"]);
    let logged = root.path().join("log");
    format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert_eq!(
        log(&logged)
            .iter()
            .map(|line| line.rsplit('|').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        [".sops.yaml"]
    );
}

// ------------------------------------------------------ the repo's settings

/// The regression that started this: nothing in this repository is called
/// `.taplo.toml` or `biome.json` where taplo and biome look, so both ran at
/// their own defaults over the whole tree and reported it clean.
#[test]
fn taplo_and_biome_are_pointed_at_this_repositorys_config() {
    let repo = checkout();
    let root = tree(&["a.toml=x\n", "b.json=x\n"]);
    let bin = only(&["taplo", "biome"]);
    let logged = root.path().join("log");
    format(
        &["--check", &at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
            ("DOTFILE_ROOT", &root_of(&repo)),
        ],
    );
    let tools = repo.path().join("shared/tools");
    let said: Vec<String> = log(&logged)
        .iter()
        .map(|line| line.rsplit('|').next().unwrap().to_string())
        .collect();
    assert!(
        said.contains(&format!(
            "fmt --check --config {} a.toml",
            tools.join(".taplo.toml").display()
        )),
        "{said:?}"
    );
    // biome reads no directory that does not hold a `biome.json`, and this
    // repository keeps its copy under a name that will not shadow a
    // project's own, so the file itself is what has to be named.
    assert!(
        said.contains(&format!(
            "format --json-parse-allow-comments=true --config-path {} b.json",
            tools.join("biome.global.json").display()
        )),
        "{said:?}"
    );
}

/// A project's own settings win, and per provider: this target keeps its
/// `.taplo.toml` and is still given biome's.
#[test]
fn a_target_with_its_own_config_keeps_it() {
    let repo = checkout();
    let root = tree(&["a.toml=x\n", "b.json=x\n", ".taplo.toml=mine\n"]);
    let bin = only(&["taplo", "biome"]);
    let logged = root.path().join("log");
    format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
            ("DOTFILE_ROOT", &root_of(&repo)),
        ],
    );
    let said: Vec<String> = log(&logged)
        .iter()
        .map(|line| line.rsplit('|').next().unwrap().to_string())
        .collect();
    assert!(
        said.contains(&"fmt .taplo.toml a.toml".to_string()),
        "{said:?}"
    );
    assert!(
        said.iter().any(|line| line.contains("--config-path")),
        "{said:?}"
    );
}

/// The case the plan said must keep working: `shared/nvim/.stylua.toml` and
/// `shared/wezterm/.stylua.toml` win inside their subtrees, and everything
/// else gets this repository's. `--config-path` outranks a nearer file, so
/// the two sets have to be two invocations.
#[test]
fn a_subtree_with_its_own_stylua_config_is_run_without_the_injected_one() {
    let repo = checkout();
    let root = tree(&[
        "nvim/.stylua.toml=mine\n",
        "nvim/init.lua=x\n",
        "yazi/init.lua=x\n",
    ]);
    let bin = only(&["stylua"]);
    let logged = root.path().join("log");
    let output = format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
            ("DOTFILE_ROOT", &root_of(&repo)),
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let mut said: Vec<String> = log(&logged)
        .iter()
        .map(|line| line.rsplit('|').next().unwrap().to_string())
        .collect();
    said.sort();
    assert_eq!(
        said,
        [
            format!(
                "--config-path {} yazi/init.lua",
                repo.path().join("shared/tools/stylua.toml").display()
            ),
            // The subtree that brought its own is run bare, so stylua finds it.
            "nvim/init.lua".to_string(),
        ]
    );
}

/// The YAML row runs two programs and `-c` is a flag for only one of them.
/// yamlfmt exits printing its usage when handed one it does not know, which
/// is how `-w` went unnoticed for so long.
#[test]
fn yamllint_is_given_the_config_and_yamlfmt_is_not() {
    let repo = checkout();
    let root = tree(&["ci.yaml=a: 1\n"]);
    let bin = only(&["yamlfmt", "yamllint"]);
    let logged = root.path().join("log");
    format(
        &["--check", &at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
            ("DOTFILE_ROOT", &root_of(&repo)),
        ],
    );
    let said: Vec<String> = log(&logged)
        .iter()
        .map(|line| {
            let mut parts = line.splitn(3, '|');
            format!("{} {}", parts.next().unwrap(), parts.nth(1).unwrap())
        })
        .collect();
    assert_eq!(
        said,
        [
            "yamlfmt -lint ci.yaml".to_string(),
            format!(
                "yamllint -c {} ci.yaml",
                repo.path().join("shared/tools/.yamllint.yaml").display()
            ),
        ]
    );
}

/// `-w` is not one of yamlfmt's flags. It exited 2 printing the usage text,
/// which is why no YAML in this repository had ever been formatted.
#[test]
fn yamlfmt_is_run_the_way_yamlfmt_writes_in_place() {
    let root = tree(&["ci.yaml=x\n"]);
    let bin = only(&["yamlfmt"]);
    let logged = root.path().join("log");
    format(
        &[&at(&root, "")],
        "",
        &[
            ("PATH", &bin.path().display().to_string()),
            ("DFF_LOG", &logged.display().to_string()),
        ],
    );
    assert_eq!(
        log(&logged)
            .iter()
            .map(|line| line.rsplit('|').next().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["ci.yaml"]
    );
}

// --------------------------------------------------------------- the report

/// The headline: a tree that is already formatted says so and says nothing
/// else.
#[test]
fn a_run_with_nothing_to_report_is_one_line() {
    let root = tree(&["a.py=x\n", "b.lua=x\n"]);
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
    assert_eq!(stderr(&output), "2 / 2 files formatted\n");
}

/// shfmt cannot parse `${{~var}}`, which one file in this repository uses. The
/// row failing is not actionable; the row naming the file is.
#[test]
fn a_provider_that_fell_over_on_one_file_names_that_file() {
    let root = tree(&["a.sh=x\n", "deep/b.sh=x\n", "deep/odd.zsh=x\n"]);
    let bin = tempfile::tempdir().unwrap();
    stub(
        bin.path(),
        "shfmt",
        "echo 'deep/odd.zsh:376:24: not a valid parameter expansion operator: `~`' >&2; exit 1",
    );
    let output = format(
        &[&at(&root, "")],
        "",
        &[("PATH", &bin.path().display().to_string())],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output).trim_end(),
        "shell  3 files  failed\n  deep/odd.zsh\n0 / 3 files formatted"
    );
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
