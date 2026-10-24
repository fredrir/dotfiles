#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_toml::config::{Config, LineEndings};

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
    assert_eq!(config.allowed_blank_lines, 1);
    assert!(!config.align_entries);
    assert!(config.align_comments);
    assert!(config.align_single_comments);
    assert!(config.array_trailing_comma);
    assert!(config.array_auto_expand);
    assert!(config.array_auto_collapse);
    assert!(config.compact_arrays);
    assert!(!config.compact_inline_tables);
    assert!(!config.compact_entries);
    assert!(config.inline_table_expand);
    assert!(!config.indent_tables);
    assert!(!config.indent_entries);
    assert!(!config.reorder_keys);
    assert!(!config.format_comments);
}

#[test]
fn every_setting_overrides_its_default() {
    let config = Config::from_settings(&settings(&[
        ("indent", "4", false),
        ("width", "120", false),
        ("final_newline", "false", false),
        ("line_endings", "windows", false),
        ("allowed_blank_lines", "3", false),
        ("align_entries", "true", false),
        ("align_comments", "false", false),
        ("align_single_comments", "false", false),
        ("array_trailing_comma", "false", false),
        ("array_auto_expand", "false", false),
        ("array_auto_collapse", "false", false),
        ("compact_arrays", "false", false),
        ("compact_inline_tables", "true", false),
        ("compact_entries", "true", false),
        ("inline_table_expand", "false", false),
        ("indent_tables", "true", false),
        ("indent_entries", "true", false),
        ("reorder_keys", "true", false),
        ("format_comments", "true", false),
    ]))
    .unwrap();
    assert_eq!(config.indent, 4);
    assert_eq!(config.width, 120);
    assert!(!config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Windows);
    assert_eq!(config.allowed_blank_lines, 3);
    assert!(config.align_entries);
    assert!(!config.align_comments);
    assert!(!config.align_single_comments);
    assert!(!config.array_trailing_comma);
    assert!(!config.array_auto_expand);
    assert!(!config.array_auto_collapse);
    assert!(!config.compact_arrays);
    assert!(config.compact_inline_tables);
    assert!(config.compact_entries);
    assert!(!config.inline_table_expand);
    assert!(config.indent_tables);
    assert!(config.indent_entries);
    assert!(config.reorder_keys);
    assert!(config.format_comments);
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
fn invalid_settings_name_the_source_and_line() {
    let cases = [
        ("indent", "0"),
        ("indent", "17"),
        ("indent", "two"),
        ("width", "0"),
        ("width", "10001"),
        ("allowed_blank_lines", "11"),
        ("line_endings", "mac"),
        ("align_entries", "yes"),
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
