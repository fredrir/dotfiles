use std::path::PathBuf;
use std::process::Command;

use super::*;

// The filter jq runs, with the probe written as `P`.
fn probed(filter: &str) -> String {
    let program = analyze(filter).program.expect(filter);
    format!("{}P{}", program.before, program.after)
}

const PROBES: [(&str, &str); 30] = [
    (".", "P"),
    (".a.", "(.a | P)"),
    (".items[].na", "(.items[] | P)"),
    (".items[] | select(.na", ".items[] | select(P)"),
    (".items | map(.", ".items | map(P)"),
    (".[0].", "(.[0] | P)"),
    ("map(.a) | .[0].", "map(.a) | (.[0] | P)"),
    (".a?.b.", "(.a?.b | P)"),
    (r#".a."b c"."#, r#"(.a."b c" | P)"#),
    (r#".a.["b"#, "(.a | P)"),
    (r#".["b"#, "(. | P)"),
    (r#"."my"#, "P"),
    ("$__loc__.", "($__loc__ | P)"),
    ("limit(1; .a).", "(limit(1; .a) | P)"),
    ("if . then 1 else 2 end.", "(if . then 1 else 2 end | P)"),
    (".a as $x | $x.", ".a as $x | ($x | P)"),
    ("if .", "if P then empty else empty end"),
    ("if .a then .b.", "if .a then (.b | P) else empty end"),
    (
        ".a | if . then .b elif .",
        ".a | if . then .b elif P then empty else empty end",
    ),
    ("if . then 1 else .", "if . then 1 else P end"),
    ("reduce .", "reduce P as $__zcomp (.; .)"),
    (
        "reduce range(0; 3) as $i (0; . + $i.",
        "reduce range(0; 3) as $i (0; . + ($i | P))",
    ),
    (
        "foreach .[] as $x (0; . + 1; .",
        "foreach .[] as $x (0; . + 1; P)",
    ),
    ("{a: .", "{a: P}"),
    ("{a: 1, na", "{a: 1, (P): null}"),
    (".a | {b: [.c, .", ".a | {b: [.c, P]}"),
    (r#""x\(.a."#, r#""x\((.a | P))""#),
    (r#""\(1 + (."#, r#""\(1 + (P))""#),
    ("def f: .", "def f: P; f"),
    ("def f(x): x | .", "def f(x): x | P; ."),
];

#[test]
fn the_probe_replaces_the_term_at_the_cursor_and_open_constructs_are_closed() {
    for (filter, program) in PROBES {
        assert_eq!(probed(filter), program, "{filter}");
    }
    assert_eq!(probed(".a | "), ".a | P");
    assert_eq!(probed("try .a."), "try (.a | P)");
    assert_eq!(probed("label $out | ."), "label $out | P");
}

fn system_jq() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("jq"))
        .find(|path| path.is_file())
        .expect("jq on PATH")
}

#[test]
fn every_probe_program_compiles() {
    let jq = system_jq();
    for (filter, _) in PROBES {
        let program = probed(filter).replace('P', "empty");
        let ran = Command::new(&jq).args(["-n", &program]).output().unwrap();
        assert_ne!(ran.status.code(), Some(3), "{filter}: {program}");
    }
}

#[test]
fn keys_are_wanted_after_dots_quotes_and_brackets() {
    let field = analyze(".items[] | select(.na");
    assert_eq!(field.want, Want::Field);
    assert_eq!(field.partial, "na");
    assert_eq!(
        &".items[] | select(.na"[..field.skip],
        ".items[] | select(."
    );
    let quoted = analyze(r#".a."my k"#);
    assert_eq!(quoted.want, Want::QuotedField);
    assert_eq!(quoted.partial, "my k");
    assert_eq!(quoted.base, ".a");
    let index = analyze(r#".a["x"#);
    assert_eq!(index.want, Want::Index);
    assert_eq!(index.skip, 4);
    assert_eq!(analyze("{a: 1, na").want, Want::ObjectKey);
    assert_eq!(analyze("[\"x").want, Want::Nothing);
}

#[test]
fn environment_paths_keep_their_base() {
    assert_eq!(analyze("$ENV.HO").base, "$ENV");
    assert_eq!(analyze("env.").base, "env");
}

#[test]
fn words_at_a_new_term_want_functions_and_keywords() {
    let start = analyze("ma");
    assert_eq!(start.want, Want::Term);
    assert_eq!(start.partial, "ma");
    assert_eq!(start.program, None);
    assert!(start.keywords.contains(&"if") && start.keywords.contains(&"import"));
    let later = analyze(".a | ma");
    assert_eq!(later.skip, 5);
    assert!(!later.keywords.contains(&"import"));
    assert!(
        analyze(r#"import "m" as m; "#)
            .keywords
            .contains(&"include")
    );
    assert_eq!(analyze(r#".a | "\("#).want, Want::Term);
}

#[test]
fn after_a_term_only_operators_spelled_as_words_fit() {
    assert_eq!(analyze(".a ").keywords, ["and", "or", "as"]);
    assert_eq!(analyze("if .a ").keywords, ["and", "or", "as", "then"]);
    assert_eq!(
        analyze("if .a then .b ").keywords,
        ["and", "or", "as", "elif", "else", "end"]
    );
    assert_eq!(analyze("try .a ").keywords, ["and", "or", "as", "catch"]);
    assert_eq!(analyze("try .a catch . ").keywords, ["and", "or", "as"]);
}

#[test]
fn variables_formats_modules_and_module_functions_are_recognised() {
    let variable = analyze(".a and $");
    assert_eq!((variable.want, variable.skip), (Want::Variable, 7));
    assert_eq!(analyze("@ba").want, Want::Format);
    assert_eq!(analyze(r#"import ""#).want, Want::Module { data: true });
    assert_eq!(analyze(r#"include "a/"#).want, Want::Module { data: false });
    assert_eq!(analyze(r#"include "a/"#).partial, "a/");
    let function = analyze(r#"import "m" as m; m::"#);
    assert_eq!(function.want, Want::ModuleFunction { module: "m".into() });
    assert_eq!(function.partial, "m::");
}

#[test]
fn names_being_bound_strings_and_comments_get_nothing() {
    for filter in [
        ". as $",
        ". as [$a, $",
        ". as $x ",
        "def f($",
        "def ",
        "label $",
        r#""abc"#,
        "# .",
        "..",
        "1",
    ] {
        assert_eq!(analyze(filter).want, Want::Nothing, "{filter}");
    }
    assert_eq!(analyze(r#"import "c" as $c; $"#).want, Want::Variable);
}

#[test]
fn bound_variables_are_collected_from_patterns_parameters_and_labels() {
    let analysis =
        analyze(". as [$a, {b: $c}] | reduce .[] as $d (0; .) | def f($e; g): 1; label $out | $");
    assert_eq!(analysis.variables, ["$a", "$c", "$d", "$e", "$out"]);
}

#[test]
fn definitions_and_module_directives_are_collected() {
    let analysis = analyze(
        r#"import "lib/m" as m; import "d" as $d; include "i"; def f: 1; def g(a; $b): 2; "#,
    );
    assert_eq!(analysis.functions, [("f".into(), 0), ("g".into(), 2)]);
    assert_eq!(
        analysis.imports,
        [
            Import {
                path: "lib/m".into(),
                alias: Some("m".into()),
                data: false
            },
            Import {
                path: "d".into(),
                alias: Some("d".into()),
                data: true
            },
            Import {
                path: "i".into(),
                alias: None,
                data: false
            },
        ]
    );
}

#[test]
fn free_variables_are_read_but_never_bound() {
    assert_eq!(
        free_variables("$who + $x | . as $y | $y + $ENV.A + $__loc__.line + $who"),
        ["who", "x"]
    );
}
