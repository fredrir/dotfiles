#![forbid(unsafe_code)]

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_lua::{config::Config, dialect::Dialect, format_with_dialect};

#[test]
fn parser_failures_expose_locations_without_parsing_error_messages() {
    let error =
        format_with_dialect("\n)", &Config::default(), Dialect::Lua54).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(2));
    assert_eq!(error.column, Some(1));
}

#[test]
fn unsupported_interpolated_strings_expose_the_backtick_position() {
    let error =
        format_with_dialect("\nlocal x = `hello`", &Config::default(), Dialect::Lua54).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(2));
    assert_eq!(error.column, Some(11));
    assert!(error.message.contains("--dialect luau"));
}
