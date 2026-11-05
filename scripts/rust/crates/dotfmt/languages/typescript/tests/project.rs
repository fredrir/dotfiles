#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use dotfmt_core::edit::Analysis;
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

    let analysis = analyze::analyze(
        &root.join("src/a.ts"),
        "export const unused = 1;\n",
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: true,
            unsafe_fixes: false,
            type_aware: true,
            organize: true,
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

fn unused_codes(path: &Path) -> Vec<String> {
    project::unused_exports(path)
        .unwrap()
        .findings
        .into_iter()
        .map(|finding| finding.message)
        .collect()
}

#[cfg(unix)]
fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn dotted_specifiers_try_appended_source_extensions() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/x.repo.ts", "export const repo = 1;\n");
    write(root, "src/x.types.ts", "export const types = 1;\n");
    write(root, "src/x.banner.ts", "export const banner = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { repo } from \"./x.repo\";\nimport type { types } from \"./x.types\";\nimport { banner } from \"./x.banner\";\nconsole.log(repo, banner);\n",
    );

    for file in ["src/x.repo.ts", "src/x.types.ts", "src/x.banner.ts"] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn tsconfig_paths_alias_resolves_dotted_specifiers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
    );
    write(
        root,
        "src/feature/explorer.repo.ts",
        "export const explorer = 1;\n",
    );
    write(
        root,
        "src/use.ts",
        "import { explorer } from \"@/feature/explorer.repo\";\nconsole.log(explorer);\n",
    );

    assert_eq!(
        unused_codes(&root.join("src/feature/explorer.repo.ts")),
        Vec::<String>::new()
    );
}

#[test]
fn tsconfig_base_url_resolves_bare_specifiers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":"src"}}"#,
    );
    write(root, "src/sub/deep.ts", "export const deep = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { deep } from \"sub/deep\";\nconsole.log(deep);\n",
    );

    assert_eq!(
        unused_codes(&root.join("src/sub/deep.ts")),
        Vec::<String>::new()
    );
}

#[test]
fn config_dir_placeholder_expands_to_the_root_config_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "configs/base.json",
        r#"{"compilerOptions":{"paths":{"@/*":["${configDir}/src/*"]}}}"#,
    );
    write(
        root,
        "tsconfig.json",
        r#"{"extends":"./configs/base.json"}"#,
    );
    write(root, "src/a.ts", "export const a = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { a } from \"@/a\";\nconsole.log(a);\n",
    );

    assert_eq!(unused_codes(&root.join("src/a.ts")), Vec::<String>::new());
}

#[test]
fn dynamic_import_keeps_exports_alive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/lazy.ts", "export const lazy = 1;\n");
    write(
        root,
        "src/use.ts",
        "const load = () => import(\"./lazy\");\nconsole.log(load);\n",
    );

    assert_eq!(
        unused_codes(&root.join("src/lazy.ts")),
        Vec::<String>::new()
    );
}

#[test]
fn require_keeps_exports_alive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/req.ts", "export const req = 1;\n");
    write(
        root,
        "src/use.ts",
        "const value = require(\"./req\");\nconsole.log(value);\n",
    );

    assert_eq!(unused_codes(&root.join("src/req.ts")), Vec::<String>::new());
}

#[test]
fn import_meta_glob_keeps_matched_exports_alive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/pages/home.ts", "export const home = 1;\n");
    write(root, "src/pages/about.ts", "export const about = 1;\n");
    write(
        root,
        "src/use.ts",
        "const modules = import.meta.glob(\"./pages/*.ts\");\nconsole.log(modules);\n",
    );

    for file in ["src/pages/home.ts", "src/pages/about.ts"] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn import_meta_glob_expands_brace_alternatives() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/a.ts", "export const a = 1;\n");
    write(root, "src/b.ts", "export const b = 1;\n");
    write(root, "src/c.ts", "export const c = 1;\n");
    write(
        root,
        "src/use.ts",
        "const modules = import.meta.glob(\"./{a,b}.ts\");\nconsole.log(modules);\n",
    );

    assert!(project::graph_complete(&root.join("src/a.ts")));
    for file in ["src/a.ts", "src/b.ts"] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
    assert!(!unused_codes(&root.join("src/c.ts")).is_empty());
}

#[test]
fn import_meta_glob_expands_brace_alternatives_with_wildcards() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/lib/home.ts", "export const home = 1;\n");
    write(root, "src/lib/about.tsx", "export const about = 1;\n");
    write(root, "src/lib/other.js", "export const other = 1;\n");
    write(
        root,
        "src/use.ts",
        "const modules = import.meta.glob(\"./lib/*.{ts,tsx}\");\nconsole.log(modules);\n",
    );

    assert!(project::graph_complete(&root.join("src/lib/home.ts")));
    for file in ["src/lib/home.ts", "src/lib/about.tsx"] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
    assert!(!unused_codes(&root.join("src/lib/other.js")).is_empty());
}

#[test]
fn negated_glob_patterns_block_project_fixes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(root, "tsconfig.json", "{}");
    write(root, "src/lib/home.ts", "export const home = 1;\n");
    write(
        root,
        "src/use.ts",
        "const modules = import.meta.glob([\"./lib/*.ts\", \"!./lib/skip.ts\"]);\nconsole.log(modules);\n",
    );

    assert!(!project::graph_complete(&root.join("src/lib/home.ts")));
    assert!(
        project::unused_exports(&root.join("src/lib/home.ts"))
            .unwrap()
            .fixes
            .is_empty()
    );
}

#[test]
fn gitignored_consumers_keep_exports_alive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(root, "tsconfig.json", "{}");
    write(root, ".gitignore", "src/use.ts\n");
    write(root, "src/home.ts", "export const home = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { home } from \"./home\";\nconsole.log(home);\n",
    );
    assert_eq!(unused_codes(&root.join("src/home.ts")), Vec::<String>::new());
    assert!(project::graph_complete(&root.join("src/home.ts")));
}

