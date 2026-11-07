#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use dotfmt_core::edit::{Analysis, Finding, Severity};
use dotfmt_typescript::analyze::{self, Options};
use dotfmt_typescript::config::Config;
use dotfmt_typescript::dialect::Dialect;

fn analyze(path: &Path, input: &str) -> Analysis {
    analyze::analyze(
        path,
        input,
        &Config::default(),
        Dialect::Auto,
        &Options::default(),
    )
    .unwrap()
}

fn findings<'a>(analysis: &'a Analysis, code: &str) -> Vec<&'a Finding> {
    analysis
        .findings
        .iter()
        .filter(|finding| finding.code == code)
        .collect()
}

fn project(tsconfig: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("tsconfig.json"), tsconfig).unwrap();
    let path = directory.path().join("a.ts");
    fs::write(&path, "").unwrap();
    (directory, path)
}

#[test]
fn parameters_of_type_level_signatures_are_not_reported() {
    for input in [
        "function f(value: number) { type T = (a: number) => void; return value; }\nf(1);\n",
        "class C { method(value: number) { type T = { fn: (b: number) => void }; return value; } }\nnew C();\n",
        "const f = (value: number) => { type T = (c: number) => void; return value; };\nf(1);\n",
        "class C { field: number; constructor(value: number) { type T = (d: number) => void; this.field = value; } }\nnew C(1);\n",
        "type F = (a: number) => void;\n",
        "interface I { (a: number): void; }\n",
        "type Ctor = new (a: number) => object;\n",
        "type Dict = { [key: string]: number };\n",
        "declare function df(a: number): void;\n",
        "declare class D { method(a: number): void; }\n",
        "abstract class A { abstract method(a: number): void; }\n",
    ] {
        let analysis = analyze(Path::new("a.ts"), input);
        assert!(
            findings(&analysis, "unused-parameter").is_empty(),
            "{input}: {:?}",
            analysis.findings
        );
    }
}

#[test]
fn real_unused_parameters_are_still_reported() {
    let input = "function f(a: number) { return 1; }\nf(1);\n";
    let analysis = analyze(Path::new("a.ts"), input);
    let unused = findings(&analysis, "unused-parameter");
    assert_eq!(unused.len(), 1, "{:?}", analysis.findings);
    assert_eq!(unused[0].start, 11);
    assert_eq!(unused[0].end, 12);
}

#[test]
fn setter_parameters_follow_tsc() {
    let input = "class C { set value(input: number) {} }\nnew C();\n";
    let analysis = analyze(Path::new("a.ts"), input);
    let unused = findings(&analysis, "unused-parameter");
    assert_eq!(unused.len(), 1, "{:?}", analysis.findings);
    assert_eq!(
        unused[0].message,
        "`input` is a parameter that is never used"
    );
}

#[test]
fn parameters_used_only_in_typeof_are_not_reported() {
    let input = "function f(value: number) { const kind = typeof value; return kind; }\nf(1);\n";
    let analysis = analyze(Path::new("a.ts"), input);
    assert!(
        findings(&analysis, "unused-parameter").is_empty(),
        "{:?}",
        analysis.findings
    );
}

