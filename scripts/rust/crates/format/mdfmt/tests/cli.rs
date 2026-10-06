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
        let output = run(root.path(), &["--stdin", "a.md"], "text");
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