#[test]
fn non_literal_dynamic_imports_block_project_fixes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(root, "tsconfig.json", "{}");
    write(root, "src/home.ts", "export const home = 1;\n");
    write(
        root,
        "src/use.ts",
        "const name = \"./home\";\nconst module = import(name);\nconsole.log(module);\n",
    );
    assert!(!project::graph_complete(&root.join("src/home.ts")));
    assert!(project::unused_exports(&root.join("src/home.ts"))
        .unwrap()
        .fixes
        .is_empty());
}

#[test]
fn unsupported_glob_patterns_block_project_fixes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(root, "tsconfig.json", "{}");
    write(root, "src/lib/home.ts", "export const home = 1;\n");
    write(
        root,
        "src/use.ts",
        "const modules = import.meta.glob(\"./lib/[ab].ts\");\nconsole.log(modules);\n",
    );
    assert!(!project::graph_complete(&root.join("src/lib/home.ts")));
    assert!(project::unused_exports(&root.join("src/lib/home.ts"))
        .unwrap()
        .fixes
        .is_empty());
}

#[test]
fn declaration_files_with_mts_cts_and_tsx_extensions_are_exempt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(
        root,
        "src/decl.d.mts",
        "export declare const declared: number;\n",
    );
    write(
        root,
        "src/decl.d.cts",
        "export declare const declared: number;\n",
    );
    write(
        root,
        "src/decl.d.tsx",
        "export declare const declared: number;\n",
    );
    write(root, "src/use.ts", "console.log(1);\n");

    for file in ["src/decl.d.mts", "src/decl.d.cts", "src/decl.d.tsx"] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn config_files_are_entry_points() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "vite.config.ts", "export default { plugins: [] };\n");
    write(
        root,
        "src/eslint.config.mjs",
        "export default [{ rules: {} }];\n",
    );
    write(
        root,
        "tailwind.config.ts",
        "export default { content: [] };\n",
    );
    write(root, "src/use.ts", "console.log(1);\n");

    for file in [
        "vite.config.ts",
        "src/eslint.config.mjs",
        "tailwind.config.ts",
    ] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn vscode_extension_entry_is_mapped_from_dist_to_src() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"engines":{"vscode":"^1.80.0"},"main":"./dist/extension.js"}"#,
    );
    write(
        root,
        "src/extension.ts",
        "export function activate() { return 1; }\n",
    );
    write(root, "src/other.ts", "export const other = 1;\n");

    assert_eq!(
        unused_codes(&root.join("src/extension.ts")),
        Vec::<String>::new()
    );
    assert_eq!(unused_codes(&root.join("src/other.ts")).len(), 1);
}

#[test]
fn framework_routes_and_main_files_are_entry_points() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(
        root,
        "app/page.tsx",
        "export default function Page() { return null; }\n",
    );
    write(
        root,
        "app/routes/home.tsx",
        "export default function Home() { return null; }\n",
    );
    write(
        root,
        "src/routes/+page.ts",
        "export const load = () => 1;\n",
    );
    write(
        root,
        "src/pages/api.ts",
        "export const handler = () => 1;\n",
    );
    write(root, "src/main.ts", "export const main = 1;\n");
    write(root, "src/use.ts", "console.log(1);\n");

    for file in [
        "app/page.tsx",
        "app/routes/home.tsx",
        "src/routes/+page.ts",
        "src/pages/api.ts",
        "src/main.ts",
    ] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn test_story_and_bench_files_are_entry_points() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/a.test.ts", "export const testHelper = 1;\n");
    write(root, "src/b.spec.ts", "export const specHelper = 1;\n");
    write(
        root,
        "src/c.stories.tsx",
        "export const Story = () => null;\n",
    );
    write(root, "src/d.bench.ts", "export const bench = () => 1;\n");
    write(root, "src/use.ts", "console.log(1);\n");

    for file in [
        "src/a.test.ts",
        "src/b.spec.ts",
        "src/c.stories.tsx",
        "src/d.bench.ts",
    ] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn nested_package_entry_points_are_exempt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(
        root,
        "packages/ui/package.json",
        r#"{"main":"./dist/index.js"}"#,
    );
    write(
        root,
        "packages/ui/src/index.ts",
        "export const index = 1;\n",
    );
    write(
        root,
        "packages/ui/src/other.ts",
        "export const other = 1;\n",
    );

    assert_eq!(
        unused_codes(&root.join("packages/ui/src/index.ts")),
        Vec::<String>::new()
    );
    assert_eq!(
        unused_codes(&root.join("packages/ui/src/other.ts")).len(),
        1
    );
}

#[test]
fn component_consumers_keep_exports_alive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/a.ts", "export const a = 1;\n");
    write(root, "src/b.ts", "export const b = 1;\n");
    write(root, "src/c.ts", "export const c = 1;\n");
    write(root, "src/d.ts", "export const d = 1;\n");
    write(
        root,
        "src/App.vue",
        "<script setup lang=\"ts\">\nimport { a } from \"./a\";\nconsole.log(a);\n</script>\n<template><div /></template>\n",
    );
    write(
        root,
        "src/Widget.svelte",
        "<script>\nimport { b } from \"./b\";\nconsole.log(b);\n</script>\n",
    );
    write(
        root,
        "src/Page.astro",
        "---\nimport { c } from \"./c\";\nconsole.log(c);\n---\n<div />\n",
    );
    write(
        root,
        "src/Doc.mdx",
        "import { d } from \"./d\";\n\n# Doc {d}\n",
    );
    write(root, "src/use.ts", "console.log(1);\n");

    for file in ["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
}

#[test]
fn package_imports_subpath_resolves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r##"{"imports":{"#internal/*":"./src/internal/*"}}"##,
    );
    write(root, "tsconfig.json", "{}");
    write(root, "src/internal/thing.ts", "export const thing = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { thing } from \"#internal/thing\";\nconsole.log(thing);\n",
    );

    assert_eq!(
        unused_codes(&root.join("src/internal/thing.ts")),
        Vec::<String>::new()
    );
}

