#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_env::config::{Config, LineEndings};

fn settings(entries: &[(&str, &str, bool)]) -> Settings {
    entries
        .iter()
        .enumerate()
        .map(|(line, (key, value, global))| {
            (
                (*key).to_owned(),
                Setting {
                    value: (*value).to_owned(),
                    source: "project/dotfmt.dotfile".into(),
                    line: line + 1,
                    global: *global,
                },
            )
        })
        .collect()
}

fn local(key: &str, value: &str) -> Settings {
    settings(&[(key, value, false)])
}

#[test]
fn defaults_match_the_engine_settings() {
    let config = Config::default();
    assert_eq!(config.indent, 2);
    assert_eq!(config.width, 80);
    assert!(config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert_eq!(config.blank_lines, 1);
    assert!(!config.align);
    assert_eq!(config.align_max, 24);
    assert!(!config.format_comments);
    assert!(config.trim_trailing_whitespace);
}

#[test]
fn every_setting_overrides_its_default() {
    let config = Config::from_settings(&settings(&[
        ("indent", "4", false),
        ("width", "120", false),
        ("final_newline", "false", false),
        ("line_endings", "windows", false),
        ("blank_lines", "3", false),
        ("align", "true", false),
        ("align_max", "1000", false),
        ("format_comments", "true", false),
        ("trim_trailing_whitespace", "false", false),
    ]))
    .unwrap();
    assert_eq!(config.indent, 4);
    assert_eq!(config.width, 120);
    assert!(!config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Windows);
    assert_eq!(config.blank_lines, 3);
    assert!(config.align);
    assert_eq!(config.align_max, 1000);
    assert!(config.format_comments);
    assert!(!config.trim_trailing_whitespace);
}

#[test]
fn shared_globals_apply_and_quote_style_is_ignored() {
    let config = Config::from_settings(&settings(&[
        ("width", "100", true),
        ("indent", "8", true),
        ("final_newline", "false", true),
        ("quote_style", "double", true),
    ]))
    .unwrap();
    assert_eq!(config.width, 100);
    assert_eq!(config.indent, 8);
    assert!(!config.final_newline);
}

#[test]
fn local_settings_win_over_shared_globals() {
    let config =
        Config::from_settings(&settings(&[("width", "100", true), ("width", "60", false)]))
            .unwrap();
    assert_eq!(config.width, 60);
}

#[test]
fn accepts_the_edges_of_every_range() {
    let config = Config::from_settings(&settings(&[
        ("indent", "1", false),
        ("width", "1", false),
        ("blank_lines", "0", false),
        ("align_max", "0", false),
    ]))
    .unwrap();
    assert_eq!(config.indent, 1);
    assert_eq!(config.width, 1);
    assert_eq!(config.blank_lines, 0);
    assert_eq!(config.align_max, 0);

    let config = Config::from_settings(&settings(&[
        ("indent", "16", false),
        ("width", "10000", false),
        ("blank_lines", "10", false),
        ("align_max", "1000", false),
    ]))
    .unwrap();
    assert_eq!(config.indent, 16);
    assert_eq!(config.width, 10000);
    assert_eq!(config.blank_lines, 10);
    assert_eq!(config.align_max, 1000);
}

#[test]
fn invalid_settings_name_the_source_and_line() {
    let cases = [
        ("indent", "0"),
        ("indent", "17"),
        ("indent", "two"),
        ("width", "0"),
        ("width", "10001"),
        ("blank_lines", "11"),
        ("align_max", "1001"),
        ("line_endings", "mac"),
        ("align", "yes"),
        ("format_comments", "yes"),
        ("trim_trailing_whitespace", "yes"),
        ("quote_style", "double"),
        ("unknown_setting", "1"),
    ];
    for (key, value) in cases {
        let error = Config::from_settings(&local(key, value)).unwrap_err();
        assert_eq!(
            error.path.as_deref(),
            Some(std::path::Path::new("project/dotfmt.dotfile")),
            "{key} = {value}"
        );
        assert_eq!(error.line, Some(1), "{key} = {value}");
        assert_eq!(error.kind, DiagnosticKind::Configuration, "{key} = {value}");
        assert!(error.message.contains(key), "{error}");
    }
}
