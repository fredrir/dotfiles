#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use dotfmt_typescript::analyze::{self, Options, TypeDiagnostic};
use dotfmt_typescript::config::Config;
use dotfmt_typescript::dialect::Dialect;
use dotfmt_typescript::project;

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

#[test]
fn project_analysis_adds_unused_export_findings_without_type_awareness() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { other } from \"./b\";\nconsole.log(other);\n",
    );
    write(root, "src/b.ts", "export const other = 1;\n");
    project::clear_cache();

    let analysis = analyze::analyze(
        &root.join("src/a.ts"),
        "export const unused = 1;\n",
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: true,
            unsafe_fixes: false,
            type_aware: false,
            project_analysis: true,
            type_diagnostics: Vec::new(),
        },
    )
    .unwrap();
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "unused-export"),
        "{:?}",
        analysis.findings
    );
}

#[test]
fn type_aware_without_project_analysis_skips_unused_exports() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { other } from \"./b\";\nconsole.log(other);\n",
    );
    write(root, "src/b.ts", "export const other = 1;\n");
    project::clear_cache();

    // The supplied diagnostic stands in for a compiler run, keeping the test
    // independent of an installed `tsc`.
    let analysis = analyze::analyze(
        &root.join("src/a.ts"),
        "export const unused = 1;\n",
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: true,
            unsafe_fixes: false,
            type_aware: true,
            project_analysis: false,
            type_diagnostics: vec![TypeDiagnostic {
                code: 2322,
                message: "Type 'number' is not assignable to type 'string'.".to_owned(),
                start: 0,
                end: 6,
            }],
        },
    )
    .unwrap();
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "tsc-2322"),
        "{:?}",
        analysis.findings
    );
    assert!(
        analysis
            .findings
            .iter()
            .all(|finding| finding.code != "unused-export"),
        "{:?}",
        analysis.findings
    );
}
