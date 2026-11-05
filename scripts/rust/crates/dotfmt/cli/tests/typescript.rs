#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn formats_typescript_tsx_and_reports_diagnostics() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript {\n width = 80\n tailwind = off\n type_aware = false\n}\n",
        ),
        (
            "a.tsx",
            "import {useState} from \"react\";\nconst x={a:1,b:[2,3]};\nexport function C(){const [v,setV]=useState(0);return <div className=\"p-4 flex\">{v}</div>}\n",
        ),
    ]);
    let output = dotfmt(root.path()).arg("a.tsx").run();
    assert!(output.success(), "{output:?}");
    let formatted = fs::read_to_string(root.path().join("a.tsx")).unwrap();
    assert!(formatted.contains("import { useState } from \"react\";"));
    assert!(formatted.contains("const x = { a: 1, b: [2, 3] };"));
    assert!(formatted.contains("className=\"p-4 flex\""));

    let output = dotfmt(root.path())
        .args(["--diagnose", "--json", "a.tsx"])
        .run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stdout.contains("\"version\": 1"));
    assert!(output.stdout.contains("\"language\": \"typescript\""));
}

#[test]
fn extensionless_tsx_alias_formats_jsx_from_editor_stdin() {
    let root = tree_pairs(&[(
        "dotfmt.dotfile",
        "typescript {\n tailwind = off\n type_aware = false\n}\n",
    )]);
    let output = dotfmt(root.path())
        .args(["-e", "-l", "tsx"])
        .stdin("const App=()=> <div className=\"p-4 flex\">{value}</div>\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        "const App = () => <div className=\"p-4 flex\">{value}</div>;\n"
    );
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn extension_bearing_stdin_keeps_filename_dialect_over_alias() {
    let root = tree_pairs(&[(
        "dotfmt.dotfile",
        "typescript {\n tailwind = off\n type_aware = false\n}\n",
    )]);
    let input = "const App=()=> <div>{value}</div>\n";
    let output = dotfmt(root.path())
        .args(["-e", "-l", "tsx", "--stdin", "a.ts"])
        .stdin(input)
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, input);
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn editor_mode_passes_through_typescript_syntax_errors_quietly() {
    let root = tree_pairs(&[("dotfmt.dotfile", "typescript {}\n")]);
    let output = dotfmt(root.path())
        .args(["--editor", "a.tsx"])
        .stdin("const =")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "const =");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn fix_unsafe_does_not_delete_a_module_with_a_malformed_tsconfig() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "typescript {\n tailwind = off\n type_aware = false\n}\n",
        ),
        (
            "package.json",
            r#"{ "name": "bad-config", "private": true }"#,
        ),
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"] } }"#,
        ),
        (
            "src/lib.ts",
            "export const aliased = 1;\nexport const other = 2;\n",
        ),
        (
            "src/consumer.ts",
            "import { aliased } from \"@/lib\";\nconsole.log(aliased);\n",
        ),
    ]);
    let output = dotfmt(root.path())
        .args(["--fix-unsafe", "--json", "src"])
        .run();
    let lib = fs::read_to_string(root.path().join("src/lib.ts")).unwrap();
    assert_eq!(
        lib, "export const aliased = 1;\nexport const other = 2;\n",
        "a malformed tsconfig hides aliases, so no export may be deleted: {output:?}"
    );
    assert!(
        output.stdout.contains("\"fixes\": []"),
        "project fixes must be suppressed: {output:?}"
    );
}
