#![forbid(unsafe_code)]

use std::{fs, path::Path};
use testkit::{Bin, Ran, tree_pairs};

fn run(root: &Path, args: &[&str], input: &str) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_mdfmt"))
        .args(args)
        .current_dir(root)
        .plain()
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .stdin(input)
        .run()
}

#[test]
fn stdin_emits_only_markdown_and_check_does_not_emit_or_write() {
    let root = tree_pairs(&[]);
    for args in [vec![], vec!["-"], vec!["--stdin", "readme.md"]] {
        let output = run(root.path(), &args, "# Title\ntext\n");
        assert_eq!(output.code(), Some(0), "{}", output.stderr);
        assert_eq!(output.stdout, "# Title\n\ntext");
        assert_eq!(output.stderr, "");
    }
    let check = run(root.path(), &["--check", "-"], "# Title\ntext\n");
    assert_eq!(check.code(), Some(1));
    assert_eq!(check.stdout, "");
}

#[test]
fn editor_mode_accepts_bundled_flags_and_keeps_reports_off_stdout_and_stderr() {
    let root = tree_pairs(&[]);
    for args in [
        vec!["-eq"],
        vec!["--editor"],
        vec!["-ev"],
        vec!["-eq", "--stdin", "note.md"],
    ] {
        let output = run(root.path(), &args, "## Title\nbody\n");
        assert_eq!(output.code(), Some(0), "{}", output.stderr);
        assert_eq!(output.stdout, "## Title\n\nbody");
        assert_eq!(output.stderr, "");
    }
    assert_eq!(run(root.path(), &["-e", "note.md"], "").code(), Some(1));
}

#[test]
fn implicit_stdin_check_matches_explicit_stdin_and_never_walks_the_directory() {
    let root = tree_pairs(&[("unformatted.md", "# Heading\nbody\n")]);
    let clean = run(root.path(), &["--check"], "already formatted");
    assert_eq!(clean.code(), Some(0));
    assert_eq!(clean.stdout, "");
    assert_eq!(
        run(root.path(), &["--check"], "# Title\nbody\n").code(),
        Some(1)
    );
    assert_eq!(
        run(root.path(), &["-eq", "--check"], "# Title\nbody\n").code(),
        Some(1)
    );
    let verbose = run(root.path(), &["-v", "-"], "text");
    assert_eq!(verbose.stdout, "text");
    assert!(verbose.stderr.contains("config"));
    assert!(verbose.stderr.contains("formatted 0 of 1 file"));
}

#[test]
fn explicit_stdin_and_file_targets_can_be_combined_like_jqfmt() {
    let root = tree_pairs(&[("a.md", "# File\nbody\n")]);
    let output = run(root.path(), &["-", "a.md"], "# Stdin\nbody\n");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, "# Stdin\n\nbody");
    assert_eq!(
        fs::read_to_string(root.path().join("a.md")).unwrap(),
        "# File\n\nbody"
    );
}

#[test]
fn dialect_auto_detects_vaults_and_explicit_choices_override_config() {
    let root = tree_pairs(&[
        ("vault/.obsidian/app.json", "{}"),
        ("mdfmt.dotfile", "mdfmt {\ndialect = auto\n}"),
    ]);
    let callout = "> [!custom]\n> # Heading\n> text";
    assert_eq!(
        run(root.path(), &["--stdin", "vault/notes/note.md"], callout).stdout,
        callout
    );
    assert_ne!(
        run(
            root.path(),
            &["--dialect", "gfm", "--stdin", "vault/note.md"],
            callout
        )
        .stdout,
        callout
    );
    fs::write(
        root.path().join("mdfmt.dotfile"),
        "mdfmt {\ndialect = obsidian\n}",
    )
    .unwrap();
    assert_eq!(run(root.path(), &["-"], callout).stdout, callout);
    assert_ne!(
        run(root.path(), &["--dialect", "commonmark", "-"], callout).stdout,
        callout
    );
    assert_eq!(
        run(root.path(), &["--dialect", "unknown", "-"], "").code(),
        Some(2)
    );
    for dialect in [
        "gfm",
        "github",
        "github-flavored-markdown",
        "obsidian",
        "obsidian-markdown",
        "auto",
        "commonmark",
    ] {
        assert_eq!(
            run(root.path(), &["--dialect", dialect, "-"], "text").code(),
            Some(0),
            "{dialect}"
        );
    }
}

