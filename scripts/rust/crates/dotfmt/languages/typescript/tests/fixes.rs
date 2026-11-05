#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use dotfmt_core::edit::{Analysis, Applicability, Severity, apply, apply_fixes};
use dotfmt_typescript::analyze::{self, Options, TypeDiagnostic};
use dotfmt_typescript::config::Config;
use dotfmt_typescript::dialect::Dialect;

fn analyze(path: &Path, input: &str, fix: bool, unsafe_fixes: bool) -> Analysis {
    analyze_with_config(path, input, &Config::default(), fix, unsafe_fixes)
}

fn analyze_with_config(
    path: &Path,
    input: &str,
    config: &Config,
    fix: bool,
    unsafe_fixes: bool,
) -> Analysis {
    analyze::analyze(
        path,
        input,
        config,
        Dialect::Auto,
        &Options {
            fix,
            unsafe_fixes,
            type_aware: false,
            organize: true,
            project_analysis: false,
            type_diagnostics: Vec::new(),
        },
    )
    .unwrap()
}

fn apply_every_fix(input: &str, analysis: &Analysis) -> String {
    let fixes: Vec<_> = analysis.fixes.iter().collect();
    apply_fixes(input, &fixes).unwrap()
}

fn temp_project(tsconfig: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("tsconfig.json"), tsconfig).unwrap();
    let path = directory.path().join("a.ts");
    fs::write(&path, "").unwrap();
    (directory, path)
}

#[test]
fn unused_named_specifiers_are_removed() {
    let input = "import { a, b } from \"m\";\nconsole.log(b);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    assert_eq!(analysis.findings.len(), 1);
    assert_eq!(analysis.findings[0].code, "unused-import");
    assert_eq!(analysis.findings[0].start, 9);
    assert_eq!(analysis.findings[0].end, 10);
    assert_eq!(
        apply_every_fix(input, &analysis),
        "import { b } from \"m\";\nconsole.log(b);\n"
    );
}

#[test]
fn unused_default_and_namespace_imports_preserve_side_effects() {
    let input = "import value from \"m\";\nconsole.log(1);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    assert_eq!(analysis.findings[0].code, "unused-import");
    assert_eq!(
        apply_every_fix(input, &analysis),
        "import \"m\";\nconsole.log(1);\n"
    );

    let input = "import * as ns from \"n\";\nconsole.log(1);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    assert_eq!(
        apply_every_fix(input, &analysis),
        "import \"n\";\nconsole.log(1);\n"
    );
}

#[test]
fn unused_imports_from_declaration_modules_are_deleted_not_bare_imported() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("tsconfig.json"), "{}").unwrap();
    fs::write(
        directory.path().join("types.d.ts"),
        "export interface Unused { x: number }\n",
    )
    .unwrap();
    let path = directory.path().join("a.ts");
    let input = "import { Unused } from \"./types\";\nconsole.log(1);\n";
    fs::write(&path, input).unwrap();
    let analysis = analyze(&path, input, true, false);
    assert_eq!(apply_every_fix(input, &analysis), "console.log(1);\n");
}

#[test]
fn declaration_module_import_keeps_side_effect_under_verbatim_module_syntax() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("tsconfig.json"),
        "{\"compilerOptions\":{\"verbatimModuleSyntax\":true}}\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("types.d.ts"),
        "export interface Unused { x: number }\n",
    )
    .unwrap();
    let path = directory.path().join("a.ts");
    let input = "import { Unused } from \"./types\";\nconsole.log(1);\n";
    fs::write(&path, input).unwrap();
    let analysis = analyze(&path, input, true, false);
    assert_eq!(
        apply_every_fix(input, &analysis),
        "import \"./types\";\nconsole.log(1);\n"
    );
}

