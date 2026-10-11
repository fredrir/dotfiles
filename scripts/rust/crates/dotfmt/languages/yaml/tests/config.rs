#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_yaml::config::Config;

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
fn final_newline_defaults_on_and_applies_from_global_and_local_settings() {
    assert!(
        Config::from_settings(&Settings::new())
            .unwrap()
            .final_newline
    );
    for global in [true, false] {
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
    assert!(Config::from_settings(&settings("dialect", "auto", false)).is_err());
    let error = Config::from_settings(&settings("dash_spacing", "two", false)).unwrap_err();
    assert!(error.message.contains("one-space, indent"), "{error}");
}

#[test]
fn shared_quote_names_select_the_corresponding_yaml_style() {
    let double = Config::from_settings(&settings("quote_style", "double", true)).unwrap();
    let single = Config::from_settings(&settings("quote_style", "single", false)).unwrap();
    assert_eq!(
        dotfmt_yaml::format("a: 'hello'\n", &double).unwrap(),
        "a: \"hello\"\n"
    );
    assert_eq!(
        dotfmt_yaml::format("a: \"hello\"\n", &single).unwrap(),
        "a: 'hello'\n"
    );
}

#[test]
fn global_width_and_indent_apply() {
    let indent = Config::from_settings(&settings("indent", "4", true)).unwrap();
    assert_eq!(
        dotfmt_yaml::format("a:\n  b: 1\n", &indent).unwrap(),
        "a:\n    b: 1\n"
    );
    let width = Config::from_settings(&settings("width", "10", true)).unwrap();
    assert_eq!(
        dotfmt_yaml::format("a: [one, two, three]\n", &width).unwrap(),
        "a:\n  [\n    one,\n    two,\n    three\n  ]\n"
    );
    assert!(Config::from_settings(&settings("indent", "0", false)).is_err());
}
