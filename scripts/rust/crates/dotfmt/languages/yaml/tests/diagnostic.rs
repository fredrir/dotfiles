#![forbid(unsafe_code)]

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_yaml::{config::Config, format};

#[test]
fn parser_failures_expose_locations() {
    for (input, line, column) in [("a:\n  b: 1\n c: 2\n", 3, 2), ("k: \"open\n", 1, 4)] {
        let error = format(input, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input:?}");
        assert_eq!(
            (error.line, error.column),
            (Some(line), Some(column)),
            "{error}"
        );
    }
}

#[test]
fn excessive_nesting_is_rejected_instead_of_overflowing() {
    let input = format!("{}x\n", "- ".repeat(300));
    let error = format(&input, &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.message.contains("nesting deeper"), "{error}");
}
