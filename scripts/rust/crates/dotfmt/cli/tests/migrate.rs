#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn migrates_json_prettier_options_and_plugins() {
    let root = tree_pairs(&[
        (
            "package.json",
            r#"{
              "prettier": {
                "printWidth": 100,
                "tabWidth": 4,
                "semi": false,
                "plugins": ["prettier-plugin-tailwindcss", "prettier-plugin-organize-imports"],
                "tailwindFunctions": ["cn", "tw"]
              }
            }"#,
        ),
        (
            ".prettierrc",
            r#"{"singleQuote": true, "trailingComma": "es5"}"#,
        ),
    ]);
    let output = dotfmt(root.path()).args(["--migrate", "prettier"]).run();
    assert!(output.success(), "{output:?}");
    for expected in [
        "width                 = 100",
        "indent                = 4",
        "quote_style           = single",
        "trailing_commas       = es5",
        "semicolons            = as-needed",
        "sort_imports          = true",
        "tailwind              = auto",
        "tailwind_functions    = \"cn, tw\"",
    ] {
        assert!(
            output.stdout.contains(expected),
            "missing {expected}: {output:?}"
        );
    }
}

const DEFAULT_LANGUAGES: [&str; 12] = [
    "bib", "conf", "env", "ignore", "ini", "json", "latex", "lua", "markdown", "plist", "toml",
    "yaml",
];

#[test]
fn migrated_config_keeps_the_default_languages_enabled() {
    let root = tree_pairs(&[
        ("package.json", r#"{"prettier":{"semi":false}}"#),
        ("a.json", "{\"a\":1}\n"),
        ("a.yaml", "a:   1\n"),
        ("a.toml", "a=1\n"),
        ("a.js", "const x=1\n"),
    ]);
    let output = dotfmt(root.path()).args(["--migrate", "prettier"]).run();
    assert!(output.success(), "{output:?}");
    for language in DEFAULT_LANGUAGES {
        assert!(
            output.stdout.contains(&format!("\n{language} {{}}\n")),
            "missing {language} block: {output:?}"
        );
    }
    assert!(!output.stdout.contains("javascript {"), "{output:?}");
    fs::write(root.path().join("dotfmt.dotfile"), &output.stdout).unwrap();

    for (name, language) in [("a.json", "json"), ("a.yaml", "yaml"), ("a.toml", "toml")] {
        let output = dotfmt(root.path()).args(["--explain", name]).run();
        assert!(output.success(), "{name}: {output:?}");
        assert!(
            output.stdout.starts_with(&format!("language {language}\n")),
            "{name}: {output:?}"
        );
        assert!(!output.stdout.contains("disabled"), "{name}: {output:?}");
    }

    let targets = ["a.json", "a.yaml", "a.toml"];
    let output = dotfmt(root.path()).args(["--check"]).args(targets).run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    let output = dotfmt(root.path()).args(targets).run();
    assert!(output.success(), "{output:?}");
    let output = dotfmt(root.path()).args(["--check"]).args(targets).run();
    assert!(output.success(), "{output:?}");

    let output = dotfmt(root.path()).args(["--explain", "a.js"]).run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("skipped  disabled"), "{output:?}");
}