#[cfg(unix)]
#[test]
fn workspace_package_import_keeps_exports_alive_through_node_modules_symlink() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "package.json", r#"{"private":true}"#);
    write(
        root,
        "apps/app/src/use.ts",
        "import { helper } from \"@repo/ui/src/internal/helper\";\nconsole.log(helper);\n",
    );
    write(root, "packages/ui/package.json", r#"{"name":"@repo/ui"}"#);
    write(
        root,
        "packages/ui/src/internal/helper.ts",
        "export const helper = 1;\n",
    );
    fs::create_dir_all(root.join("node_modules/@repo")).unwrap();
    symlink(root.join("packages/ui"), root.join("node_modules/@repo/ui")).unwrap();

    assert_eq!(
        unused_codes(&root.join("packages/ui/src/internal/helper.ts")),
        Vec::<String>::new()
    );
    assert!(project::graph_complete(
        &root.join("packages/ui/src/internal/helper.ts")
    ));
}

#[test]
fn graph_is_incomplete_when_an_import_cannot_be_resolved() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/a.ts", "export const a = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { a } from \"./a\";\nimport { missing } from \"./missing\";\nconsole.log(a, missing);\n",
    );
    assert!(!project::graph_complete(&root.join("src/a.ts")));

    write(root, "src/missing.ts", "export const missing = 1;\n");
    project::invalidate_root(root);
    assert!(project::graph_complete(&root.join("src/a.ts")));
}

#[test]
fn graph_is_incomplete_when_a_source_file_does_not_parse() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/a.ts", "export const a = 1;\n");
    write(root, "src/broken.ts", "export const = ;\n");

    assert!(!project::graph_complete(&root.join("src/a.ts")));
}

#[cfg(unix)]
#[test]
fn tsc_runs_once_for_many_files() {
    if std::env::var_os("DOTFMT_TSC").is_some() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/m0.ts", "const x: number = \"s\";\n");
    let mut files = vec![root.join("src/m0.ts")];
    for index in 1..25 {
        let path = format!("src/m{index}.ts");
        write(root, &path, &format!("export const m{index} = {index};\n"));
        files.push(root.join(path));
    }
    let marker = root.join("calls");
    let arguments = root.join("arguments");
    write_script(
        &root.join("node_modules/.bin/tsc"),
        &format!(
            "echo call >> '{}'\necho \"$@\" >> '{}'\necho 'src/m0.ts(1,7): error TS2322: Type mismatch'",
            marker.display(),
            arguments.display()
        ),
    );
    let tsconfig = root.join("tsconfig.json");

    for file in &files {
        let diagnostics = project::type_diagnostics_for(file, root, &tsconfig).unwrap();
        if file.ends_with("m0.ts") {
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        } else {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
        }
    }
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1);
    let arguments = fs::read_to_string(&arguments).unwrap();
    assert!(arguments.contains("--noEmit"), "{arguments}");
    assert!(arguments.contains("--locale en"), "{arguments}");
    assert!(arguments.contains("--project"), "{arguments}");
}

#[cfg(unix)]
#[test]
fn concurrent_tsc_requests_share_one_run() {
    if std::env::var_os("DOTFMT_TSC").is_some() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    write(&root, "tsconfig.json", "{}");
    write(&root, "src/a.ts", "const x: number = \"s\";\n");
    let marker = root.join("calls");
    write_script(
        &root.join("node_modules/.bin/tsc"),
        &format!(
            "echo call >> '{}'\nsleep 0.2\necho 'src/a.ts(1,7): error TS2322: Type mismatch'",
            marker.display()
        ),
    );
    let tsconfig = root.join("tsconfig.json");
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let root = root.clone();
            let tsconfig = tsconfig.clone();
            scope.spawn(move || {
                assert_eq!(
                    project::type_diagnostics(&root, &tsconfig).unwrap().len(),
                    1
                );
            });
        }
    });
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1);
}

#[cfg(unix)]
#[test]
fn tsc_invalidates_after_a_source_change() {
    if std::env::var_os("DOTFMT_TSC").is_some() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/m0.ts", "const x: number = \"s\";\n");
    let marker = root.join("calls");
    write_script(
        &root.join("node_modules/.bin/tsc"),
        &format!(
            "echo call >> '{}'\necho 'src/m0.ts(1,7): error TS2322: Type mismatch'",
            marker.display()
        ),
    );
    let tsconfig = root.join("tsconfig.json");

    assert_eq!(project::type_diagnostics(root, &tsconfig).unwrap().len(), 1);
    std::thread::sleep(std::time::Duration::from_millis(10));
    write(root, "src/m0.ts", "const x: number = \"changed value\";\n");
    assert_eq!(project::type_diagnostics(root, &tsconfig).unwrap().len(), 1);
    assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 2);
}

#[test]
fn compiler_discovery_prefers_package_compiler_then_bin_then_native_preview() {
    if std::env::var_os("DOTFMT_TSC").is_some() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();

    write(root, "node_modules/typescript/lib/tsc.js", "");
    let compiler = project::resolve_compiler(root);
    assert_eq!(compiler.program, std::path::PathBuf::from("node"));
    assert!(
        compiler.arguments[0].ends_with("node_modules/typescript/lib/tsc.js"),
        "{compiler:?}"
    );

    fs::remove_file(root.join("node_modules/typescript/lib/tsc.js")).unwrap();
    write_user_script(&root.join("node_modules/.bin/tsc"));
    fs::create_dir_all(root.join("sub")).unwrap();
    let compiler = project::resolve_compiler(&root.join("sub"));
    assert!(
        compiler.program.ends_with("node_modules/.bin/tsc"),
        "{compiler:?}"
    );

    fs::remove_file(root.join("node_modules/.bin/tsc")).unwrap();
    write_user_script(&root.join("node_modules/@typescript/native-preview/bin/tsgo"));
    let compiler = project::resolve_compiler(root);
    assert!(
        compiler
            .program
            .ends_with("node_modules/@typescript/native-preview/bin/tsgo"),
        "{compiler:?}"
    );
}

fn write_user_script(path: &Path) {
    write_script(path, "exit 1");
}

