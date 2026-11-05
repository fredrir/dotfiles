#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_typescript::config::{Config, FixMode, IndentType, LineEndings, SortOrder};
use dotfmt_typescript::dialect::Dialect;
use oxc_formatter::{
    ArrowParentheses, AttributePosition, Expand, QuoteProperties, QuoteStyle, Semicolons,
    TrailingCommas,
};

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
fn tailwind_paths_keep_the_declaring_config_layer() {
    let mut entries = Settings::new();
    for (key, value, global) in [
        ("tailwind_stylesheet", "styles/app.css", false),
        ("tailwind_config", "tailwind.config.ts", false),
    ] {
        for (key, setting) in settings(key, value, global) {
            entries.insert(key, setting);
        }
    }
    let config = Config::from_settings(&entries).unwrap();
    assert_eq!(
        config.tailwind.stylesheet_base.as_deref(),
        Some(std::path::Path::new("project"))
    );
    assert_eq!(
        config.tailwind.config_base.as_deref(),
        Some(std::path::Path::new("project"))
    );
}

#[test]
fn defaults_match_prettier() {
    let config = Config::default();
    assert_eq!(config.indent, 2);
    assert_eq!(config.indent_type, IndentType::Spaces);
    assert_eq!(config.width, 80);
    assert!(config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert_eq!(config.quote_style, QuoteStyle::Double);
    assert_eq!(config.jsx_quote_style, QuoteStyle::Double);
    assert_eq!(config.quote_properties, QuoteProperties::AsNeeded);
    assert_eq!(config.semicolons, Semicolons::Always);
    assert_eq!(config.trailing_commas, TrailingCommas::All);
    assert_eq!(config.arrow_parentheses, ArrowParentheses::Always);
    assert!(config.bracket_spacing);
    assert!(!config.bracket_same_line);
    assert_eq!(config.attribute_position, AttributePosition::Auto);
    assert_eq!(config.expand, Expand::Auto);
    assert!(!config.jsdoc);
    assert!(!config.sort_imports);
    assert_eq!(config.import_order, SortOrder::Asc);
    assert_eq!(config.fixes, FixMode::Off);
    assert!(config.type_aware);
    assert!(config.verify);
    assert_eq!(config.dialect, Dialect::Auto);
}

#[test]
fn settings_apply_and_unknown_keys_name_the_source() {
    let config = Config::from_settings(&settings("width", "100", false)).unwrap();
    assert_eq!(config.width, 100);

    let error = Config::from_settings(&settings("width", "wide", false)).unwrap_err();
    assert_eq!(
        error.path.as_deref(),
        Some(std::path::Path::new("project/dotfmt.dotfile"))
    );
    assert_eq!(error.line, Some(7));
    assert_eq!(
        error.kind,
        dotfmt_core::diagnostic::DiagnosticKind::Configuration
    );
    assert!(error.message.contains("must be a number"), "{error}");

    let error = Config::from_settings(&settings("typo", "1", true)).unwrap_err();
    assert!(error.message.contains("unknown setting: typo"), "{error}");
}

#[test]
fn every_documented_option_changes_the_configuration() {
    for (key, value) in [
        ("indent", "4"),
        ("width", "120"),
        ("final_newline", "false"),
        ("line_endings", "windows"),
        ("quote_style", "single"),
        ("jsx_quote_style", "single"),
        ("quote_properties", "preserve"),
        ("semicolons", "as-needed"),
        ("trailing_commas", "es5"),
        ("arrow_parentheses", "as-needed"),
        ("bracket_spacing", "false"),
        ("bracket_same_line", "true"),
        ("attribute_position", "multiline"),
        ("expand", "never"),
        ("jsdoc", "true"),
        ("sort_imports", "true"),
        ("import_order", "desc"),
        ("import_side_effects", "true"),
        ("import_newlines", "false"),
        ("import_partition_by_newline", "true"),
        ("import_partition_by_comment", "true"),
        ("fixes", "unsafe"),
        ("type_aware", "false"),
        ("verify", "false"),
        ("dialect", "tsx"),
    ] {
        Config::from_settings(&settings(key, value, true))
            .unwrap_or_else(|error| panic!("{key}={value}: {error}"));
    }
    assert_eq!(
        Config::from_settings(&settings("indent_type", "tabs", false))
            .unwrap()
            .indent_type,
        IndentType::Tabs
    );
    assert_eq!(
        Config::from_settings(&settings("fixes", "safe", false))
            .unwrap()
            .fixes,
        FixMode::Safe
    );
    assert_eq!(
        Config::from_settings(&settings("dialect", "jsx", false))
            .unwrap()
            .dialect,
        Dialect::Jsx
    );
}

#[test]
fn quote_style_auto_variants_resolve_for_global_and_local_settings() {
    for (value, expected) in [
        ("auto", QuoteStyle::Double),
        ("auto-prefer-double", QuoteStyle::Double),
        ("auto-prefer-single", QuoteStyle::Single),
    ] {
        for global in [true, false] {
            let config = Config::from_settings(&settings("quote_style", value, global)).unwrap();
            assert_eq!(config.quote_style, expected, "{value} global={global}");
        }
    }
}

#[test]
fn global_layout_values_clamp_but_local_settings_stay_strict() {
    for indent in 25..=40 {
        let value = indent.to_string();
        let config = Config::from_settings(&settings("indent", &value, true)).unwrap();
        assert_eq!(config.indent, 24, "indent={indent}");
    }
    for width in [321, 400, 10_000] {
        let value = width.to_string();
        let config = Config::from_settings(&settings("width", &value, true)).unwrap();
        assert_eq!(config.width, 320, "width={width}");
    }
    assert!(Config::from_settings(&settings("indent", "25", false)).is_err());
    assert!(Config::from_settings(&settings("width", "321", false)).is_err());
}

#[test]
fn invalid_enum_values_are_rejected() {
    for (key, value) in [
        ("indent", "25"),
        ("width", "0"),
        ("quote_style", "backtick"),
        ("semicolons", "never"),
        ("trailing_commas", "many"),
        ("line_endings", "mac"),
        ("fixes", "sometimes"),
        ("dialect", "coffee"),
    ] {
        assert!(
            Config::from_settings(&settings(key, value, false)).is_err(),
            "{key}={value}"
        );
    }
}

#[test]
fn dialect_resolution_uses_paths_and_explicit_overrides() {
    use std::path::Path;
    assert_eq!(
        Dialect::Auto.resolve(Dialect::Auto, Path::new("a.tsx")),
        Dialect::Tsx
    );
    assert_eq!(
        Dialect::Auto.resolve(Dialect::Auto, Path::new("a.jsx")),
        Dialect::Jsx
    );
    assert_eq!(
        Dialect::Ts.resolve(Dialect::Auto, Path::new("a.jsx")),
        Dialect::Ts
    );
    assert!(
        Dialect::Auto
            .source_type_for_path(Path::new("a.d.ts"))
            .is_typescript_definition()
    );
    assert!(Dialect::parse("tsx", true).is_ok());
    assert!(Dialect::parse("TSX", false).is_err());
}
