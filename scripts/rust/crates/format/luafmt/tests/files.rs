#![forbid(unsafe_code)]

use luafmt::config::Config;
use testkit::tree_pairs;

#[test]
fn gitignore_patterns_select_files_and_directories() {
    let cases = [
        ("/root.lua", "root.lua", false),
        ("/root.lua", "sub/root.lua", true),
        ("draft.lua", "sub/draft.lua", false),
        ("drafts/", "sub/drafts/note.lua", false),
        ("drafts/", "drafts", true),
        ("docs/*.lua", "docs/note.lua", false),
        ("docs/*.lua", "docs/deep/note.lua", true),
        ("docs/**/*.lua", "docs/deep/note.lua", false),
        ("note?.[lL][uU][aA]", "sub/note1.LUA", false),
        ("*.lua\n!keep.lua", "keep.lua", true),
        ("!keep.lua\n*.lua", "keep.lua", false),
        ("drafts/\n!drafts/keep.lua", "drafts/keep.lua", false),
        ("drafts/*\n!drafts/keep.lua", "drafts/keep.lua", true),
        ("# comment", "note.lua", true),
        (r"\#note.lua", "#note.lua", false),
        (r"\!note.lua", "!note.lua", false),
        ("note#1.lua", "note#1.lua", false),
        ("note=1.lua", "note=1.lua", false),
        ("my note.lua", "my note.lua", false),
        ("note.lua   ", "note.lua", false),
        ("note.lua\\ ", "note.lua ", false),
        (r"note\{", "note{", false),
        (r"\}", "}", false),
    ];
    for (pattern, name, allowed) in cases {
        let text = format!("blacklist {{\n{pattern}\n}}\n");
        let root = tree_pairs(&[("luafmt.dotfile", &text)]);
        let config = Config::read(&root.path().join("luafmt.dotfile")).unwrap();
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
        "luafmt.dotfile",
        r"
luafmt {
  width = 80 # existing inline comments still work
}
whitelist {
  /README.lua
  docs/
  !docs/private/
}
blacklist {
  generated.lua
  docs/drafts/*
  !docs/drafts/keep.lua
}
",
    )]);
    let config = Config::read(&root.path().join("luafmt.dotfile")).unwrap();
    for (name, allowed) in [
        ("README.lua", true),
        ("sub/README.lua", false),
        ("docs/nested/note.lua", true),
        ("docs/private/note.lua", false),
        ("docs/generated.lua", false),
        ("docs/drafts/note.lua", false),
        ("docs/drafts/keep.lua", true),
        ("other.lua", false),
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
        let root = tree_pairs(&[("luafmt.dotfile", text)]);
        let config = Config::read(&root.path().join("luafmt.dotfile")).unwrap();
        assert!(config.files.allows(&root.path().join("any/note.lua")));
    }
}

#[test]
fn invalid_patterns_report_the_config_and_line() {
    let root = tree_pairs(&[("luafmt.dotfile", "blacklist {\n[z-a]\n}\n")]);
    let error = Config::read(&root.path().join("luafmt.dotfile")).unwrap_err();
    assert!(error.contains("luafmt.dotfile: line 2:"), "{error}");
}
