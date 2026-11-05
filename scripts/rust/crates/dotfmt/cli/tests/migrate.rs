#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
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
