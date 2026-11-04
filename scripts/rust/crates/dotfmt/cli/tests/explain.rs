#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use testkit::tree_pairs;

const CONFIG: &str = "json {}\nini {}\nmarkdown {}\nlua {\n  excluded_files {\n    skip.lua\n  }\n}\nexcluded_files {\n  vendor/\n}\n";

fn explain(root: &std::path::Path, args: &[&str]) -> String {
    let output = dotfmt(root).args(args).run();
    assert!(output.success(), "{args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    output.stdout
}

#[test]
fn explain_reports_language_dialect_and_configuration_chain() {
    let root = tree_pairs(&[
        (".config/dotfmt/dotfmt.dotfile", "{\n width = 100\n}\n"),
        ("dotfmt.dotfile", CONFIG),
        ("nested/dotfmt.dotfile", "json {\n  indent = 4\n}\n"),
    ]);
    let global = root.path().join(".config/dotfmt/dotfmt.dotfile");
    let canonical = root.path().canonicalize().unwrap();
    let local = canonical.join("dotfmt.dotfile");
    let nested = canonical.join("nested/dotfmt.dotfile");
    assert_eq!(
        explain(root.path(), &["--explain", "nested/a.jsonc"]),
        format!(
            "language json\ndialect  jsonc\nconfig   {}\nconfig   {}\nconfig   {}\n",
            global.display(),
            local.display(),
            nested.display()
        )
    );
    assert!(explain(root.path(), &["--explain", "a.json"]).starts_with("language json\ndialect  auto\n"));
    assert!(explain(root.path(), &["--explain", ".ssh/config"]).starts_with("language ini\ndialect  ssh\n"));
    assert!(explain(root.path(), &["--explain", "notes.md"]).starts_with("language markdown\ndialect  gfm\n"));
}

#[test]
fn explain_reports_why_a_file_is_skipped() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG)]);
    for (name, expected) in [
        ("vendor/a.json", "skipped  excluded\n"),
        ("skip.lua", "skipped  excluded\n"),
        ("a.unknown", "skipped  unmapped\n"),
        ("a.toml", "language toml\nskipped  disabled\n"),
    ] {
        assert!(
            explain(root.path(), &["--explain", name]).starts_with(expected),
            "{name}"
        );
    }
}

#[test]
fn explain_honors_forced_languages_and_dialects_and_reports_configuration_errors() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG)]);
    assert!(
        explain(root.path(), &["-l", "json", "--dialect", "jsonl", "--explain", "Untitled-1"])
            .starts_with("language json\ndialect  jsonl\n")
    );
    let output = dotfmt(root.path()).args(["-l", "toml", "--explain", "a.toml"]).run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stderr.contains("toml is not configured"), "{output:?}");
    let root = tree_pairs(&[("dotfmt.dotfile", "json {\n  bogus = 1\n}\n")]);
    let output = dotfmt(root.path()).args(["--explain", "a.json"]).run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("dotfmt.dotfile:2: unknown setting: bogus"), "{output:?}");
}
