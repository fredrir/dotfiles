#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use testkit::{Bin, TempDir, stderr, stdout, tree};

/// A binary with colour off and an empty global config, so a developer's
/// `$XDG_CONFIG_HOME/doc-purge/doc-purge.dotfile` cannot change these tests.
fn bin() -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .env("NO_COLOR", "1")
        .env("XDG_CONFIG_HOME", empty_config())
}

fn empty_config() -> PathBuf {
    static DIR: OnceLock<TempDir> = OnceLock::new();
    DIR.get_or_init(|| TempDir::new().unwrap())
        .path()
        .to_path_buf()
}

fn sample() -> TempDir {
    tree(&[
        "a.rs=// note\nlet x = 1;\n",
        "b.py=x = 1  # note\n",
        "c.txt=# note\n",
    ])
}

fn read(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(name)).unwrap()
}

#[test]
fn bare_it_prints_help() {
    let output = bin().env("NO_COLOR", "1").stdin("").output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage: doc-purge"));
}

#[test]
fn a_dry_run_writes_nothing() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .arg("--dry")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("-1"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1  # note\n");
}

#[test]
fn answering_no_writes_nothing() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .env("NO_COLOR", "1")
        .stdin("n\n")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("cancelled"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn answering_yes_purges() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .env("NO_COLOR", "1")
        .stdin("\n")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn the_yes_flag_skips_the_prompt() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .arg("-y")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
}

