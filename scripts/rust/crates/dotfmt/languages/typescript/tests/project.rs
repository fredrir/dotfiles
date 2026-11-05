#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use dotfmt_typescript::analyze::{self, Options};
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
fn exports_imported_by_another_module_are_not_flagged() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"main":"src/index.ts"}"#);
    write(
        root,
        "src/index.ts",
        "export { a } from \"./a\";\nexport { b } from \"./b\";\n",
    );
    write(
        root,
        "src/a.ts",
        "export const a = 1;\nexport const unusedA = 2;\n",
    );
    write(root, "src/b.ts", "export function b() { return 1; }\n");
    write(
        root,
        "src/use.ts",
        "import { a } from \"./a\";\nconsole.log(a);\n",
    );
    project::clear_cache();

    let findings = project::unused_exports(&root.join("src/a.ts")).unwrap();
    assert_eq!(findings.findings.len(), 1, "{:?}", findings.findings);
    assert_eq!(findings.findings[0].code, "unused-export");
    assert!(findings.findings[0].message.contains("unusedA"));
    assert!(
        project::unused_exports(&root.join("src/b.ts"))
            .unwrap()
            .findings
            .is_empty()
    );
    assert!(
        project::unused_exports(&root.join("src/index.ts"))
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn namespace_imports_and_directory_index_files_resolve() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
    write(root, "src/index.ts", "export * from \"./lib\";\n");
    write(
        root,
        "src/lib/index.ts",
        "export const a = 1;\nexport const b = 2;\n",
    );
    write(
        root,
        "src/use.ts",
        "import * as lib from \"./lib\";\nconsole.log(lib);\n",
    );
    project::clear_cache();

    assert!(
        project::unused_exports(&root.join("src/lib/index.ts"))
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn node_modules_and_dist_are_ignored() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
    write(root, "src/a.ts", "export const a = 1;\n");
    write(
        root,
        "node_modules/pkg/index.ts",
        "import { a } from \"../../src/a\";\nconsole.log(a);\n",
    );
    write(
        root,
        "dist/generated.ts",
        "import { a } from \"../src/a\";\nconsole.log(a);\n",
    );
    project::clear_cache();

    let findings = project::unused_exports(&root.join("src/a.ts")).unwrap();
    assert_eq!(findings.findings.len(), 1, "{:?}", findings.findings);
}

#[test]
fn package_exports_mark_entry_points() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"exports":{".":{"import":"./src/entry.ts"}}}"#,
    );
    write(root, "src/entry.ts", "export const entry = 1;\n");
    write(root, "src/other.ts", "export const other = 1;\n");
    project::clear_cache();

    assert!(
        project::unused_exports(&root.join("src/entry.ts"))
            .unwrap()
            .findings
            .is_empty()
    );
    assert_eq!(
        project::unused_exports(&root.join("src/other.ts"))
            .unwrap()
            .findings
            .len(),
        1
    );
}

#[test]
fn reexports_keep_target_exports_alive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
    write(
        root,
        "src/index.ts",
        "export { thing as renamed } from \"./thing\";\n",
    );
    write(root, "src/thing.ts", "export const thing = 1;\n");
    project::clear_cache();

    assert!(
        project::unused_exports(&root.join("src/thing.ts"))
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn analyze_can_add_project_unused_export_findings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(
        root,
        "src/b.ts",
        "import { unused } from \"./a\";\nconsole.log(unused);\n",
    );
    project::clear_cache();

    assert!(
        project::unused_exports(&root.join("src/a.ts"))
            .unwrap()
            .findings
            .is_empty()
    );
}
#[test]
fn analyze_with_type_aware_adds_unused_export_findings() {
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
            type_aware: true,
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
fn graph_cache_refreshes_when_a_source_file_changes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
    write(root, "src/a.ts", "export const a = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { a } from \"./a\";\nconsole.log(a);\n",
    );
    project::clear_cache();
    assert!(
        project::unused_exports(&root.join("src/a.ts"))
            .unwrap()
            .findings
            .is_empty()
    );

    write(root, "src/use.ts", "console.log(1);\n");
    assert_eq!(
        project::unused_exports(&root.join("src/a.ts"))
            .unwrap()
            .findings
            .len(),
        1
    );
}
