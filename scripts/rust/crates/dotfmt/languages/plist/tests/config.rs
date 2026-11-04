#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_plist::config::{Config, EmptyStyle, IndentType, LineEndings, PrologStyle};

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
    assert_eq!(config.indent_type, IndentType::Spaces);
    assert_eq!(config.width, 80);
    assert!(config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert_eq!(config.blank_lines, 1);
    assert_eq!(config.data_width, 0);
    assert_eq!(config.prolog, PrologStyle::Preserve);
    assert_eq!(config.self_closing, EmptyStyle::Preserve);
    assert!(!config.sort_keys);
}

#[test]
fn every_setting_overrides_its_default() {
    let config = Config::from_settings(&settings(&[
        ("indent", "4", false),
        ("indent_type", "tabs", false),
        ("width", "120", false),
        ("final_newline", "false", false),
        ("line_endings", "windows", false),
        ("blank_lines", "3", false),
        ("data_width", "76", false),
        ("prolog", "normalize", false),
        ("self_closing", "never", false),
        ("sort_keys", "false", false),
    ]))
    .unwrap();
    assert_eq!(config.indent, 4);
    assert_eq!(config.indent_type, IndentType::Tabs);
    assert_eq!(config.width, 120);
    assert!(!config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Windows);
    assert_eq!(config.blank_lines, 3);
    assert_eq!(config.data_width, 76);
    assert_eq!(config.prolog, PrologStyle::Normalize);
    assert_eq!(config.self_closing, EmptyStyle::Never);
    assert!(!config.sort_keys);
}

#[test]
fn shared_globals_apply_and_quote_style_is_ignored() {
    let config = Config::from_settings(&settings(&[
        ("indent", "4", true),
        ("width", "120", true),
        ("final_newline", "false", true),
        ("quote_style", "double", true),
    ]))
    .unwrap();
    assert_eq!(config.indent, 4);
    assert_eq!(config.width, 120);
    assert!(!config.final_newline);

    let error = Config::from_settings(&local("quote_style", "single")).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert!(error.message.contains("unknown setting: quote_style"));
}

#[test]
fn rejects_out_of_range_and_unknown_settings() {
    for (key, value, expected) in [
        ("indent", "0", "indent must be between 1 and 16, not 0"),
        ("indent", "17", "indent must be between 1 and 16, not 17"),
        ("width", "0", "width must be between 1 and 10000, not 0"),
        (
            "blank_lines",
            "11",
            "blank_lines must be between 0 and 10, not 11",
        ),
        (
            "data_width",
            "121",
            "data_width must be between 0 and 120, not 121",
        ),
        (
            "indent_type",
            "mixed",
            "indent_type must be spaces, tabs, not mixed",
        ),
        (
            "line_endings",
            "mac",
            "line_endings must be unix, windows, not mac",
        ),
        (
            "prolog",
            "drop",
            "prolog must be preserve, normalize, not drop",
        ),
        (
            "self_closing",
            "sometimes",
            "self_closing must be preserve, always, never, not sometimes",
        ),
        (
            "final_newline",
            "yes",
            "final_newline must be true or false, not yes",
        ),
        ("unknown", "1", "unknown setting: unknown"),
    ] {
        let error = Config::from_settings(&local(key, value)).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Configuration, "{key}");
        assert_eq!(error.message, expected, "{key}");
    }
}

#[test]
fn refuses_to_sort_keys() {
    let error = Config::from_settings(&local("sort_keys", "true")).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(
        error.message,
        "sort_keys must be false: reordering keys is not supported"
    );
}
