#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_json::config::Config;
use dotfmt_json::render::Indent;

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
fn out_of_range_shared_indent_clamps_but_local_settings_stay_strict() {
    for indent in 8..=24 {
        let value = indent.to_string();
        let config = Config::from_settings(&settings("indent", &value, true)).unwrap();
        assert_eq!(config.indent, Indent::Spaces(7), "indent={indent}");
    }
    assert!(Config::from_settings(&settings("indent", "8", false)).is_err());
}

#[test]
fn unsupported_global_settings_are_ignored_but_local_settings_are_rejected() {
    assert!(Config::from_settings(&settings("width", "80", true)).is_ok());
    assert!(Config::from_settings(&settings("width", "80", false)).is_err());
}

#[test]
fn json_ignores_quote_style_at_every_layer_and_value() {
    for value in [
        "double",
        "single",
        "auto",
        "auto-prefer-double",
        "auto-prefer-single",
    ] {
        for global in [true, false] {
            assert!(
                Config::from_settings(&settings("quote_style", value, global)).is_ok(),
                "quote_style={value} global={global}"
            );
        }
    }
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
    assert_eq!(Dialect::parse("NDJSON", true), Ok(Dialect::Jsonl));
    assert_eq!(
        Dialect::for_path(std::path::Path::new("rows.jsonlines")),
        Some(Dialect::Jsonl)
    );
}
