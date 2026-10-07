#![forbid(unsafe_code)]

use mdfmt::config::Config;
use testkit::tree_pairs;

#[test]
fn gitignore_patterns_select_files_and_directories() {
    let cases = [
        ("/root.md", "root.md", false),
        ("/root.md", "sub/root.md", true),
        ("draft.md", "sub/draft.md", false),
        ("drafts/", "sub/drafts/note.md", false),
        ("drafts/", "drafts", true),
        ("docs/*.md", "docs/note.md", false),
        ("docs/*.md", "docs/deep/note.md", true),
        ("docs/**/*.md", "docs/deep/note.md", false),
        ("note?.[mM][dD]", "sub/note1.MD", false),
        ("*.md\n!keep.md", "keep.md", true),
        ("!keep.md\n*.md", "keep.md", false),
        ("drafts/\n!drafts/keep.md", "drafts/keep.md", false),
        ("drafts/*\n!drafts/keep.md", "drafts/keep.md", true),
        ("# comment", "note.md", true),
        (r"\#note.md", "#note.md", false),
        (r"\!note.md", "!note.md", false),
        ("note#1.md", "note#1.md", false),
        ("note=1.md", "note=1.md", false),
        ("my note.md", "my note.md", false),
        ("note.md   ", "note.md", false),
        ("note.md\\ ", "note.md ", false),
        (r"note\{", "note{", false),
        (r"\}", "}", false),
    ];
    for (pattern, name, allowed) in cases {
        let text = format!("blacklist {{\n{pattern}\n}}\n");
        let root = tree_pairs(&[("mdfmt.dotfile", &text)]);
        let config = Config::read(&root.path().join("mdfmt.dotfile")).unwrap();
        assert_eq!(
            config.files.allows(&root.path().join(name)),
            allowed,
            "{pattern:?}: {name}"
        );
    }
}

#[test]
fn whitelist_restricts_selection_and_blacklist_wins() {
    let root = tree_pairs(&[(
        "mdfmt.dotfile",
        r"
mdfmt {
  width = 80 # existing inline comments still work
}
whitelist {
  /README.md
  docs/
  !docs/private/
}
blacklist {
  generated.md
  docs/drafts/*
  !docs/drafts/keep.md
}
",
    )]);
    let config = Config::read(&root.path().join("mdfmt.dotfile")).unwrap();
    for (name, allowed) in [
        ("README.md", true),
        ("sub/README.md", false),
        ("docs/nested/note.md", true),
        ("docs/private/note.md", false),
        ("docs/generated.md", false),
        ("docs/drafts/note.md", false),
        ("docs/drafts/keep.md", true),
        ("other.md", false),
    ] {
        assert_eq!(
            config.files.allows(&root.path().join(name)),
            allowed,
            "{name}"
        );
    }
}

#[test]
fn empty_or_comment_only_lists_allow_all_files() {
    for text in [
        "",
        "whitelist {\n}\nblacklist {\n}\n",
        "whitelist {\n# docs/\n}\n",
    ] {
        let root = tree_pairs(&[("mdfmt.dotfile", text)]);
        let config = Config::read(&root.path().join("mdfmt.dotfile")).unwrap();
        assert!(config.files.allows(&root.path().join("any/note.md")));
    }
}

#[test]
fn invalid_patterns_report_the_config_and_line() {
    let root = tree_pairs(&[("mdfmt.dotfile", "blacklist {\n[z-a]\n}\n")]);
    let error = Config::read(&root.path().join("mdfmt.dotfile")).unwrap_err();
    assert!(error.contains("mdfmt.dotfile: line 2:"), "{error}");
}