#[test]
fn unused_tsc_codes_are_classified_as_warnings() {
    for code in [6133, 6192, 6196, 6198, 6199, 6205] {
        assert!(project::is_unused_diagnostic(code), "{code}");
        assert_eq!(
            format!("{:?}", project::type_diagnostic_severity(code)),
            "Warning"
        );
    }
    assert!(!project::is_unused_diagnostic(2322));
    assert_eq!(
        format!("{:?}", project::type_diagnostic_severity(2322)),
        "Error"
    );
}

fn private_project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "package.json", r#"{"private":true}"#);
    write(
        directory.path(),
        "tsconfig.json",
        r#"{"compilerOptions":{}}"#,
    );
    directory
}

fn apply_fix(text: &str, analysis: &Analysis) -> String {
    let fix = analysis.fixes.first().expect("an unsafe export fix");
    dotfmt_core::edit::apply(text, &fix.edits).unwrap()
}

fn assert_parses_and_formats(path: &Path, text: &str) {
    analyze::analyze(
        path,
        text,
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: false,
            unsafe_fixes: false,
            type_aware: false,
            organize: false,
            project_analysis: false,
            type_diagnostics: Vec::new(),
        },
    )
    .expect("fixed text re-parses");
    dotfmt_typescript::format(path, text, &Config::default(), Dialect::Auto)
        .expect("formatting the fixed text does not remove code");
}

#[test]
fn removes_unused_export_keyword_for_single_declaration() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(root, "src/use.ts", "console.log(1);\n");
    let path = root.join("src/a.ts");
    let input = "export const unused = 1;\n";

    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
    let fix = &analysis.fixes[0];
    assert_eq!(fix.title, "Remove unused export `unused`");
    assert_eq!(fix.kind, "quickfix");
    assert_eq!(fix.applicability, dotfmt_core::edit::Applicability::Unsafe);
    let finding = fix.finding.as_ref().expect("fix finding");
    assert_eq!(finding.start, analysis.findings[0].start);
    assert_eq!(finding.end, analysis.findings[0].end);

    let edited = apply_fix(input, &analysis);
    assert_eq!(edited, "const unused = 1;\n");
    assert_parses_and_formats(&path, &edited);
}

#[test]
fn removes_only_the_unused_specifier_from_a_local_export() {
    let directory = private_project();
    let root = directory.path();
    write(
        root,
        "src/a.ts",
        "const a = 1;\nconst b = 2;\nexport { a, b };\n",
    );
    write(
        root,
        "src/use.ts",
        "import { b } from \"./a\";\nconsole.log(b);\n",
    );
    let path = root.join("src/a.ts");

    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert_eq!(
        analysis.findings[0].message,
        "Export `a` is never imported in this project"
    );
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
    let edited = apply_fix("const a = 1;\nconst b = 2;\nexport { a, b };\n", &analysis);
    let formatted =
        dotfmt_typescript::format(&path, &edited, &Config::default(), Dialect::Auto).unwrap();
    assert_eq!(formatted, "const a = 1;\nconst b = 2;\nexport { b };\n");
    assert_parses_and_formats(&path, &edited);
}

#[test]
fn deletes_a_local_export_statement_when_every_specifier_is_unused() {
    let directory = private_project();
    let root = directory.path();
    write(
        root,
        "src/a.ts",
        "const a = 1;\nconst b = 2;\nexport { a, b };\n",
    );
    write(root, "src/use.ts", "console.log(1);\n");
    let path = root.join("src/a.ts");
    let input = "const a = 1;\nconst b = 2;\nexport { a, b };\n";

    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 2, "{:?}", analysis.findings);
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
    let edited = apply_fix(input, &analysis);
    assert_eq!(edited, "const a = 1;\nconst b = 2;\n");
    assert_parses_and_formats(&path, &edited);
}

#[test]
fn removes_export_default_prefix_for_named_function_and_class() {
    let directory = private_project();
    let root = directory.path();
    write(
        root,
        "src/fn.ts",
        "export default function f() { return 1; }\n",
    );
    write(root, "src/class.ts", "export default class C {}\n");
    write(root, "src/use.ts", "console.log(1);\n");

    let function = project::unused_exports(&root.join("src/fn.ts")).unwrap();
    assert_eq!(function.findings.len(), 1, "{:?}", function.findings);
    assert_eq!(
        function.findings[0].message,
        "Export `default` is never imported in this project"
    );
    assert_eq!(function.fixes.len(), 1, "{:?}", function.fixes);
    assert_eq!(
        apply_fix("export default function f() { return 1; }\n", &function),
        "function f() { return 1; }\n"
    );

    let class = project::unused_exports(&root.join("src/class.ts")).unwrap();
    assert_eq!(class.findings.len(), 1, "{:?}", class.findings);
    assert_eq!(class.fixes.len(), 1, "{:?}", class.fixes);
    assert_eq!(
        apply_fix("export default class C {}\n", &class),
        "class C {}\n"
    );
}

#[test]
fn skips_export_default_expressions_and_anonymous_defaults() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/expression.ts", "export default 1;\n");
    write(
        root,
        "src/anonymous.ts",
        "export default function () { return 1; }\n",
    );
    write(root, "src/use.ts", "console.log(1);\n");

    for file in ["src/expression.ts", "src/anonymous.ts"] {
        let analysis = project::unused_exports(&root.join(file)).unwrap();
        assert_eq!(
            analysis.findings.len(),
            1,
            "{file}: {:?}",
            analysis.findings
        );
        assert!(analysis.fixes.is_empty(), "{file}: {:?}", analysis.fixes);
    }
}

#[test]
fn skips_reexports_with_a_module_specifier() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/thing.ts", "export const thing = 1;\n");
    write(root, "src/named.ts", "export { thing } from \"./thing\";\n");
    write(root, "src/star.ts", "export * from \"./thing\";\n");
    write(root, "src/use.ts", "console.log(1);\n");

    for file in ["src/named.ts", "src/star.ts"] {
        let analysis = project::unused_exports(&root.join(file)).unwrap();
        assert!(
            analysis.findings.is_empty(),
            "{file}: {:?}",
            analysis.findings
        );
        assert!(analysis.fixes.is_empty(), "{file}: {:?}", analysis.fixes);
    }
}

