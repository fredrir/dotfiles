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
