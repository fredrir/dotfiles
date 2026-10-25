#![forbid(unsafe_code)]

use std::path::PathBuf;

use dotfmt_core::config::{Setting, Settings};
use dotfmt_latex::config::{Config, IndentType, LineEndings};
use dotfmt_latex::dialect::Dialect;

fn settings(values: &[(&str, &str, bool)]) -> Settings {
    values
        .iter()
        .map(|(key, value, global)| {
            (
                (*key).to_owned(),
                Setting {
                    value: (*value).to_owned(),
                    source: PathBuf::from("dotfmt.dotfile"),
                    line: 1,
                    global: *global,
                },
            )
        })
        .collect()
}

#[test]
fn defaults_follow_the_documented_engine_defaults() {
    let config = Config::default();
    assert_eq!(config.width, 80);
    assert_eq!(config.indent, 2);
    assert_eq!(config.indent_type, IndentType::Spaces);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert!(config.wrap);
    assert!(!config.format_tables);
    assert_eq!(config.blank_lines, 1);
    assert!(!config.final_newline);
    assert!(!config.verify);
    assert_eq!(config.dialect, Dialect::Auto);
    assert_eq!(config.wrap_chars, vec![' ']);
}

#[test]
fn global_settings_are_inherited_and_quote_style_is_ignored() {
    let config = Config::from_settings(&settings(&[
        ("width", "96", true),
        ("indent", "4", true),
        ("final_newline", "true", true),
        ("quote_style", "single", true),
    ]))
    .unwrap();
    assert_eq!(config.width, 96);
    assert_eq!(config.indent, 4);
    assert!(config.final_newline);
}

#[test]
fn local_settings_override_global_settings() {
    let config =
        Config::from_settings(&settings(&[("width", "96", true), ("width", "120", false)]))
            .unwrap();
    assert_eq!(config.width, 120);
}

#[test]
fn lists_are_comma_separated_and_wrap_chars_always_keep_spaces() {
    let config = Config::from_settings(&settings(&[
        ("lists", "mylist, myother", false),
        ("verbatims", "myverbatim", false),
        ("no_indent_envs", "mydocument", false),
        ("wrap_chars", ",;", false),
    ]))
    .unwrap();
    assert_eq!(config.lists, ["mylist", "myother"]);
    assert_eq!(config.verbatims, ["myverbatim"]);
    assert_eq!(config.no_indent_envs, ["mydocument"]);
    assert_eq!(config.wrap_chars, [',', ';', ' ']);
}

#[test]
fn invalid_settings_are_rejected_with_the_setting_name() {
    for (key, value, expected) in [
        ("indent", "0", "indent"),
        ("width", "10001", "width"),
        ("wrap_min", "10001", "wrap_min"),
        ("indent_type", "mixed", "indent_type"),
        ("line_endings", "crlf", "line_endings"),
        ("blank_lines", "11", "blank_lines"),
        ("dialect", "unknown", "dialect"),
        ("unknown", "true", "unknown setting"),
    ] {
        let error = Config::from_settings(&settings(&[(key, value, false)])).unwrap_err();
        assert!(error.to_string().contains(expected), "{key}: {error}");
    }
}

#[test]
fn wrap_min_defaults_to_ten_columns_below_the_width() {
    let config = Config {
        width: 80,
        ..Config::default()
    };
    assert_eq!(config.wrap_min(), 70);
    let config = Config {
        width: 8,
        ..Config::default()
    };
    assert_eq!(config.wrap_min(), 1);
    let config = Config {
        width: 80,
        wrap_min: Some(50),
        ..Config::default()
    };
    assert_eq!(config.wrap_min(), 50);
}