#[test]
fn skips_multi_declarator_exports_when_another_name_is_used() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/mixed.ts", "export const a = 1, b = 2;\n");
    write(root, "src/all.ts", "export const c = 1, d = 2;\n");
    write(
        root,
        "src/use.ts",
        "import { b } from \"./mixed\";\nconsole.log(b);\n",
    );

    let mixed = project::unused_exports(&root.join("src/mixed.ts")).unwrap();
    assert_eq!(mixed.findings.len(), 1, "{:?}", mixed.findings);
    assert!(mixed.fixes.is_empty(), "{:?}", mixed.fixes);

    let all = project::unused_exports(&root.join("src/all.ts")).unwrap();
    assert_eq!(all.findings.len(), 2, "{:?}", all.findings);
    assert_eq!(all.fixes.len(), 1, "{:?}", all.fixes);
    assert_eq!(
        apply_fix("export const c = 1, d = 2;\n", &all),
        "const c = 1, d = 2;\n"
    );
}

#[test]
fn skips_fixes_that_would_touch_comments_ignore_markers_or_decorators() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/comment.ts", "export /* note */ const a = 1;\n");
    write(
        root,
        "src/specifier.ts",
        "const a = 1;\nconst b = 2;\nexport { a /* note */, b };\n",
    );
    write(
        root,
        "src/ignored.ts",
        "// prettier-ignore\nexport const a = 1;\n",
    );
    write(root, "src/decorated.ts", "export @dec class C {}\n");
    write(
        root,
        "src/use.ts",
        "import { b } from \"./specifier\";\nconsole.log(b);\n",
    );

    for (file, expected) in [
        ("src/comment.ts", 1),
        ("src/specifier.ts", 1),
        ("src/ignored.ts", 1),
        ("src/decorated.ts", 1),
    ] {
        let analysis = project::unused_exports(&root.join(file)).unwrap();
        assert_eq!(
            analysis.findings.len(),
            expected,
            "{file}: {:?}",
            analysis.findings
        );
        assert!(analysis.fixes.is_empty(), "{file}: {:?}", analysis.fixes);
    }
}

#[test]
fn skips_declaration_files_declare_module_and_non_private_packages() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"name":"library","private":false}"#,
    );
    write(root, "tsconfig.json", r#"{"compilerOptions":{}}"#);
    write(
        root,
        "src/decl.d.mts",
        "export declare const value: number;\n",
    );
    write(
        root,
        "src/decl.d.cts",
        "export declare const value: number;\n",
    );
    write(
        root,
        "src/ambient.ts",
        "declare module \"m\" {\n  export const value: number;\n}\n",
    );
    write(root, "src/public.ts", "export const publicApi = 1;\n");
    write(root, "src/use.ts", "console.log(1);\n");

    for file in ["src/decl.d.mts", "src/decl.d.cts", "src/ambient.ts"] {
        let analysis = project::unused_exports(&root.join(file)).unwrap();
        assert!(
            analysis.findings.is_empty(),
            "{file}: {:?}",
            analysis.findings
        );
        assert!(analysis.fixes.is_empty(), "{file}: {:?}", analysis.fixes);
    }

    let public = project::unused_exports(&root.join("src/public.ts")).unwrap();
    assert_eq!(public.findings.len(), 1, "{:?}", public.findings);
    assert!(public.fixes.is_empty(), "{:?}", public.fixes);
}

#[test]
fn refuses_project_fixes_for_an_incomplete_graph() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(root, "src/broken.ts", "export const = ;\n");
    write(root, "src/use.ts", "console.log(1);\n");

    let analysis = project::unused_exports(&root.join("src/a.ts")).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn applied_export_fixes_reparse_and_do_not_trigger_code_removal_verify() {
    let directory = private_project();
    let root = directory.path();
    let input =
        "export const a = 1;\nexport function f() { return 1; }\nexport default class C {}\n";
    write(root, "src/a.ts", input);
    write(root, "src/use.ts", "console.log(1);\n");
    let path = root.join("src/a.ts");

    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 3, "{:?}", analysis.findings);
    assert_eq!(analysis.fixes.len(), 3, "{:?}", analysis.fixes);
    for fix in &analysis.fixes {
        let edited = dotfmt_core::edit::apply(input, &fix.edits).unwrap();
        assert_parses_and_formats(&path, &edited);
    }
}

#[test]
fn bom_offsets_apply_to_export_fixes() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "\u{feff}export const unused = 1;\n");
    write(root, "src/use.ts", "console.log(1);\n");
    let path = root.join("src/a.ts");
    let input = "\u{feff}export const unused = 1;\n";

    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
    assert_eq!(apply_fix(input, &analysis), "\u{feff}const unused = 1;\n");
}

#[test]
fn remove_unused_exports_produces_a_fix() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(root, "src/use.ts", "console.log(1);\n");

    let analysis = project::unused_exports(&root.join("src/a.ts")).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
    assert_eq!(
        analysis.fixes[0].applicability,
        dotfmt_core::edit::Applicability::Unsafe
    );
}

#[test]
fn incomplete_graph_suppresses_unused_exports() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(root, "src/broken.ts", "export const = ;\n");
    write(root, "src/use.ts", "console.log(1);\n");

    assert!(!project::graph_complete(&root.join("src/a.ts")));
    let analysis = project::unused_exports(&root.join("src/a.ts")).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn unresolved_external_packages_do_not_incomplete_the_graph() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "export const a = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { missing } from \"not-installed\";\nconsole.log(missing);\n",
    );
    assert!(project::graph_complete(&root.join("src/a.ts")));

    write(
        root,
        "node_modules/external/package.json",
        r#"{"name":"external","main":"index.js"}"#,
    );
    write(
        root,
        "node_modules/external/index.js",
        "export const thing = 1;\n",
    );
    write(
        root,
        "src/use.ts",
        "import { thing } from \"external\";\nconsole.log(thing);\n",
    );
    project::invalidate_root(root);
    assert!(project::graph_complete(&root.join("src/a.ts")));
}