#[test]
fn unused_local_severity_tracks_the_tsconfig_options() {
    let input = "function f(value: number) { const unused = 1; return 2; }\nf(1);\n";

    let (_directory, path) = project(r#"{"compilerOptions":{}}"#);
    let analysis = analyze(&path, input);
    assert_eq!(
        findings(&analysis, "unused-variable")[0].severity,
        Severity::Information
    );
    assert_eq!(
        findings(&analysis, "unused-parameter")[0].severity,
        Severity::Information
    );

    let (_directory, path) =
        project(r#"{"compilerOptions":{"noUnusedLocals":true,"noUnusedParameters":true}}"#);
    let analysis = analyze(&path, input);
    assert_eq!(
        findings(&analysis, "unused-variable")[0].severity,
        Severity::Warning
    );
    assert_eq!(
        findings(&analysis, "unused-parameter")[0].severity,
        Severity::Warning
    );

    let (_directory, path) =
        project(r#"{"compilerOptions":{"noUnusedLocals":false,"noUnusedParameters":false}}"#);
    let analysis = analyze(&path, input);
    assert_eq!(
        findings(&analysis, "unused-variable")[0].severity,
        Severity::Information
    );
    assert_eq!(
        findings(&analysis, "unused-parameter")[0].severity,
        Severity::Information
    );
}

#[test]
fn type_parameters_and_private_members_stay_warnings() {
    let (_directory, path) = project(r#"{"compilerOptions":{}}"#);
    let input = "class C { #unused = 1; }\nfunction f<T>() { return 1; }\nf();\nnew C();\n";
    let analysis = analyze(&path, input);
    assert_eq!(
        findings(&analysis, "unused-type-parameter")[0].severity,
        Severity::Warning
    );
    assert_eq!(
        findings(&analysis, "unused-class-member")[0].severity,
        Severity::Warning
    );
}

#[test]
fn declaration_files_never_report_type_parameters() {
    for name in ["a.d.ts", "a.d.mts", "a.d.cts"] {
        let input = "export type Box<T> = { value: string };\n";
        let analysis = analyze(Path::new(name), input);
        assert!(
            findings(&analysis, "unused-type-parameter").is_empty(),
            "{name}: {:?}",
            analysis.findings
        );
    }

    let analysis = analyze(
        Path::new("a.ts"),
        "export type Box<T> = { value: string };\n",
    );
    assert_eq!(
        findings(&analysis, "unused-type-parameter").len(),
        1,
        "{:?}",
        analysis.findings
    );
}

fn analyze_with_fixes(path: &Path, input: &str, unsafe_fixes: bool) -> Analysis {
    analyze::analyze(
        path,
        input,
        &Config::default(),
        Dialect::Auto,
        &Options {
            fix: true,
            unsafe_fixes,
            type_aware: false,
            ..Options::default()
        },
    )
    .unwrap()
}

fn removal_fixes(analysis: &Analysis) -> usize {
    analysis
        .fixes
        .iter()
        .filter(|fix| fix.title.contains("unused variable"))
        .count()
}

#[test]
fn top_level_script_let_and_const_bind_globals_too() {
    let script = "var a = 1;\nlet b = 2;\nconst c = 3;\n";
    let analysis = analyze_with_fixes(Path::new("a.js"), script, true);
    assert_eq!(
        findings(&analysis, "unused-variable").len(),
        3,
        "{:?}",
        analysis.findings
    );
    assert_eq!(removal_fixes(&analysis), 0, "{:?}", analysis.fixes);

    let module = "var a = 1;\nlet b = 2;\nconst c = 3;\nexport {};\n";
    let analysis = analyze_with_fixes(Path::new("a.js"), module, true);
    assert_eq!(removal_fixes(&analysis), 3, "{:?}", analysis.fixes);
}

#[test]
fn pure_initializers_follow_unsafe_fixes() {
    for initializer in [
        "count + 1",
        "count ? 1 : 2",
        "`${count}`",
        "count.value",
        "count?.value",
        "-count",
        "!count",
        "typeof count",
        "void 0",
        "count ?? 1",
        "[count, 2]",
        "{ value: count }",
        "count as number",
    ] {
        let input = format!(
            "function f(count: number) {{ const unused = {initializer}; return count; }}\nf(1);\n"
        );
        let analysis = analyze_with_fixes(Path::new("a.ts"), &input, true);
        assert_eq!(
            removal_fixes(&analysis),
            1,
            "{initializer}: {:?}",
            analysis.findings
        );
        assert!(
            findings(&analysis, "unused-variable")
                .iter()
                .all(|finding| !finding.message.contains("side effects")),
            "{initializer}: {:?}",
            analysis.findings
        );
    }
}

#[test]
fn impure_initializers_are_kept_under_unsafe_fixes() {
    for initializer in [
        "risky()",
        "new Thing()",
        "count = 2",
        "count++",
        "tag`value`",
        "obj[risky()]",
        "delete obj.value",
        "{ [key()]: count }",
        "[...items]",
    ] {
        let input = format!(
            "function f(count: number) {{ const unused = {initializer}; return count; }}\nf(1);\n"
        );
        let analysis = analyze_with_fixes(Path::new("a.ts"), &input, true);
        assert_eq!(
            removal_fixes(&analysis),
            0,
            "{initializer}: {:?}",
            analysis.fixes
        );
        assert!(
            findings(&analysis, "unused-variable")
                .iter()
                .any(|finding| finding.message.contains("side effects")),
            "{initializer}: {:?}",
            analysis.findings
        );
    }

    let input = "async function f() { const unused = await load(); return 1; }\nf();\n";
    let analysis = analyze_with_fixes(Path::new("a.ts"), input, true);
    assert_eq!(removal_fixes(&analysis), 0, "{:?}", analysis.fixes);
}