#[test]
fn a_closed_stdin_writes_nothing() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn a_type_filter_leaves_the_other_types_alone() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .args(["-y", "-t", "py"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn a_type_filter_takes_a_leading_dot_and_a_language_name() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .args(["-y", "-t", ".py,rust"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn an_unknown_type_names_the_known_ones() {
    let root = sample();
    let output = bin()
        .arg(root.path())
        .args(["-t", "nope"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("unknown type: nope"));
    assert!(stderr(&output).contains("py"));
    assert!(stderr(&output).contains("css"));
}

#[test]
fn a_file_of_an_unread_type_is_reported() {
    let root = sample();
    let output = bin()
        .arg(root.path().join("c.txt"))
        .arg("--dry")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("not a file type doc-purge reads"));
    assert_eq!(read(root.path(), "c.txt"), "# note\n");
}

#[test]
fn a_missing_target_is_an_error() {
    let root = sample();
    let output = bin()
        .arg(root.path().join("nope"))
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("no such file or directory"));
}

#[test]
fn a_tree_with_nothing_to_purge_says_so() {
    let root = tree(&["a.rs=let x = 1;\n"]);
    let output = bin()
        .arg(root.path())
        .arg("--dry")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("nothing to purge"));
}

#[test]
fn file_permissions_survive_a_purge() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = sample();
        let target = root.path().join("a.rs");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        let output = bin()
            .arg(root.path())
            .arg("-y")
            .env("NO_COLOR", "1")
            .stdin("")
            .output();
        assert!(output.status.success());
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
}

#[test]
fn a_css_type_filter_covers_css_variations_but_not_preprocessors() {
    let root = tree(&[
        "a.css=/* note */\n.a { color: red; }\n",
        "b.module.css=/* note */\n.b { color: red; }\n",
        "c.pcss=/* note */\n.c { color: red; }\n",
        "i.postcss=/* note */\n.i { color: red; }\n",
        "d.scss=// note\n.d { color: red; }\n",
        "e.sass=// note\n.e\n  color: red\n",
        "f.less=// note\n.f { color: red; }\n",
        "g.styl=// note\n.g\n  color red\n",
        "h.sss=// note\n.h\n  color: red\n",
    ]);
    let output = bin()
        .arg(root.path())
        .args(["-y", "-t", "css"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.css"), ".a { color: red; }\n");
    assert_eq!(read(root.path(), "b.module.css"), ".b { color: red; }\n");
    assert_eq!(read(root.path(), "c.pcss"), ".c { color: red; }\n");
    assert_eq!(read(root.path(), "i.postcss"), ".i { color: red; }\n");
    assert_eq!(read(root.path(), "d.scss"), "// note\n.d { color: red; }\n");
    assert_eq!(read(root.path(), "e.sass"), "// note\n.e\n  color: red\n");
    assert_eq!(read(root.path(), "f.less"), "// note\n.f { color: red; }\n");
    assert_eq!(read(root.path(), "g.styl"), "// note\n.g\n  color red\n");
    assert_eq!(read(root.path(), "h.sss"), "// note\n.h\n  color: red\n");
}

#[test]
fn css_module_file_names_and_tailwind_names_select_css() {
    for (wanted, name) in [
        ("module.css", "a.module.css"),
        ("style.module.scss", "a.module.scss"),
        ("tailwindcss", "a.module.css"),
        ("css-modules", "a.module.css"),
    ] {
        let entry = format!("{name}=/* note */\n.a {{ color: red; }}\n");
        let root = tree(&[&entry]);
        let output = bin()
            .arg(root.path())
            .args(["-y", "-t", wanted])
            .env("NO_COLOR", "1")
            .stdin("")
            .output();
        assert!(output.status.success(), "type {wanted} failed");
        assert_eq!(
            read(root.path(), name),
            ".a { color: red; }\n",
            "type {wanted} did not read {name}"
        );
    }
}

#[test]
fn a_tailwind_stylesheet_keeps_its_directives() {
    let source = r#"/* note */
@import "tailwindcss";
@reference "../app.css";
@theme {
  --color-*: initial;
  --color-mint-500: oklch(0.72 0.11 178);
}
@custom-variant dark (&:where(.dark, .dark *));
@utility tab-4 { tab-size: 4; }
.button { @apply text-sm font-medium; }
"#;
    let root = tree(&[&format!("app.css={source}")]);
    let output = bin()
        .arg(root.path())
        .arg("-y")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(
        read(root.path(), "app.css"),
        r#"@import "tailwindcss";
@reference "../app.css";
@theme {
  --color-*: initial;
  --color-mint-500: oklch(0.72 0.11 178);
}
@custom-variant dark (&:where(.dark, .dark *));
@utility tab-4 { tab-size: 4; }
.button { @apply text-sm font-medium; }
"#
    );
}

#[test]
fn an_important_css_comment_is_reported_and_kept() {
    let root = tree(&["a.css=/*! keep */\n/* gone */\n.a { color: red; }\n"]);
    let output = bin()
        .arg(root.path())
        .arg("--dry")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("important comment"));
    assert_eq!(
        read(root.path(), "a.css"),
        "/*! keep */\n/* gone */\n.a { color: red; }\n"
    );
    let output = bin()
        .arg(root.path())
        .arg("-y")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(
        read(root.path(), "a.css"),
        "/*! keep */\n.a { color: red; }\n"
    );
}

fn javascript_tree() -> TempDir {
    tree(&[
        "a.js=// note\nconst a = 1;\n",
        "b.mjs=// note\nconst b = 1;\n",
        "c.cjs=// note\nconst c = 1;\n",
        "d.ts=// note\nconst d: number = 1;\n",
        "e.mts=// note\nconst e: number = 1;\n",
        "f.cts=// note\nconst f: number = 1;\n",
        "g.jsx=// note\nconst g = <div />;\n",
        "h.tsx=// note\nconst h = <div />;\n",
    ])
}

#[test]
fn every_javascript_extension_is_read_without_a_type_filter() {
    let root = javascript_tree();
    let output = bin()
        .arg(root.path())
        .arg("-y")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    for (name, expected) in [
        ("a.js", "const a = 1;\n"),
        ("b.mjs", "const b = 1;\n"),
        ("c.cjs", "const c = 1;\n"),
        ("d.ts", "const d: number = 1;\n"),
        ("e.mts", "const e: number = 1;\n"),
        ("f.cts", "const f: number = 1;\n"),
        ("g.jsx", "const g = <div />;\n"),
        ("h.tsx", "const h = <div />;\n"),
    ] {
        assert_eq!(read(root.path(), name), expected, "{name}");
    }
}

#[test]
fn javascript_type_filters_cover_the_expected_variations() {
    for (wanted, purged, kept) in [
        (
            "js",
            &["a.js", "b.mjs", "c.cjs"][..],
            &["d.ts", "e.mts", "f.cts", "g.jsx", "h.tsx"][..],
        ),
        (
            "ts",
            &["d.ts", "e.mts", "f.cts"][..],
            &["a.js", "b.mjs", "c.cjs", "g.jsx", "h.tsx"][..],
        ),
        (
            "tsx",
            &["h.tsx"][..],
            &["a.js", "b.mjs", "c.cjs", "d.ts", "e.mts", "f.cts", "g.jsx"][..],
        ),
        (
            "jsx",
            &["g.jsx", "h.tsx"][..],
            &["a.js", "b.mjs", "c.cjs", "d.ts", "e.mts", "f.cts"][..],
        ),
    ] {
        let root = javascript_tree();
        let output = bin()
            .arg(root.path())
            .args(["-y", "-t", wanted])
            .env("NO_COLOR", "1")
            .stdin("")
            .output();
        assert!(output.status.success(), "type {wanted} failed");
        for name in purged {
            assert!(
                !read(root.path(), name).starts_with("// note"),
                "type {wanted} did not read {name}"
            );
        }
        for name in kept {
            assert!(
                read(root.path(), name).starts_with("// note"),
                "type {wanted} read {name}"
            );
        }
    }
}

#[test]
fn check_exits_one_when_something_would_change() {
    let root = tree(&["a.rs=// note\nlet x = 1;\n"]);
    let output = bin().arg(root.path()).arg("--check").stdin("").output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("would purge"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn check_exits_zero_when_everything_is_clean() {
    let root = tree(&["a.rs=let x = 1;\n"]);
    let output = bin().arg(root.path()).arg("--check").stdin("").output();
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("nothing to purge"));
}

#[test]
fn stdin_writes_the_purged_source_to_stdout() {
    let output = bin()
        .args(["--stdin", "a.rs"])
        .stdin("// note\nlet x = 1;\n")
        .output();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "let x = 1;\n");
}

#[test]
fn stdin_never_writes_a_file() {
    let root = tree(&["a.rs=// note\nlet x = 1;\n"]);
    let output = bin()
        .current_dir(root.path())
        .args(["--stdin", "a.rs"])
        .stdin("// note\nlet x = 1;\n")
        .output();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "let x = 1;\n");
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn stdin_honours_check() {
    let output = bin()
        .args(["--stdin", "a.rs", "--check"])
        .stdin("// note\n")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty());
}

#[test]
fn stdin_needs_a_known_file_type() {
    let output = bin().args(["--stdin", "a.zzz"]).stdin("// note\n").output();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("not a file type doc-purge reads"));
}