#[test]
fn unresolved_local_specifiers_incomplete_the_graph() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "export const a = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { missing } from \"./missing\";\nconsole.log(missing);\n",
    );
    assert!(!project::graph_complete(&root.join("src/a.ts")));

    write(
        root,
        "src/use.ts",
        "import { missing } from \"#missing\";\nconsole.log(missing);\n",
    );
    project::invalidate_root(root);
    assert!(!project::graph_complete(&root.join("src/a.ts")));
}

#[test]
fn declaration_consumers_keep_exports_alive() {
    let directory = private_project();
    let root = directory.path();
    write(root, "src/a.ts", "export const helper = 1;\n");
    write(
        root,
        "src/types.d.ts",
        "import { helper } from \"./a\";\nexport type Helper = typeof helper;\n",
    );
    write(root, "src/use.ts", "console.log(1);\n");

    assert!(
        project::unused_exports(&root.join("src/a.ts"))
            .unwrap()
            .findings
            .is_empty()
    );
}

#[test]
fn declaration_only_module_resolution_keeps_the_graph_complete() {
    let directory = private_project();
    let root = directory.path();
    write(
        root,
        "src/use.ts",
        "import type { Types } from \"./types\";\nexport type X = Types;\n",
    );
    write(
        root,
        "src/types.d.ts",
        "export interface Types { value: string }\n",
    );

    assert!(project::graph_complete(&root.join("src/use.ts")));
}

#[test]
fn per_package_tsconfig_monorepos_allow_project_fixes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(root, "packages/ui/package.json", r#"{"private":true}"#);
    write(root, "packages/ui/tsconfig.json", "{}");
    write(root, "packages/ui/src/a.ts", "export const unused = 1;\n");
    write(root, "packages/ui/src/use.ts", "console.log(1);\n");

    assert!(project::graph_complete(&root.join("packages/ui/src/a.ts")));
    let analysis = project::unused_exports(&root.join("packages/ui/src/a.ts")).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
}

#[test]
fn discover_root_prefers_a_package_workspace_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(root, "packages/ui/package.json", r#"{"private":true}"#);
    write(root, "packages/ui/tsconfig.json", "{}");
    write(root, "packages/ui/src/a.ts", "");

    assert_eq!(
        project::discover_root(&root.join("packages/ui/src/a.ts")),
        Some(root.to_path_buf())
    );
}

#[test]
fn discover_root_recognizes_workspace_marker_files() {
    for marker in ["pnpm-workspace.yaml", "turbo.json", "lerna.json"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let contents = if marker == "pnpm-workspace.yaml" {
            "packages:\n  - packages/*\n"
        } else {
            "{}"
        };
        write(root, marker, contents);
        write(root, "packages/ui/package.json", r#"{"private":true}"#);
        write(root, "packages/ui/tsconfig.json", "{}");
        write(root, "packages/ui/src/a.ts", "");

        assert_eq!(
            project::discover_root(&root.join("packages/ui/src/a.ts")),
            Some(root.to_path_buf()),
            "{marker}"
        );
    }
}

#[test]
fn discover_root_prefers_a_solution_tsconfig_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{"files":[],"references":[{"path":"./packages/ui"}]}"#,
    );
    write(root, "packages/ui/tsconfig.json", "{}");
    write(root, "packages/ui/package.json", r#"{"private":true}"#);
    write(root, "packages/ui/src/a.ts", "");

    assert_eq!(
        project::discover_root(&root.join("packages/ui/src/a.ts")),
        Some(root.to_path_buf())
    );
}

#[test]
fn discover_root_keeps_the_package_root_without_a_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "packages/ui/tsconfig.json", "{}");
    write(root, "packages/ui/src/a.ts", "");

    assert_eq!(
        project::discover_root(&root.join("packages/ui/src/a.ts")),
        Some(root.join("packages/ui"))
    );
}

#[test]
fn workspace_root_counts_cross_package_relative_imports() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(root, "packages/ui/package.json", r#"{"private":true}"#);
    write(root, "packages/ui/tsconfig.json", "{}");
    write(
        root,
        "packages/ui/src/helper.ts",
        "export const helper = 1;\nexport const unused = 2;\n",
    );
    write(root, "packages/app/package.json", r#"{"private":true}"#);
    write(root, "packages/app/tsconfig.json", "{}");
    write(
        root,
        "packages/app/src/use.ts",
        "import { helper } from \"../../ui/src/helper\";\nconsole.log(helper);\n",
    );

    let path = root.join("packages/ui/src/helper.ts");
    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.findings[0].message.contains("unused"));
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
    assert_eq!(analysis.fixes[0].title, "Remove unused export `unused`");
}