#[test]
fn top_level_script_vars_are_not_removed() {
    let input = "var exposed;\nconsole.log(\"exposed\" in globalThis);\n";
    let analysis = analyze(Path::new("a.js"), input, true, false);
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn unused_type_imports_are_deleted() {
    let input = "import type { A } from \"m\";\nconsole.log(1);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    assert_eq!(analysis.findings[0].code, "unused-import");
    assert_eq!(apply_every_fix(input, &analysis), "console.log(1);\n");
}

#[test]
fn jsx_usage_counts_as_a_reference() {
    let input = "import { Widget } from \"./widget\";\nexport const view = <Widget />;\n";
    let analysis = analyze(Path::new("a.tsx"), input, true, false);
    assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
}

#[test]
fn type_only_imports_are_converted_when_the_tsconfig_allows_it() {
    let (_directory, path) = temp_project(r#"{"compilerOptions":{}}"#);
    let input = "import { A } from \"m\";\nlet x: A;\n";
    let analysis = analyze(&path, input, true, false);
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "type-only-import"),
        "{:?}",
        analysis.findings
    );
    let conversion = analysis
        .fixes
        .iter()
        .find(|fix| fix.title.contains("type-only"))
        .expect("type-only fix");
    assert_eq!(conversion.applicability, Applicability::Safe);
    assert_eq!(
        apply(input, &conversion.edits).unwrap(),
        "import type { A } from \"m\";\nlet x: A;\n"
    );

    for input in [
        "import * as NS from \"m\";\nlet x: NS.A;\n",
        "import Foo from \"m\";\nlet x: Foo;\n",
    ] {
        let analysis = analyze(&path, input, true, false);
        let conversion = analysis
            .fixes
            .iter()
            .find(|fix| fix.title.contains("type-only"))
            .expect("type-only fix");
        assert_eq!(
            apply(input, &conversion.edits).unwrap(),
            input.replacen("import ", "import type ", 1)
        );
    }
}

#[test]
fn tsconfig_gates_disable_type_only_conversion() {
    for tsconfig in [
        r#"{"compilerOptions":{"verbatimModuleSyntax":true}}"#,
        r#"{"compilerOptions":{"importsNotUsedAsValues":"preserve"}}"#,
        r#"{"compilerOptions":{"preserveValueImports":true}}"#,
        r#"{"compilerOptions":{"emitDecoratorMetadata":true}}"#,
    ] {
        let (_directory, path) = temp_project(tsconfig);
        let input = "import { A } from \"m\";\nlet x: A;\n";
        let analysis = analyze(&path, input, true, true);
        assert!(
            !analysis
                .findings
                .iter()
                .any(|finding| finding.code == "type-only-import"),
            "{tsconfig}: {:?}",
            analysis.findings
        );
        assert!(
            !analysis
                .fixes
                .iter()
                .any(|fix| fix.title.contains("type-only")),
            "{tsconfig}: {:?}",
            analysis.fixes
        );
    }
}

#[test]
fn mixed_type_and_value_imports_are_report_only() {
    let (_directory, path) = temp_project(r#"{"compilerOptions":{}}"#);
    let input = "import { A, value } from \"m\";\nlet x: A;\nconsole.log(value);\n";
    let analysis = analyze(&path, input, true, false);
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "type-only-import"),
        "{:?}",
        analysis.findings
    );
    assert!(
        !analysis
            .fixes
            .iter()
            .any(|fix| fix.title.contains("type-only")),
        "{:?}",
        analysis.fixes
    );
}

#[test]
fn default_and_named_type_imports_are_report_only_because_typescript_rejects_them() {
    let (_directory, path) = temp_project(r#"{"compilerOptions":{}}"#);
    let input = "import Foo, { Bar } from \"m\";\nlet x: Foo | Bar;\n";
    let analysis = analyze(&path, input, true, false);
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "type-only-import"),
        "{:?}",
        analysis.findings
    );
    assert!(
        !analysis
            .fixes
            .iter()
            .any(|fix| fix.title.contains("type-only")),
        "{:?}",
        analysis.fixes
    );
}

