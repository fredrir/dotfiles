#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_lua::config::Config;

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
fn out_of_range_shared_globals_clamp_but_local_settings_stay_strict() {
    for indent in 17..=24 {
        let value = indent.to_string();
        let config = Config::from_settings(&settings("indent", &value, true)).unwrap();
        assert_eq!(config.style.indent_width, 16, "indent={indent}");
    }
    let width = Config::from_settings(&settings("width", "10001", true)).unwrap();
    assert_eq!(width.style.column_width, 10_000);
    assert!(Config::from_settings(&settings("indent", "17", false)).is_err());
    assert!(Config::from_settings(&settings("width", "10001", false)).is_err());
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
fn shared_quote_names_select_the_corresponding_lua_style() {
    let double = Config::from_settings(&settings("quote_style", "double", true)).unwrap();
    let single = Config::from_settings(&settings("quote_style", "single", false)).unwrap();
    assert_eq!(
        dotfmt_lua::format("return 'hello'", &double).unwrap(),
        "return \"hello\""
    );
    assert_eq!(
        dotfmt_lua::format("return \"hello\"", &single).unwrap(),
        "return 'hello'"
    );
}

#[test]
fn dialect_parsing_preserves_aliases_and_configuration_case_sensitivity() {
    use dotfmt_lua::dialect::Dialect;
    let expected = "luau".parse::<Dialect>().unwrap();
    assert_eq!("luau".parse::<Dialect>().unwrap(), expected);
    assert!(Config::from_settings(&settings("dialect", "LUAU", false)).is_err());
    assert_eq!(Dialect::parse("LUAU", true).unwrap(), expected);
    assert!(Dialect::parse("unknown", true).is_err());
}