#[test]
fn solution_style_root_unions_referenced_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{"files":[],"references":[{"path":"./packages/ui"},{"path":"./packages/app"},{"path":"./packages/lib"}]}"#,
    );
    write(root, "packages/ui/package.json", r#"{"private":true}"#);
    write(
        root,
        "packages/ui/tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@ui/*":["src/*"]}}}"#,
    );
    write(
        root,
        "packages/ui/src/helper.ts",
        "export const helper = 1;\nexport const unused = 2;\n",
    );
    write(root, "packages/app/package.json", r#"{"private":true}"#);
    write(root, "packages/app/tsconfig.json", "{}");
    write(root, "packages/lib/package.json", r#"{"private":true}"#);
    write(
        root,
        "packages/lib/tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":"src"}}"#,
    );
    write(root, "packages/lib/src/deep.ts", "export const deep = 1;\n");
    write(
        root,
        "src/use.ts",
        "import { helper } from \"@ui/helper\";\nconsole.log(helper);\n",
    );
    write(
        root,
        "src/base.ts",
        "import { deep } from \"deep\";\nconsole.log(deep);\n",
    );

    let path = root.join("packages/ui/src/helper.ts");
    assert_eq!(
        project::discover_root(&path),
        Some(root.to_path_buf()),
        "the solution config is the workspace root"
    );
    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.findings[0].message.contains("unused"));
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);

    let deep = root.join("packages/lib/src/deep.ts");
    assert!(project::graph_complete(&deep));
    assert_eq!(unused_codes(&deep), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn pnpm_workspace_root_counts_package_name_imports() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "pnpm-workspace.yaml", "packages:\n  - packages/*\n");
    write(
        root,
        "packages/ui/package.json",
        r#"{"name":"@repo/ui","private":true,"main":"./src/index.ts"}"#,
    );
    write(root, "packages/ui/tsconfig.json", "{}");
    write(
        root,
        "packages/ui/src/index.ts",
        "export { helper } from \"./helper\";\n",
    );
    write(
        root,
        "packages/ui/src/helper.ts",
        "export const helper = 1;\nexport const unused = 2;\n",
    );
    write(
        root,
        "packages/app/package.json",
        r#"{"name":"@repo/app","private":true}"#,
    );
    write(root, "packages/app/tsconfig.json", "{}");
    write(
        root,
        "packages/app/src/use.ts",
        "import { helper } from \"@repo/ui\";\nconsole.log(helper);\n",
    );
    fs::create_dir_all(root.join("node_modules/@repo")).unwrap();
    symlink(root.join("packages/ui"), root.join("node_modules/@repo/ui")).unwrap();

    let path = root.join("packages/ui/src/helper.ts");
    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.findings[0].message.contains("unused"));
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
}

#[test]
fn workspace_root_keeps_nested_package_entry_points() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(
        root,
        "packages/ui/package.json",
        r#"{"private":true,"main":"./dist/index.js","module":"./dist/module.js","types":"./dist/types.js","exports":{"./extra":"./dist/extra.js"},"bin":{"tool":"./dist/cli.js"}}"#,
    );
    for file in [
        "packages/ui/src/index.ts",
        "packages/ui/src/module.ts",
        "packages/ui/src/types.ts",
        "packages/ui/src/extra.ts",
        "packages/ui/src/cli.ts",
    ] {
        write(root, file, "export const entry = 1;\n");
    }
    write(
        root,
        "packages/ui/src/other.ts",
        "export const other = 1;\n",
    );

    for file in [
        "packages/ui/src/index.ts",
        "packages/ui/src/module.ts",
        "packages/ui/src/types.ts",
        "packages/ui/src/extra.ts",
        "packages/ui/src/cli.ts",
    ] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}"
        );
    }
    assert_eq!(
        unused_codes(&root.join("packages/ui/src/other.ts")).len(),
        1,
        "a non-entry file in a nested package is still reported"
    );
}

#[test]
fn workspace_root_does_not_treat_package_directories_as_framework_folders() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(root, "packages/app/package.json", r#"{"private":true}"#);
    write(root, "packages/app/tsconfig.json", "{}");
    write(
        root,
        "packages/app/src/unrelated/page.ts",
        "export const page = 1;\n",
    );
    write(
        root,
        "packages/app/src/pages/api.ts",
        "export const handler = 1;\n",
    );
    write(root, "packages/web/tsconfig.json", "{}");
    write(
        root,
        "packages/web/src/unrelated/page.ts",
        "export const page = 1;\n",
    );
    write(
        root,
        "packages/web/src/pages/api.ts",
        "export const handler = 1;\n",
    );

    for file in [
        "packages/app/src/unrelated/page.ts",
        "packages/web/src/unrelated/page.ts",
    ] {
        assert_eq!(
            unused_codes(&root.join(file)).len(),
            1,
            "{file}: a file named page outside app/ is not a route"
        );
    }
    for file in [
        "packages/app/src/pages/api.ts",
        "packages/web/src/pages/api.ts",
    ] {
        assert_eq!(
            unused_codes(&root.join(file)),
            Vec::<String>::new(),
            "{file}: framework folders are relative to the project, not the workspace root"
        );
    }
}

#[test]
fn workspace_root_ignores_an_unrelated_ancestor_package() {
    let outer = tempfile::tempdir().unwrap();
    write(outer.path(), "package.json", r#"{"private":true}"#);
    let root = outer.path().join("repo");
    write(
        &root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(&root, "packages/ui/package.json", r#"{"private":true}"#);
    write(&root, "packages/ui/tsconfig.json", "{}");
    write(&root, "packages/ui/src/a.ts", "export const unused = 1;\n");
    write(&root, "packages/ui/src/use.ts", "console.log(1);\n");

    let path = root.join("packages/ui/src/a.ts");
    assert_eq!(project::discover_root(&path), Some(root.clone()));
    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
}

#[test]
fn workspace_root_keeps_the_private_package_check_per_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(
        root,
        "packages/ui/package.json",
        r#"{"private":false}"#,
    );
    write(root, "packages/ui/tsconfig.json", "{}");
    write(root, "packages/ui/src/a.ts", "export const unused = 1;\n");
    write(root, "packages/ui/src/use.ts", "console.log(1);\n");

    let analysis = project::unused_exports(&root.join("packages/ui/src/a.ts")).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn nested_workspace_roots_stay_incomplete() {
    let outer = tempfile::tempdir().unwrap();
    write(
        outer.path(),
        "package.json",
        r#"{"private":true,"workspaces":["repo/*"]}"#,
    );
    let root = outer.path().join("repo");
    write(&root, "pnpm-workspace.yaml", "packages:\n  - packages/*\n");
    write(&root, "packages/ui/package.json", r#"{"private":true}"#);
    write(&root, "packages/ui/tsconfig.json", "{}");
    write(&root, "packages/ui/src/a.ts", "export const unused = 1;\n");
    write(&root, "packages/ui/src/use.ts", "console.log(1);\n");

    let path = root.join("packages/ui/src/a.ts");
    assert_eq!(project::discover_root(&path), Some(root.clone()));
    assert!(!project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn solution_style_tsconfig_with_an_outside_reference_refuses_fixes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "tsconfig.json",
        r#"{"files":[],"references":[{"path":"../lib"}]}"#,
    );
    write(root, "src/a.ts", "export const unused = 1;\n");
    write(root, "src/use.ts", "console.log(1);\n");

    assert!(!project::graph_complete(&root.join("src/a.ts")));
    let analysis = project::unused_exports(&root.join("src/a.ts")).unwrap();
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn malformed_tsconfig_keeps_alias_imports_local_and_suppresses_fixes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{ "name": "bad-config", "private": true }"#,
    );
    write(
        root,
        "tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"] } }"#,
    );
    write(
        root,
        "src/lib.ts",
        "export const aliased = 1;\nexport const other = 2;\n",
    );
    write(
        root,
        "src/consumer.ts",
        "import { aliased } from \"@/lib\";\nconsole.log(aliased);\n",
    );
    let path = root.join("src/lib.ts");

    assert!(!project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.message.contains("aliased")),
        "findings may remain conservative: {:?}",
        analysis.findings
    );
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);

    let original = fs::read_to_string(&path).unwrap();
    let fixes: Vec<_> = analysis.fixes.iter().collect();
    let after = dotfmt_core::edit::apply_fixes(&original, &fixes).unwrap();
    assert_eq!(after, original);
    assert!(!after.is_empty(), "the imported module must not be deleted");
}