#[test]
fn no_tsconfig_still_protects_classic_jsx_factory_imports() {
    let input = "import React from \"react\";\nexport const x = 1;\n";
    let analysis = analyze::analyze(
        Path::new("a.tsx"),
        input,
        &Config::default(),
        Dialect::Tsx,
        &Options {
            fix: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn classic_jsx_factory_imports_are_never_removed() {
    let (_directory, path) = temp_project(
        r#"{"compilerOptions":{"jsx":"react","jsxFactory":"h","jsxFragmentFactory":"Fragment"}}"#,
    );
    let path = path.with_extension("tsx");
    let input = "import h from \"preact\";\nimport Fragment from \"preact\";\nexport const view = <><div /></>;\n";
    let analysis = analyze(&path, input, true, true);
    assert!(
        analysis
            .findings
            .iter()
            .all(|finding| finding.code != "unused-import"),
        "{:?}",
        analysis.findings
    );
}

#[test]
fn composite_and_coercing_initializers_are_never_safe_to_remove() {
    for initializer in ["a + b", "[...items]", "{ ...value }", "`${value}`", "value"] {
        let input = format!("const unused = {initializer};\n");
        let analysis = analyze::analyze(
            Path::new("a.ts"),
            &input,
            &Config::default(),
            Dialect::Auto,
            &Options {
                fix: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            analysis.fixes.is_empty(),
            "{initializer} should not have a safe fix: {:?}",
            analysis.fixes
        );
    }
}

#[test]
fn unused_variables_are_removed_only_by_unsafe_fixes() {
    let input = "const x = 1;\nconsole.log(2);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    assert_eq!(analysis.findings[0].code, "unused-variable");
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
    assert_eq!(apply_every_fix(input, &analysis), input);

    let analysis = analyze(Path::new("a.ts"), input, true, true);
    let fix = analysis
        .fixes
        .iter()
        .find(|fix| fix.title.contains("unused variable"))
        .expect("unsafe fix");
    assert_eq!(fix.applicability, Applicability::Unsafe);
    assert_eq!(apply_every_fix(input, &analysis), "console.log(2);\n");
}

#[test]
fn impure_initializers_need_remove_impure_locals() {
    let input = "const x = risky();\nconsole.log(2);\n";
    for unsafe_fixes in [false, true] {
        let analysis = analyze(Path::new("a.ts"), input, true, unsafe_fixes);
        assert_eq!(analysis.findings[0].code, "unused-variable");
        assert!(analysis.fixes.is_empty(), "{analysis:?}");
    }

    let config = Config {
        remove_impure_locals: true,
        ..Config::default()
    };
    let analysis = analyze_with_config(Path::new("a.ts"), input, &config, true, true);
    let fix = analysis
        .fixes
        .iter()
        .find(|fix| fix.title.contains("unused variable"))
        .expect("opted-in unsafe fix");
    assert_eq!(fix.applicability, Applicability::Unsafe);
    assert_eq!(apply_every_fix(input, &analysis), "console.log(2);\n");

    let analysis = analyze_with_config(Path::new("a.ts"), input, &config, true, false);
    assert!(analysis.fixes.is_empty(), "{analysis:?}");
}

#[test]
fn unused_variable_removal_is_blocked_by_side_effect_prerequisites() {
    for input in [
        "eval(\"x\");\nconst x = 1;\n",
        "new Function(\"return 1\");\nconst x = 1;\n",
        "// keep me\nconst x = 1;\n",
        "const a = 1, x = 2;\nconsole.log(a);\n",
        "const { x } = obj();\nconsole.log(1);\n",
        "for (let x = 0; x < 1; x++) {}\n",
        "export const x = 1;\n",
    ] {
        let analysis = analyze(Path::new("a.ts"), input, true, true);
        assert!(
            !analysis
                .fixes
                .iter()
                .any(|fix| fix.title.contains("unused variable")),
            "{input:?}: {:?}",
            analysis.fixes
        );
    }
}

#[test]
fn parameters_class_members_and_type_parameters_are_report_only() {
    let input = "class C { #used = 1; #unused = 2; m() { return this.#used; } }\nfunction f<T>(value: number) { return 1; }\nnew C();\nf(1);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, true);
    for code in [
        "unused-class-member",
        "unused-parameter",
        "unused-type-parameter",
    ] {
        assert!(
            analysis.findings.iter().any(|finding| finding.code == code),
            "missing {code}: {:?}",
            analysis.findings
        );
    }
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
}

#[test]
fn declaration_files_never_get_removal_fixes() {
    let input = "import { A } from \"m\";\nconst x = 1;\nexport const y = 2;\n";
    for name in ["a.d.ts", "a.d.mts", "a.d.cts"] {
        let analysis = analyze(Path::new(name), input, true, true);
        assert!(!analysis.findings.is_empty(), "{name}");
        assert!(analysis.fixes.is_empty(), "{name}: {:?}", analysis.fixes);
    }
}

#[test]
fn individual_import_fixes_are_not_hidden_when_organize_is_not_selected() {
    let input = "import { a } from \"a\";\nimport { b } from \"b\";\nconsole.log(a);\n";
    let analysis = analyze::analyze(
        Path::new("a.ts"),
        input,
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: true,
            unsafe_fixes: false,
            type_aware: false,
            organize: false,
            project_analysis: false,
            type_diagnostics: Vec::new(),
        },
    )
    .unwrap();
    assert!(
        analysis
            .fixes
            .iter()
            .any(|fix| fix.title.contains("Remove unused imports")),
        "{:?}",
        analysis.fixes
    );
    assert_eq!(
        apply_every_fix(input, &analysis),
        "import { a } from \"a\";\nimport \"b\";\nconsole.log(a);\n"
    );
}

#[test]
fn tsc_unused_codes_are_warnings_and_other_codes_are_errors() {
    let input = "const value = 1;\nconsole.log(value);\n";
    let analysis = analyze::analyze(
        Path::new("a.ts"),
        input,
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: false,
            unsafe_fixes: false,
            type_aware: false,
            organize: false,
            project_analysis: false,
            type_diagnostics: vec![
                TypeDiagnostic {
                    code: 6133,
                    message: "`value` is declared but never read.".to_owned(),
                    start: 6,
                    end: 11,
                },
                TypeDiagnostic {
                    code: 7027,
                    message: "Unreachable code detected.".to_owned(),
                    start: 12,
                    end: 13,
                },
                TypeDiagnostic {
                    code: 2322,
                    message: "Type mismatch.".to_owned(),
                    start: 18,
                    end: 23,
                },
            ],
        },
    )
    .unwrap();
    let severity = |code: &str| {
        analysis
            .findings
            .iter()
            .find(|finding| finding.code == code)
            .map(|finding| finding.severity)
    };
    assert_eq!(severity("tsc-6133"), Some(Severity::Warning));
    assert_eq!(severity("tsc-7027"), Some(Severity::Warning));
    assert_eq!(severity("tsc-2322"), Some(Severity::Error));
}

#[test]
fn organize_imports_sorts_contiguous_imports_and_keeps_side_effects_in_place() {
    let input = "import z from \"z\";\nimport a from \"a\";\n// b stays with b\nimport b from \"b\";\n\nimport \"side\";\nimport c from \"c\";\na;b;c;z;\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    let organize = analysis
        .fixes
        .iter()
        .find(|fix| fix.kind == "source.organizeImports.dotfmt")
        .expect("organize imports fix");
    assert_eq!(organize.applicability, Applicability::Safe);
    let output = apply(input, &organize.edits).unwrap();
    let a = output.find("from \"a\"").unwrap();
    let b = output.find("from \"b\"").unwrap();
    let c = output.find("from \"c\"").unwrap();
    assert!(a < b && b < c, "{output}");
    assert!(output.contains("// b stays with b"), "{output}");
    let side = output.find("import \"side\";").unwrap();
    let original_side = input.find("import \"side\";").unwrap();
    let original_b = input.find("import b").unwrap();
    let original_c = input.find("import c").unwrap();
    assert!(
        original_side > original_b && original_side < original_c,
        "fixture"
    );
    assert!(
        output.matches("import \"").count() == 1,
        "side-effect import should stay: {output}"
    );
    let _ = side;
}

#[test]
fn organize_imports_never_reorders_across_side_effect_imports() {
    let input = "import { b } from \"./b\";\nimport \"./side\";\nimport { a } from \"./a\";\nconsole.log(a, b);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    let organize = analysis
        .fixes
        .iter()
        .find(|fix| fix.kind.contains("organizeImports"));
    assert!(organize.is_none(), "{:?}", analysis.fixes);
    assert_eq!(apply_every_fix(input, &analysis), input);
}

#[test]
fn organize_and_unused_import_edits_do_not_overlap() {
    let input = "import { unused, used } from \"b\";\nimport other from \"a\";\nconsole.log(used, other);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    let fixes: Vec<_> = analysis.fixes.iter().collect();
    let output = apply_fixes(input, &fixes).unwrap();
    assert_eq!(
        output,
        "import other from \"a\";\nimport { used } from \"b\";\nconsole.log(used, other);\n"
    );
}

#[test]
fn findings_are_still_reported_when_fixes_are_disabled() {
    let input = "import { a } from \"m\";\n";
    let analysis = analyze(Path::new("a.ts"), input, false, false);
    assert_eq!(analysis.findings[0].code, "unused-import");
    assert!(analysis.fixes.is_empty());
}

#[test]
fn editor_type_diagnostics_gate_fixes_and_add_related_information() {
    let input = "import { a } from \"m\";\n";
    let analysis = analyze::analyze(
        Path::new("a.ts"),
        input,
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: true,
            unsafe_fixes: false,
            type_aware: false,
            organize: true,
            project_analysis: false,
            type_diagnostics: vec![TypeDiagnostic {
                code: 2322,
                message: "Type 'number' is not assignable to type 'string'.".to_owned(),
                start: 9,
                end: 10,
            }],
        },
    )
    .unwrap();
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);
    let unused = analysis
        .findings
        .iter()
        .find(|finding| finding.code == "unused-import")
        .expect("unused import finding");
    assert!(
        unused
            .related
            .iter()
            .any(|related| related.message.contains("TS2322")),
        "{:?}",
        analysis.findings
    );
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "tsc-2322"),
        "{:?}",
        analysis.findings
    );
}
#[test]
fn relative_tsconfig_extends_is_followed_for_gates() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("base.json"),
        r#"{"compilerOptions":{"verbatimModuleSyntax":true}}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("tsconfig.json"),
        r#"{"extends":"./base.json"}"#,
    )
    .unwrap();
    let path = directory.path().join("a.ts");
    fs::write(&path, "").unwrap();
    let input = "import { A } from \"m\";\nlet x: A;\n";
    let analysis = analyze(&path, input, true, false);
    assert!(
        !analysis
            .findings
            .iter()
            .any(|finding| finding.code == "type-only-import"),
        "{:?}",
        analysis.findings
    );
}

