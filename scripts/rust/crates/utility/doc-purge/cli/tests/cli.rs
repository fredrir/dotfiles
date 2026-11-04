#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use testkit::{Bin, TempDir, stderr, stdout, tree};

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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage: doc-purge"));
}

#[test]
fn a_dry_run_writes_nothing() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn a_type_filter_leaves_the_other_types_alone() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .args(["-t", "nope"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("unknown type: nope"));
    assert!(stderr(&output).contains("py"));
    assert!(stderr(&output).contains("css"));
}

#[test]
fn a_file_of_an_unread_type_is_reported() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path().join("nope"))
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
}

#[test]
fn a_tree_with_nothing_to_purge_says_so() {
    let root = tree(&["a.rs=let x = 1;\n"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
        let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
        let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
        let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
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