#[test]
fn hash_prefixed_tsconfig_paths_resolve_before_package_imports() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{ "name": "hash-paths", "private": true }"#,
    );
    write(
        root,
        "tsconfig.json",
        r##"{ "compilerOptions": { "baseUrl": ".", "paths": { "#lib/*": ["src/lib/*"] } } }"##,
    );
    write(
        root,
        "src/lib/a.ts",
        "export const hashAliasUsed = 1;\nexport const hashAliasUnused = 2;\n",
    );
    write(
        root,
        "src/consumer.ts",
        "import { hashAliasUsed } from \"#lib/a\";\nconsole.log(hashAliasUsed);\n",
    );
    let path = root.join("src/lib/a.ts");

    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.findings[0].message.contains("hashAliasUnused"));
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
}

#[test]
fn mjs_and_cjs_imports_resolve_declaration_only_modules() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(
        root,
        "tsconfig.json",
        r#"{"compilerOptions":{"module":"NodeNext","moduleResolution":"NodeNext"}}"#,
    );
    write(
        root,
        "src/only.d.mts",
        "export declare const only: number;\n",
    );
    write(
        root,
        "src/only.d.cts",
        "export declare const only: number;\n",
    );
    write(
        root,
        "src/consumer.mts",
        "import { implUsed } from \"./impl.mjs\";\nimport type { only } from \"./only.mjs\";\nconsole.log(implUsed);\nexport type T = typeof only;\n",
    );
    write(
        root,
        "src/impl.mts",
        "export const implUsed = 1;\nexport const implUnused = 2;\n",
    );
    write(
        root,
        "src/consumer.cts",
        "import type { only } from \"./only.cjs\";\nexport type T = typeof only;\n",
    );

    let path = root.join("src/impl.mts");
    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.findings[0].message.contains("implUnused"));
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
    assert!(project::graph_complete(&root.join("src/consumer.cts")));
}

#[test]
fn glob_keeps_gitignored_generated_exports_alive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "package.json", r#"{"private":true}"#);
    write(root, "tsconfig.json", "{}");
    write(root, ".gitignore", "generated/\n*.gen.ts\n");
    write(
        root,
        "generated/a.gen.ts",
        "export const genUsed = 1;\nexport const genUnused = 2;\n",
    );
    write(
        root,
        "src/consumer.ts",
        "const modules = import.meta.glob(\"../generated/*.ts\");\nconsole.log(modules);\n",
    );

    let path = root.join("generated/a.gen.ts");
    assert!(project::graph_complete(&path));
    assert_eq!(unused_codes(&path), Vec::<String>::new());
}

#[test]
fn workspace_packages_see_sibling_deep_imports() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    );
    write(root, "packages/a/package.json", r#"{"private":true}"#);
    write(root, "packages/a/tsconfig.json", "{}");
    write(
        root,
        "packages/a/src/lib.ts",
        "export const deepUsed = 1;\nexport const deepUnused = 2;\n",
    );
    write(root, "packages/b/package.json", r#"{"private":true}"#);
    write(root, "packages/b/tsconfig.json", "{}");
    write(
        root,
        "packages/b/src/consumer.ts",
        "import { deepUsed } from \"../../a/src/lib\";\nconsole.log(deepUsed);\n",
    );

    let path = root.join("packages/a/src/lib.ts");
    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.findings[0].message.contains("deepUnused"));
    assert_eq!(analysis.fixes.len(), 1, "{:?}", analysis.fixes);
}

#[test]
fn wildcard_package_exports_are_entry_points() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "package.json",
        r#"{"name":"exports-wildcard","private":true,"exports":{".":"./src/index.ts","./features/*":"./src/features/*.ts"}}"#,
    );
    write(root, "tsconfig.json", "{}");
    write(root, "src/index.ts", "export const indexThing = 1;\n");
    write(
        root,
        "src/features/alpha.ts",
        "export const publicFeature = 1;\n",
    );
    write(root, "src/use.ts", "console.log(1);\n");

    let path = root.join("src/features/alpha.ts");
    assert!(project::graph_complete(&path));
    let analysis = project::unused_exports(&path).unwrap();
    assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[cfg(unix)]
#[test]
fn tsc_timeout_kills_the_compiler_and_reports_a_diagnostic() {
    if std::env::var_os("DOTFMT_TSC").is_some() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "tsconfig.json", "{}");
    write(root, "src/a.ts", "export const a = 1;\n");
    write_script(&root.join("node_modules/.bin/tsc"), "sleep 5");
    let tsconfig = root.join("tsconfig.json");

    let started = std::time::Instant::now();
    let diagnostics = project::type_diagnostics_with_timeout(
        root,
        &tsconfig,
        std::time::Duration::from_millis(100),
    )
    .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "the compiler must be killed near the timeout"
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, project::TSC_TIMEOUT_CODE);
    assert!(diagnostics[0].message.contains("timed out"));
}
