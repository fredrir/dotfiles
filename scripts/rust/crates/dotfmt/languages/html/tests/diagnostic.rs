#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_html::{Config, Dialect, format};

fn syntax_error(input: &str) -> dotfmt_core::diagnostic::Diagnostic {
    format(
        Path::new("broken.html"),
        input,
        &Config::default(),
        Dialect::Html,
    )
    .unwrap_err()
}

#[test]
fn syntax_errors_report_one_based_line_and_column() {
    let error = syntax_error("<div><span></div>");
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(1));
    assert_eq!(error.column, Some(13));
    assert!(error.message.contains("expected close tag"), "{error}");

    let error = syntax_error("<div>\n  <span>\n</div>\n");
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(3));
    assert_eq!(error.column, Some(2));
}

#[test]
fn bom_and_crlf_do_not_shift_reported_locations() {
    let error = syntax_error("\u{feff}<div>\n  <span>\n</div>\n");
    assert_eq!((error.line, error.column), (Some(3), Some(2)));

    let error = syntax_error("<div>\r\n  <span>\r\n</div>\r\n");
    assert_eq!((error.line, error.column), (Some(3), Some(2)));
}

#[test]
fn bare_carriage_return_is_a_syntax_error() {
    let error = syntax_error("<div>\r<span></span></div>");
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.line, Some(1));
    assert_eq!(error.column, Some(6));
    assert!(error.message.contains("carriage return"), "{error}");
}

#[test]
fn invalid_embedded_code_does_not_become_a_document_error() {
    let output = format(
        Path::new("x.html"),
        "<div>\n<script>let = ;</script>\n</div>",
        &Config::default(),
        Dialect::Html,
    )
    .unwrap();
    assert!(output.contains("let = ;"), "{output}");
}

#[test]
fn configuration_errors_keep_the_setting_location() {
    let settings: Settings = [(
        "closing_tag_line_break_for_empty".to_owned(),
        Setting {
            value: "sometimes".to_owned(),
            source: "project/dotfmt.dotfile".into(),
            line: 12,
            global: false,
        },
    )]
    .into();
    let error = Config::from_settings(&settings).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(
        error.path.as_deref(),
        Some(Path::new("project/dotfmt.dotfile"))
    );
    assert_eq!(error.line, Some(12));
    assert!(error.column.is_none());
}