#[test]
fn unused_import_equals_is_reported_without_a_fix() {
    let input = "import fs = require(\"fs\");\nconsole.log(1);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, true);
    let finding = analysis
        .findings
        .iter()
        .find(|finding| finding.code == "unused-import")
        .expect("unused import-equals finding");
    assert_eq!(finding.message, "`fs` is imported but never used");
    assert_eq!((finding.start, finding.end), (7, 9));
    assert!(
        finding
            .related
            .iter()
            .any(|related| related.message == "imported from \"fs\""),
        "{:?}",
        finding.related
    );
    assert!(analysis.fixes.is_empty(), "{:?}", analysis.fixes);

    let input = "namespace N { export type T = number; }\nimport alias = N;\n";
    let analysis = analyze(Path::new("a.ts"), input, true, true);
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "unused-import"
                && finding.message == "`alias` is imported but never used"),
        "{:?}",
        analysis.findings
    );
    assert!(
        !analysis.fixes.iter().any(|fix| fix
            .finding
            .as_ref()
            .is_some_and(|finding| finding.code == "unused-import")),
        "{:?}",
        analysis.fixes
    );
}

#[test]
fn import_equals_is_left_alone() {
    let input = "import fs = require(\"fs\");\nlet x: fs.Stats;\n";
    let analysis = analyze(Path::new("a.ts"), input, true, true);
    assert!(
        !analysis
            .findings
            .iter()
            .any(|finding| finding.code == "unused-import"),
        "{:?}",
        analysis.findings
    );
    assert!(
        !analysis
            .fixes
            .iter()
            .any(|fix| fix.title.contains("type-only")),
        "{:?}",
        analysis.fixes
    );
}

