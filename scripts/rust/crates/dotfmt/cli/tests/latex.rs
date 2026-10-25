#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn tex_files_are_selected_configured_and_formatted() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "latex {}\n"),
        (
            "paper.tex",
            "\\begin{document}\n\\begin{itemize}\n\\item one\n\\end{itemize}\n\\end{document}\n",
        ),
        ("refs.bib", "@misc{k, title={T}}\n"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("paper.tex")).unwrap(),
        "\\begin{document}\n\\begin{itemize}\n  \\item one\n\\end{itemize}\n\\end{document}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("refs.bib")).unwrap(),
        "@misc{k, title={T}}\n"
    );
}

#[test]
fn latex_dialects_and_check_mode_work_through_the_cli() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "latex {}\n"),
        (
            "ctx.tex",
            "\\starttext\n\\startitemize\n\\item x\n\\stopitemize\n\\stoptext\n",
        ),
    ]);
    let output = dotfmt(root.path())
        .args(["-l", "latex", "--dialect", "context", "ctx.tex"])
        .run();
    assert!(output.success(), "{output:?}");
    assert!(
        fs::read_to_string(root.path().join("ctx.tex"))
            .unwrap()
            .contains("  \\item x")
    );
    let output = dotfmt(root.path())
        .args(["-l", "latex", "--dialect", "context", "--check", "ctx.tex"])
        .run();
    assert!(output.success(), "{output:?}");
    let output = dotfmt(root.path())
        .args(["-l", "latex", "--dialect", "unknown", "ctx.tex"])
        .run();
    assert_eq!(output.code(), Some(1), "{output:?}");
}

#[test]
fn stdin_editor_and_owns_use_the_tex_mapping() {
    let root = tree_pairs(&[("dotfmt.dotfile", "latex {}\n")]);
    let output = dotfmt(root.path())
        .args(["--stdin", "paper.tex"])
        .stdin("\\begin{document}\nx\n\\end{document}\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("\\begin{document}"));

    let output = dotfmt(root.path())
        .args(["-e", "paper.tex"])
        .stdin("\\begin{document}\nx\n\\end{document}\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("\\begin{document}"));

    let output = dotfmt(root.path())
        .args(["--owns", "-l", "latex"])
        .stdin("paper.tex\0refs.bib\0notes.md\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "paper.tex\0");
}

#[test]
fn unconfigured_latex_and_bib_are_actionable_errors() {
    let root = tree_pairs(&[
        ("a.tex", "x"),
        ("a.bib", "@misc{k}"),
        ("dotfmt.dotfile", "json {}\n"),
    ]);
    for (language, path) in [("latex", "a.tex"), ("bib", "a.bib")] {
        let output = dotfmt(root.path()).args(["-l", language, path]).run();
        assert_eq!(output.code(), Some(1), "{output:?}");
        assert!(output.stderr.contains("not configured"), "{output:?}");
    }
}

#[test]
fn editor_mode_passes_through_a_rejected_tex_document() {
    let root = tree_pairs(&[("dotfmt.dotfile", "latex {}\n")]);
    let deep = format!(
        "{}x\n{}",
        "\\begin{a}\n".repeat(120),
        "\\end{a}\n".repeat(120)
    );
    let output = dotfmt(root.path())
        .args(["-e", "deep.tex"])
        .stdin(&deep)
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, deep);
    let output = dotfmt(root.path())
        .args(["-e", "-v", "deep.tex"])
        .stdin(&deep)
        .run();
    assert_eq!(output.code(), Some(1), "{output:?}");
}
