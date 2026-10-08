#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_json::config::Config;

fn settings(key: &str, value: &str, global: bool) -> Settings {
    [(
        key.to_owned(),
        Setting {
            value: value.to_owned(),
            source: "project/dotfmt.dotfile".into(),
            line: 7,
            global,
        },
    )]
    .into()
}

#[test]
fn final_newline_applies_from_global_and_local_settings() {
    for global in [true, false] {
        assert!(
            Config::from_settings(&settings("final_newline", "true", global))
                .unwrap()
                .final_newline
        );
        assert!(
            !Config::from_settings(&settings("final_newline", "false", global))
                .unwrap()
                .final_newline
        );
    }
}

#[test]
fn invalid_settings_name_the_source_and_line() {
    let error = Config::from_settings(&settings("final_newline", "sometimes", false)).unwrap_err();
    assert_eq!(
        error.path.as_deref(),
        Some(std::path::Path::new("project/dotfmt.dotfile"))
    );
    assert_eq!(error.line, Some(7));
    assert_eq!(
        error.kind,
        dotfmt_core::diagnostic::DiagnosticKind::Configuration
    );
    assert!(error.message.contains("true or false"), "{error}");
    assert!(Config::from_settings(&settings("typo", "1", false)).is_err());
}

#[test]
fn unsupported_global_settings_are_ignored_but_local_settings_are_rejected() {
    assert!(Config::from_settings(&settings("width", "80", true)).is_ok());
    assert!(Config::from_settings(&settings("width", "80", false)).is_err());
}

#[test]
fn json_quote_style_must_remain_double() {
    assert!(Config::from_settings(&settings("quote_style", "double", false)).is_ok());
    assert!(Config::from_settings(&settings("quote_style", "single", false)).is_err());
}

#[test]
fn dialect_parsing_preserves_aliases_and_configuration_case_sensitivity() {
    use dotfmt_json::dialect::Dialect;
    let expected = "jsonc".parse::<Dialect>().unwrap();
    assert_eq!("json-with-comments".parse::<Dialect>().unwrap(), expected);
    assert!(Config::from_settings(&settings("dialect", "JSONC", false)).is_err());
    assert_eq!(
        Dialect::parse("JSON-WITH-COMMENTS", true).unwrap(),
        expected
    );
    assert!(Dialect::parse("unknown", true).is_err());
}