#[test]
fn json_emits_file_objects_and_a_summary() {
    let root = tree(&["a.rs=// note\nlet x = 1;\n", "b.py=x = 1\n"]);
    let output = bin()
        .arg(root.path())
        .args(["--dry", "--json"])
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(0));
    let lines: Vec<serde_json::Value> = stdout(&output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let files: Vec<&serde_json::Value> =
        lines.iter().filter(|line| line["type"] == "file").collect();
    assert_eq!(files.len(), 2);
    let changed = files
        .iter()
        .find(|line| line["path"].as_str().unwrap().ends_with("a.rs"))
        .unwrap();
    assert_eq!(changed["status"], "changed");
    assert_eq!(changed["comments"].as_u64(), Some(1));
    assert!(changed["changes"].is_array());
    let summary = lines.iter().find(|line| line["type"] == "summary").unwrap();
    assert_eq!(summary["files"].as_u64(), Some(2));
    assert_eq!(summary["changed"].as_u64(), Some(1));
}

#[test]
fn list_types_prints_every_known_extension() {
    let output = bin().arg("--list-types").stdin("").output();
    assert!(output.status.success());
    let text = stdout(&output);
    for (name, extension) in [("rust", "rs"), ("python", "py"), ("css", "css")] {
        assert!(
            text.lines().any(|line| line.starts_with(name)
                && line.split_whitespace().any(|part| part == extension)),
            "missing {name} {extension} in:\n{text}"
        );
    }
}

#[test]
fn an_unknown_only_kind_is_a_usage_error() {
    let output = bin().args(["--only", "nope"]).stdin("").output();
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("invalid value 'nope'"));
    assert!(stderr(&output).contains("comments"));
}

#[test]
fn uncovered_lists_extensions_no_descriptor_claims() {
    let root = tree(&[
        "a.rs=let x = 1;\n",
        "one.zzz=\n",
        "two.zzz=\n",
        "three.qqq=\n",
    ]);
    let output = bin().arg(root.path()).arg("--uncovered").stdin("").output();
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    let zzz = text
        .lines()
        .find(|line| line.ends_with("zzz"))
        .expect("zzz is uncovered");
    assert!(zzz.trim_start().starts_with('2'), "count first: {zzz}");
    assert!(text.lines().any(|line| line.ends_with("qqq")));
    assert!(!text.lines().any(|line| line.ends_with("rs")));
}

#[test]
fn a_disabled_language_is_left_alone() {
    let root = tree(&[
        "doc-purge.dotfile=python {\n  enabled = false\n}\n",
        "a.py=# note\nx = 1\n",
        "b.rs=// note\nlet x = 1;\n",
    ]);
    let output = bin().arg(root.path()).arg("-y").stdin("").output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.py"), "# note\nx = 1\n");
    assert_eq!(read(root.path(), "b.rs"), "let x = 1;\n");
}

