#![forbid(unsafe_code)]

use std::path::PathBuf;

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_ignore::config::{Config, LineEndings};
use dotfmt_ignore::dialect::Dialect;

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

fn setting(key: &str, value: &str, global: bool) -> Settings {
    settings(&[(key, value, global)])
}

#[test]
fn defaults_match_the_documented_values() {
    let config = Config::default();
    assert_eq!(config.dialect, Dialect::Auto);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert!(!config.final_newline);
    assert_eq!(config.blank_lines, 1);
    assert!(config.trim_trailing_whitespace);
    assert!(!config.trim_leading_whitespace);
    assert!(!config.format_comments);
    assert!(config.align);
    assert_eq!(config.align_max, 24);
}

#[test]
fn every_dialect_name_and_alias_is_accepted() {
    for (value, expected) in [
        ("auto", Dialect::Auto),
        ("git", Dialect::Git),
        ("gitignore", Dialect::Git),
        ("ignore", Dialect::Ignore),
        ("ripgrep", Dialect::Ignore),
        ("prettier", Dialect::Prettier),
        ("prettierignore", Dialect::Prettier),
        ("vscode", Dialect::Vscode),
        ("vscodeignore", Dialect::Vscode),
        ("attributes", Dialect::Attributes),
        ("gitattributes", Dialect::Attributes),
        ("attr", Dialect::Attributes),
    ] {
        let config = Config::from_settings(&setting("dialect", value, false)).unwrap();
        assert_eq!(config.dialect, expected, "{value}");
    }
}

#[test]
fn invalid_dialects_are_configuration_errors() {
    let error = Config::from_settings(&setting("dialect", "bogus", false)).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(
        error.path.as_deref(),
        Some(std::path::Path::new("project/dotfmt.dotfile"))
    );
    assert_eq!(error.line, Some(7));
    assert!(error.message.contains("dialect must be one of"), "{error}");
    assert!(error.message.contains("bogus"), "{error}");
}

#[test]
fn blank_line_and_alignment_ranges_are_enforced() {
    for value in ["0", "10"] {
        let config = Config::from_settings(&setting("blank_lines", value, false)).unwrap();
        assert_eq!(config.blank_lines, value.parse().unwrap());
    }
    for value in ["11", "100", "no"] {
        let error = Config::from_settings(&setting("blank_lines", value, false)).unwrap_err();
        assert!(
            error.message.contains("between 0 and 10"),
            "{value}: {error}"
        );
    }
    for value in ["0", "1000"] {
        let config = Config::from_settings(&setting("align_max", value, false)).unwrap();
        assert_eq!(config.align_max, value.parse().unwrap());
    }
    for value in ["1001", "-1", "wide"] {
        let error = Config::from_settings(&setting("align_max", value, false)).unwrap_err();
        assert!(
            error.message.contains("between 0 and 1000"),
            "{value}: {error}"
        );
    }
}

#[test]
fn booleans_and_line_endings_are_validated() {
    for (key, expected) in [
        ("final_newline", true),
        ("trim_trailing_whitespace", false),
        ("trim_leading_whitespace", true),
        ("format_comments", true),
        ("align", false),
    ] {
        let value = if expected { "true" } else { "false" };
        let config = Config::from_settings(&setting(key, value, false)).unwrap();
        let actual = match key {
            "final_newline" => config.final_newline,
            "trim_trailing_whitespace" => config.trim_trailing_whitespace,
            "trim_leading_whitespace" => config.trim_leading_whitespace,
            "format_comments" => config.format_comments,
            "align" => config.align,
            _ => unreachable!(),
        };
        assert_eq!(actual, expected, "{key}");
    }
    for key in [
        "final_newline",
        "trim_trailing_whitespace",
        "trim_leading_whitespace",
        "format_comments",
        "align",
    ] {
        let error = Config::from_settings(&setting(key, "yes", false)).unwrap_err();
        assert!(error.message.contains("true or false"), "{key}: {error}");
    }
    let config = Config::from_settings(&setting("line_endings", "windows", false)).unwrap();
    assert_eq!(config.line_endings, LineEndings::Windows);
    let error = Config::from_settings(&setting("line_endings", "mac", false)).unwrap_err();
    assert!(error.message.contains("unix, windows"), "{error}");
}

#[test]
fn unknown_settings_name_the_source_and_line() {
    let error = Config::from_settings(&setting("indent_size", "2", false)).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(
        error.path.as_deref(),
        Some(std::path::Path::new("project/dotfmt.dotfile"))
    );
    assert_eq!(error.line, Some(7));
    assert!(error.message.contains("unknown setting"), "{error}");
}

#[test]
fn global_layout_settings_are_accepted_and_ignored() {
    for (key, value) in [("width", "80"), ("indent", "2"), ("quote_style", "single")] {
        let config = Config::from_settings(&setting(key, value, true)).unwrap();
        assert_eq!(config, Config::default(), "{key}");
    }
    let config = Config::from_settings(&settings(&[
        ("width", "120", true),
        ("indent", "4", true),
        ("quote_style", "double", true),
        ("final_newline", "true", true),
    ]))
    .unwrap();
    assert!(config.final_newline);
    assert_eq!(config.blank_lines, 1);
    assert_eq!(config.align_max, 24);
}

#[test]
fn local_layout_settings_are_rejected() {
    for (key, value) in [("width", "80"), ("indent", "2"), ("quote_style", "single")] {
        let error = Config::from_settings(&setting(key, value, false)).unwrap_err();
        assert!(error.message.contains("unknown setting"), "{key}: {error}");
    }
}

#[test]
fn final_newline_applies_from_global_and_local_settings() {
    for global in [true, false] {
        assert!(
            Config::from_settings(&setting("final_newline", "true", global))
                .unwrap()
                .final_newline
        );
        assert!(
            !Config::from_settings(&setting("final_newline", "false", global))
                .unwrap()
                .final_newline
        );
    }
}

#[test]
fn trim_leading_whitespace_is_accepted_for_every_dialect() {
    for dialect in ["auto", "git", "ignore", "prettier", "vscode", "attributes"] {
        let config = Config::from_settings(&settings(&[
            ("dialect", dialect, false),
            ("trim_leading_whitespace", "true", false),
        ]))
        .unwrap();
        assert!(config.trim_leading_whitespace, "{dialect}");
    }
}

#[test]
fn align_is_accepted_for_every_dialect() {
    for dialect in ["git", "ignore", "prettier", "vscode", "attributes"] {
        let config = Config::from_settings(&settings(&[
            ("dialect", dialect, false),
            ("align", "false", false),
            ("align_max", "0", false),
        ]))
        .unwrap();
        assert!(!config.align, "{dialect}");
        assert_eq!(config.align_max, 0, "{dialect}");
    }
}
