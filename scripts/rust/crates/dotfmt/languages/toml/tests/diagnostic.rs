#![forbid(unsafe_code)]

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_toml::{config::Config, format};

#[test]
fn parser_failures_expose_locations() {
    let cases = [
        ("a = \n", 1, 5),
        ("a = \"open\n", 1, 10),
        ("= 1\n", 1, 1),
        ("a = 1\nb\n", 2, 2),
        ("a = 1\na = 2\n", 2, 1),
        ("[a]\nx = 1\n[a]\n", 3, 1),
        ("café = 1\n", 1, 4),
    ];
    for (input, line, column) in cases {
        let error = format(input, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input:?}");
        assert_eq!(
            (error.line, error.column),
            (Some(line), Some(column)),
            "{input:?}: {error}"
        );
    }
}

#[test]
fn parser_messages_are_single_line() {
    let error = format("a = \n", &Config::default()).unwrap_err();
    assert!(!error.message.contains('\n'), "{error}");
    assert!(error.message.contains("invalid string"), "{error}");
}

#[test]
fn excessive_nesting_is_rejected_instead_of_overflowing() {
    for document in [
        format!("[{}]\nx = 1\n", vec!["a"; 80].join(".")),
        format!("{} = 1\n", vec!["b"; 80].join(".")),
    ] {
        let error = format(&document, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{error}");
    }
    let arrays = format!("x = {}1{}\n", "[".repeat(200), "]".repeat(200));
    let error = format(&arrays, &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(1));
}

#[test]
fn nesting_within_the_limit_is_formatted() {
    let document = format!(
        "[{}]\n{} = 1\n",
        vec!["a"; 60].join("."),
        vec!["b"; 60].join(".")
    );
    let output = format(&document, &Config::default()).unwrap();
    assert_eq!(output, document);
}

#[test]
fn comments_inside_an_inline_table_are_a_syntax_error() {
    let error = format("a = { b = 1, # note\n}\n", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
}

#[test]
fn a_bare_carriage_return_is_not_a_line_ending() {
    let error = format("a = 1\rb = 2\n", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax, "{error}");
    assert_eq!((error.line, error.column), (Some(1), Some(6)));
    assert!(!error.message.contains('\r') && !error.message.contains('\n'));
}

#[test]
fn nesting_limits_are_consistent_across_container_kinds() {
    let inline = |levels: usize| {
        let mut value = String::from("1");
        for _ in 0..levels {
            value = format!("{{ k = {value} }}");
        }
        format!("a = {value}\n")
    };
    // 79 inline tables are the most the embedded parser accepts.
    assert!(format(&inline(79), &Config::default()).is_ok());
    let error = format(&inline(80), &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax, "{error}");

    let array = format!("a = {}1{}\n", "[".repeat(79), "]".repeat(79));
    assert!(format(&array, &Config::default()).is_ok());
    let array = format!("a = {}1{}\n", "[".repeat(80), "]".repeat(80));
    let error = format(&array, &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax, "{error}");
}

#[test]
fn extending_an_implicit_table_with_a_dotted_key_is_a_syntax_error() {
    // The printer never invents a header for such a table, because the input
    // cannot exist: a dotted key cannot extend an implicit table.
    let error = format("[t.a.c]\nd = 1\n[t]\na.b = 1\n", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.message.contains("duplicate key"), "{error}");
}