#[test]
fn a_gitignore_is_honoured() {
    let root = tree(&[
        ".gitignore=ignored.rs\n",
        "ignored.rs=// note\nlet x = 1;\n",
        "kept.rs=// note\nlet y = 2;\n",
    ]);
    let output = bin().arg(root.path()).arg("-y").stdin("").output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "ignored.rs"), "// note\nlet x = 1;\n");
    assert_eq!(read(root.path(), "kept.rs"), "let y = 2;\n");
}

#[test]
fn generated_files_are_left_alone() {
    let root = tree(&[
        "auto.rs=// @generated\n// note\nlet x = 1;\n",
        "out.min.js=// note\nconst x = 1;\n",
    ]);
    let output = bin().arg(root.path()).arg("-y").stdin("").output();
    assert!(output.status.success());
    assert_eq!(
        read(root.path(), "auto.rs"),
        "// @generated\n// note\nlet x = 1;\n"
    );
    assert_eq!(read(root.path(), "out.min.js"), "// note\nconst x = 1;\n");
}

#[test]
fn diff_prints_a_unified_diff_and_writes_nothing() {
    let root = tree(&["a.rs=// note\nlet x = 1;\n"]);
    let output = bin().arg(root.path()).arg("--diff").stdin("").output();
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("--- a/"));
    assert!(text.contains("+++ b/"));
    assert!(text.contains("@@ -1,2 +1,1 @@"));
    assert!(text.contains("-// note"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn editor_passes_input_through_on_an_unknown_type() {
    let output = bin()
        .args(["--editor", "a.zzz"])
        .stdin("// note\n")
        .output();
    assert!(output.status.success());
    assert_eq!(stdout(&output), "// note\n");
}

#[test]
fn an_unknown_config_key_names_the_line() {
    let root = tree(&[
        "doc-purge.dotfile={\n  bogus = true\n}\n",
        "a.rs=let x = 1;\n",
    ]);
    let output = bin().arg(root.path()).arg("-y").stdin("").output();
    assert_eq!(output.status.code(), Some(2));
    let errors = stderr(&output);
    assert!(errors.contains("doc-purge.dotfile:2:"), "{errors}");
    assert!(
        errors.contains("unknown global setting 'bogus'"),
        "{errors}"
    );
}

#[test]
fn owns_prints_only_the_files_doc_purge_claims() {
    let root = tree(&["a.rs=let x = 1;\n", "b.zzz=x\n", "out.min.js=x\n"]);
    let input = format!(
        "{}\0{}\0{}\0{}\0",
        root.path().join("a.rs").display(),
        root.path().join("b.zzz").display(),
        root.path().join("out.min.js").display(),
        // An editor may ask about a file before its first save.
        root.path().join("new.rs").display(),
    );
    let output = bin().arg("--owns").stdin(&input).output();
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        format!(
            "{}\n{}\n",
            root.path().join("a.rs").display(),
            root.path().join("new.rs").display(),
        )
    );
}

#[test]
fn only_comments_leaves_doc_strings_and_glyphs_alone() {
    let root = tree(&["a.py=\"\"\"Module docs.\"\"\"\nx = \"“quoted”\"  # note\n"]);
    let output = bin()
        .arg(root.path())
        .args(["--only", "comments", "-y"])
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(
        read(root.path(), "a.py"),
        "\"\"\"Module docs.\"\"\"\nx = \"“quoted”\"\n"
    );
}

#[test]
fn no_glyphs_leaves_the_glyph() {
    let root = tree(&["a.py=x = \"“quoted”\"  # note\n"]);
    let output = bin()
        .arg(root.path())
        .args(["--no-glyphs", "-y"])
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.py"), "x = \"“quoted”\"\n");
}

#[test]
fn keep_docstrings_leaves_the_doc_string() {
    let root = tree(&["a.py=\"\"\"Module docs.\"\"\"\nx = 1  # note\n"]);
    let output = bin()
        .arg(root.path())
        .args(["--keep-docstrings", "-y"])
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(
        read(root.path(), "a.py"),
        "\"\"\"Module docs.\"\"\"\nx = 1\n"
    );
}

#[test]
fn explain_names_the_marker_and_stays_silent_about_purged_comments() {
    let root = tree(&[
        "a.rs=// SAFETY: valid\n// note\nlet x = 1;\n",
        "b.rs=// SAFETY: also valid\nlet y = 2;\n",
    ]);
    let output = bin().arg(root.path()).arg("--explain").stdin("").output();
    assert!(output.status.success());
    let text = stdout(&output);
    assert_eq!(text.matches("safety:").count(), 2, "{text}");
    assert!(!text.contains("note:"), "{text}");
}
