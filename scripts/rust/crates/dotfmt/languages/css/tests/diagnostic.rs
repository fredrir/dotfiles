#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_css::config::Config;
use dotfmt_css::dialect::Dialect;
use dotfmt_css::format;

fn failure(input: &str, dialect: Dialect) -> dotfmt_core::diagnostic::Diagnostic {
    format(Path::new("style.css"), input, &Config::default(), dialect).unwrap_err()
}

#[test]
fn parser_failures_report_one_based_lines_and_columns() {
    for (input, line, column) in [
        ("a {\n", 2, 1),
        ("a {\n  color red;\n}\n", 2, 9),
        ("a {\n  color: \"unterminated;\n}\n", 2, 10),
        ("a {\n  color: red;\n}\n}\n", 4, 1),
        ("a }\n", 1, 3),
        ("}\n", 1, 1),
        ("/* é */ a }\n", 1, 11),
    ] {
        let error = failure(input, Dialect::Css);
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input:?}");
        assert_eq!(
            (error.line, error.column),
            (Some(line), Some(column)),
            "{input:?}: {error}"
        );
        assert!(!error.message.contains('\n'), "{error}");
        assert_eq!(error.path, None);
    }
}

#[test]
fn a_byte_order_mark_does_not_shift_reported_locations() {
    let error = failure("\u{feff}a {\n", Dialect::Css);
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!((error.line, error.column), (Some(2), Some(1)));
}

#[test]
fn a_bare_carriage_return_is_a_syntax_error_with_a_location() {
    let error = failure("a {\rcolor: red;\r}\r", Dialect::Css);
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!((error.line, error.column), (Some(1), Some(4)));
    assert!(
        error
            .message
            .contains("carriage return must be followed by a line feed"),
        "{error}"
    );
    assert!(!error.message.contains('\r'));
}

#[test]
fn invalid_scss_and_less_report_locations_too() {
    let error = failure("a {\n  color: $;\n}\n", Dialect::Scss);
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.line.is_some());
    assert!(error.column.is_some());

    let error = failure("a {\n  color: @;\n}\n", Dialect::Less);
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.line.is_some());
    assert!(error.column.is_some());
}

#[test]
fn empty_and_whitespace_only_input_are_not_errors() {
    for input in ["", " ", "\n\n", "\t\r\n"] {
        assert_eq!(
            format(
                Path::new("style.css"),
                input,
                &Config::default(),
                Dialect::Auto
            )
            .unwrap(),
            ""
        );
    }
}

#[test]
fn pathological_nesting_is_rejected_instead_of_overflowing() {
    let blocks = format!("{}{}", "a{".repeat(200), "}".repeat(200));
    for input in [
        blocks,
        format!("a{{color:{}1{}}}", "calc(".repeat(200), ")".repeat(200)),
        format!("a{{{}1{}}}", "[".repeat(200), "]".repeat(200)),
    ] {
        let error = failure(&input, Dialect::Css);
        assert_eq!(error.kind, DiagnosticKind::Syntax);
        assert!(
            error.message.contains("nesting deeper than 32 levels"),
            "{error}"
        );
        assert!(error.line.is_some());
        assert!(error.column.is_some());
    }
}

#[test]
fn nesting_within_the_limit_is_formatted() {
    let input = format!("{}color:red;{}", "a{".repeat(12), "}".repeat(12));
    let output = format(
        Path::new("style.css"),
        &input,
        &Config::default(),
        Dialect::Css,
    )
    .unwrap();
    assert!(output.contains("color: red;"), "{output}");

    let input = format!("a{{color:{}1{}}}", "calc(".repeat(20), ")".repeat(20));
    assert!(
        format(
            Path::new("style.css"),
            &input,
            &Config::default(),
            Dialect::Css
        )
        .is_ok()
    );

    let input = (0..12)
        .map(|depth| format!("{}a\n", "  ".repeat(depth)))
        .chain(std::iter::once(format!("{}color: red\n", "  ".repeat(12))))
        .collect::<String>();
    assert!(
        format(
            Path::new("style.sass"),
            &input,
            &Config::default(),
            Dialect::Sass
        )
        .is_ok()
    );
}

#[test]
fn pathological_sass_indentation_is_rejected_instead_of_overflowing() {
    let input = (0..200)
        .map(|depth| format!("{}a\n", "  ".repeat(depth)))
        .chain(std::iter::once(format!("{}color: red\n", "  ".repeat(200))))
        .collect::<String>();
    let error = failure(&input, Dialect::Sass);
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(
        error.message.contains("nesting deeper than 32 levels"),
        "{error}"
    );
    assert!(error.line.is_some());
    assert!(error.column.is_some());
}
