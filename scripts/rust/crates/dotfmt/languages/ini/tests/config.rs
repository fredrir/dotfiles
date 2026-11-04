#![forbid(unsafe_code)]

use std::path::PathBuf;

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_ini::config::{Config, IndentType, LineEndings};
use dotfmt_ini::dialect::Dialect;

fn settings(values: &[(&str, &str, bool)]) -> Settings {
    values
        .iter()
        .map(|(key, value, global)| {
            (
                (*key).to_owned(),
                Setting {
                    value: (*value).to_owned(),
                    source: PathBuf::from("project/dotfmt.dotfile"),
                    line: 7,
                    global: *global,
                },
            )
        })
        .collect()
}

#[test]
fn defaults_are_auto_and_dialect_aware() {
    let config = Config::default();
    assert_eq!(config.dialect, Dialect::Auto);
    assert_eq!(config.indent, None);
    assert_eq!(config.indent_type, None);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert!(config.final_newline);
    assert_eq!(config.blank_lines, 1);
    assert!(config.align);
    assert_eq!(config.align_max, 24);
    assert!(!config.format_comments);
}

#[test]
fn global_width_and_quote_style_are_accepted_and_ignored() {
    let settings = settings(&[("width", "120", true), ("quote_style", "single", true)]);
    let config = Config::from_settings(&settings).unwrap();
    assert_eq!(config, Config::default());
}

#[test]
fn global_indent_and_final_newline_apply() {
    let config = Config::from_settings(&settings(&[
        ("indent", "4", true),
        ("final_newline", "false", true),
    ]))
    .unwrap();
    assert_eq!(config.indent, Some(4));
    assert!(!config.final_newline);
}

#[test]
fn every_local_setting_is_read() {
    let config = Config::from_settings(&settings(&[
        ("dialect", "editorconfig", false),
        ("indent", "0", false),
        ("indent_type", "tabs", false),
        ("line_endings", "windows", false),
        ("final_newline", "false", false),
        ("blank_lines", "0", false),
        ("align", "false", false),
        ("align_max", "1000", false),
        ("format_comments", "true", false),
    ]))
    .unwrap();
    assert_eq!(config.dialect, Dialect::EditorConfig);
    assert_eq!(config.indent, Some(0));
    assert_eq!(config.indent_type, Some(IndentType::Tabs));
    assert_eq!(config.line_endings, LineEndings::Windows);
    assert!(!config.final_newline);
    assert_eq!(config.blank_lines, 0);
    assert!(!config.align);
    assert_eq!(config.align_max, 1000);
    assert!(config.format_comments);
}

#[test]
fn blank_lines_and_align_max_enforce_their_ranges() {
    for (key, value, expected) in [
        ("blank_lines", "11", "blank_lines must be between 0 and 10"),
        ("align_max", "1001", "align_max must be between 0 and 1000"),
    ] {
        let error = Config::from_settings(&settings(&[(key, value, false)])).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Configuration);
        assert_eq!(error.path, Some(PathBuf::from("project/dotfmt.dotfile")));
        assert_eq!(error.line, Some(7));
        assert!(error.message.contains(expected), "{error}");
    }
}

#[test]
fn invalid_choices_name_the_setting() {
    for (key, value, message) in [
        (
            "indent_type",
            "wide",
            "indent_type must be spaces, tabs, not wide",
        ),
        (
            "line_endings",
            "classic",
            "line_endings must be unix, windows, not classic",
        ),
        ("dialect", "nginx", "invalid dialect: nginx"),
        ("align", "yes", "align must be true or false, not yes"),
    ] {
        let error = Config::from_settings(&settings(&[(key, value, false)])).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Configuration);
        assert!(error.message.contains(message), "{error}");
    }
}

#[test]
fn unknown_local_settings_are_configuration_errors_with_a_location() {
    let error = Config::from_settings(&settings(&[("wrap", "80", false)])).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(error.path, Some(PathBuf::from("project/dotfmt.dotfile")));
    assert_eq!(error.line, Some(7));
    assert!(error.message.contains("unknown setting: wrap"), "{error}");
}
