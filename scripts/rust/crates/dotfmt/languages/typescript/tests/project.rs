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