#[test]
fn bom_offsets_are_applied_to_findings_and_fixes() {
    let input = "\u{feff}import { a } from \"m\";\nconsole.log(1);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    assert!(analysis.findings[0].start >= 3, "{:?}", analysis.findings);
    assert!(
        analysis.fixes[0].edits[0].start >= 3,
        "{:?}",
        analysis.fixes
    );
    assert_eq!(
        apply_every_fix(input, &analysis),
        "\u{feff}import \"m\";\nconsole.log(1);\n"
    );
}
#[test]
fn deleting_a_whole_import_run_does_not_cover_other_runs() {
    let input = "import type { A } from \"a\";\nimport type { B } from \"b\";\nconsole.log(1);\nimport type { C } from \"c\";\nconsole.log(2);\n";
    let analysis = analyze(Path::new("a.ts"), input, true, false);
    assert_eq!(
        apply_every_fix(input, &analysis),
        "console.log(1);\nconsole.log(2);\n"
    );
}
#[test]
fn direct_function_calls_block_removal_like_eval() {
    for call in [
        r#"Function("helper()")"#,
        r#"const f = Function; f("helper()")"#,
    ] {
        let input = format!("import {{ helper }} from \"./helper\";\n{call};\n");
        let analysis = analyze::analyze(
            Path::new("a.ts"),
            &input,
            &Config::default(),
            Dialect::Auto,
            &Options {
                fix: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(analysis.fixes.is_empty(), "{call}: {:?}", analysis.fixes);
    }
}

#[test]
fn eval_blocks_import_removal_and_type_only_conversion_too() {
    let input = "import { a, A } from \"m\";\nlet x: A;\neval('a');\n";
    let analysis = analyze(Path::new("a.ts"), input, true, true);
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.code == "unused-import")
    );
    assert!(
        !analysis
            .fixes
            .iter()
            .any(|fix| fix.title.contains("unused import") || fix.title.contains("type-only")),
        "{:?}",
        analysis.fixes
    );
}

#[test]
fn exported_namespace_members_do_not_produce_invalid_fixes() {
    let (_directory, path) = temp_project(r#"{"compilerOptions":{}}"#);
    let input = "export namespace N { export const x = 1; }\n";
    fs::write(&path, input).unwrap();

    let analysis = analyze(&path, input, true, true);
    assert!(
        analysis
            .findings
            .iter()
            .all(|finding| finding.code != "unused-variable"),
        "{:?}",
        analysis.findings
    );
    let edited = apply_every_fix(input, &analysis);
    assert_eq!(edited, input);
    analyze(&path, &edited, true, true);
}