#[test]
fn a_mixed_tree_selects_the_dialect_per_file() {
    let root = tree_pairs(&[
        ("vault/.obsidian/app.json", "{}"),
        ("vault/note.md", "> [!custom]\n> # Heading\n> text\n"),
        ("readme.md", "> [!custom]\n> # Heading\n> text\n"),
    ]);
    let output = run(root.path(), &["."], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert_eq!(
        fs::read_to_string(root.path().join("vault/note.md")).unwrap(),
        "> [!custom]\n> # Heading\n> text"
    );
    assert_ne!(
        fs::read_to_string(root.path().join("readme.md")).unwrap(),
        "> [!custom]\n> # Heading\n> text"
    );
}

#[test]
fn directory_formatting_is_scoped_and_check_is_read_only() {
    let input = "# Title\ntext\n";
    let root = tree_pairs(&[
        ("a.md", input),
        ("sub/b.MARKDOWN", input),
        ("keep.txt", input),
        ("target/skip.md", input),
    ]);
    let check = run(root.path(), &["--check", "."], "");
    assert_eq!(check.code(), Some(1), "{}", check.stderr);
    assert_eq!(fs::read_to_string(root.path().join("a.md")).unwrap(), input);
    let write = run(root.path(), &["."], "");
    assert_eq!(write.code(), Some(0), "{}", write.stderr);
    for name in ["a.md", "sub/b.MARKDOWN"] {
        assert_eq!(
            fs::read_to_string(root.path().join(name)).unwrap(),
            "# Title\n\ntext"
        );
    }
    for name in ["keep.txt", "target/skip.md"] {
        assert_eq!(fs::read_to_string(root.path().join(name)).unwrap(), input);
    }
    assert_eq!(run(root.path(), &["--check", "."], "").code(), Some(0));
}

#[test]
fn nearest_config_overrides_global_and_invalid_config_never_writes() {
    let root = tree_pairs(&[
        (
            "config/mdfmt/mdfmt.dotfile",
            "mdfmt {\nfinal_newline = true\n}\n",
        ),
        ("sub/mdfmt.dotfile", "mdfmt {\nheading_blank_lines = 2\n}\n"),
        ("bad/mdfmt.dotfile", "mdfmt {\nwidht = 20\n}\n"),
        ("bad/a.md", "# Hi\ntext\n"),
    ]);
    assert_eq!(
        run(root.path(), &["--stdin", "a.md"], "text").stdout,
        "text\n"
    );
    assert_eq!(
        run(root.path(), &["--stdin", "sub/deep/a.md"], "# Hi\ntext").stdout,
        "# Hi\n\n\ntext"
    );
    let output = run(root.path(), &["bad/a.md"], "");
    assert_eq!(output.code(), Some(1));
    assert!(
        output.stderr.contains("unknown setting: widht"),
        "{}",
        output.stderr
    );
    assert_eq!(
        fs::read_to_string(root.path().join("bad/a.md")).unwrap(),
        "# Hi\ntext\n"
    );
}

#[test]
fn invalid_values_and_io_failures_are_reported() {
    for setting in [
        "width = -1",
        "heading_blank_lines = 4",
        "table_style = wrap",
        "list_marker = x",
        "final_newline = yes",
    ] {
        let text = format!("mdfmt {{\n{setting}\n}}");
        let root = tree_pairs(&[("mdfmt.dotfile", &text)]);
        let output = run(root.path(), &["--stdin", "a.md"], "");
        assert_eq!(output.code(), Some(1), "{setting}");
        assert!(
            output.stderr.contains("mdfmt.dotfile: line 2:"),
            "{}",
            output.stderr
        );
        assert_eq!(output.stdout, "");
    }
    let root = tree_pairs(&[]);
    assert_eq!(run(root.path(), &["absent.md"], "").code(), Some(1));
    fs::write(root.path().join("binary.md"), [0xff, 0xfe]).unwrap();
    assert_eq!(run(root.path(), &["binary.md"], "").code(), Some(1));
    assert_eq!(
        fs::read(root.path().join("binary.md")).unwrap(),
        [0xff, 0xfe]
    );
}

#[test]
fn help_completions_and_command_dump_use_shared_cli() {
    let root = tree_pairs(&[]);
    for (args, expected) in [
        (vec!["--help"], "Usage: mdfmt"),
        (vec!["--completions", "zsh"], "#compdef mdfmt"),
        (vec!["--command-dump"], "\"version\":1"),
    ] {
        let output = run(root.path(), &args, "");
        assert_eq!(output.code(), Some(0), "{}", output.stderr);
        assert!(output.stdout.contains(expected), "{}", output.stdout);
    }
}

#[cfg(unix)]
#[test]
fn atomic_replacement_preserves_permissions_and_explicit_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tree_pairs(&[("a.md", "# Hi\ntext\n")]);
    fs::set_permissions(root.path().join("a.md"), fs::Permissions::from_mode(0o640)).unwrap();
    symlink("a.md", root.path().join("link.md")).unwrap();
    assert_eq!(run(root.path(), &["link.md"], "").code(), Some(0));
    assert!(root.path().join("link.md").is_symlink());
    assert_eq!(
        fs::metadata(root.path().join("a.md"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.md")).unwrap(),
        "# Hi\n\ntext"
    );
}
