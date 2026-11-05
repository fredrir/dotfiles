#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn fixes_remove_unused_imports_and_variables_and_preserve_side_effects() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript {\n width = 80\n type_aware = false\n tailwind = off\n}\n",
        ),
        (
            "a.ts",
            "import { unused } from \"./side\";\nconst dead = 42;\nexport const live = 1;\n",
        ),
    ]);
    let output = dotfmt(root.path()).args(["--fix", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    let text = fs::read_to_string(root.path().join("a.ts")).unwrap();
    assert!(text.contains("import \"./side\";"), "{text}");
    assert!(!text.contains("unused"), "{text}");
    assert!(!text.contains("dead"), "{text}");
    assert!(text.contains("export const live = 1;"), "{text}");
}

#[test]
fn fix_diff_does_not_write_and_diagnose_exits_one() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript {\n width = 80\n type_aware = false\n tailwind = off\n}\n",
        ),
        (
            "a.ts",
            "import { unused } from \"./side\";\nexport const live = 1;\n",
        ),
    ]);
    let original = fs::read_to_string(root.path().join("a.ts")).unwrap();
    let output = dotfmt(root.path()).args(["--fix", "--diff", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("-import { unused }"));
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        original
    );
    let output = dotfmt(root.path()).args(["--diagnose", "a.ts"]).run();
    assert_eq!(output.code(), Some(1), "{output:?}");
}
