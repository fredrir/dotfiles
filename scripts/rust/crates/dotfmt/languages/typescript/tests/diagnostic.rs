#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_typescript::config::Config;
use dotfmt_typescript::dialect::Dialect;
use dotfmt_typescript::format;

#[test]
fn syntax_errors_report_line_and_column() {
    let error = format(
        Path::new("a.ts"),
        "\nconst x = ;",
        &Config::default(),
        Dialect::Auto,
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(2));
    assert_eq!(error.column, Some(11));
    assert!(error.message.contains("Unexpected token"), "{error}");
}

#[test]
fn unterminated_strings_and_brackets_keep_parser_locations() {
    for (input, line, column) in [
        ("const x = `unterminated", 1, 11),
        ("function f( {", 1, 14),
        ("const x = 1\nconst y =", 2, 10),
    ] {
        let error =
            format(Path::new("a.ts"), input, &Config::default(), Dialect::Auto).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input}");
        assert_eq!(error.line, Some(line), "{input}");
        assert_eq!(error.column, Some(column), "{input}");
    }
}

#[test]
fn dialect_specific_syntax_is_rejected() {
    let error = format(
        Path::new("a.ts"),
        "const x = <div />;",
        &Config::default(),
        Dialect::Ts,
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.line.is_some());
}
