#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn safe_fixes_keep_unused_locals_and_unsafe_fixes_remove_them() {
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
    assert!(text.contains("const dead = 42;"), "{text}");
    assert!(text.contains("export const live = 1;"), "{text}");

    let output = dotfmt(root.path()).args(["--fix-unsafe", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    let text = fs::read_to_string(root.path().join("a.ts")).unwrap();
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

fn private_package(config: &str, source: &str) -> tempfile::TempDir {
    tree_pairs(&[
        ("dotfmt.dotfile", config),
        ("package.json", r#"{"private":true}"#),
        ("a.ts", source),
        ("b.ts", "console.log(1);\n"),
    ])
}

#[test]
fn default_fix_unsafe_keeps_a_lone_export() {
    let root = private_package(
        "typescript {\n width = 80\n type_aware = false\n tailwind = off\n}\n",
        "export const publicApi = 1;\n",
    );
    let output = dotfmt(root.path()).args(["--fix-unsafe", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "export const publicApi = 1;\n"
    );
}

#[test]
fn explicit_export_selection_is_still_safe_mode_gated() {
    let root = private_package(
        "typescript {\n remove_unused_exports = true\n type_aware = false\n tailwind = off\n}\n",
        "export const publicApi = 1;\n",
    );
    let output = dotfmt(root.path()).args(["--fix", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "export const publicApi = 1;\n"
    );

    let output = dotfmt(root.path()).args(["--fix-unsafe", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    // Unsafe mode also selects unused locals, so the former export is removed
    // on the next pass.
    assert_eq!(fs::read_to_string(root.path().join("a.ts")).unwrap(), "");
}

#[test]
fn fix_unsafe_keeps_impure_locals_until_explicitly_enabled() {
    let config = "typescript {\n width = 80\n type_aware = false\n tailwind = off\n}\n";
    let root = tree_pairs(&[
        ("dotfmt.dotfile", config),
        ("a.ts", "const unused = bump();\nexport const live = 1;\n"),
    ]);
    let output = dotfmt(root.path()).args(["--fix-unsafe", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "const unused = bump();\nexport const live = 1;\n"
    );

    fs::write(
        root.path().join("dotfmt.dotfile"),
        "typescript {\n remove_impure_locals = true\n type_aware = false\n tailwind = off\n}\n",
    )
    .unwrap();
    let output = dotfmt(root.path()).args(["--fix", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "const unused = bump();\nexport const live = 1;\n"
    );

    let output = dotfmt(root.path()).args(["--fix-unsafe", "a.ts"]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.ts")).unwrap(),
        "export const live = 1;\n"
    );
}
