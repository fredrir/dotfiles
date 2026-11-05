#![forbid(unsafe_code)]

use std::path::PathBuf;

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_html::config::{Config, IndentType, LineEndings};
use markup_fmt::config::{ClosingTagLineBreakForEmpty, Quotes};

fn settings(entries: &[(&str, &str)]) -> Settings {
    entries
        .iter()
        .map(|(key, value)| {
            (
                (*key).to_owned(),
                Setting {
                    value: (*value).to_owned(),
                    source: PathBuf::from("project/dotfmt.dotfile"),
                    line: 7,
                    global: false,
                },
            )
        })
        .collect()
}

#[test]
fn defaults_match_the_documented_markup_options() {
    let config = Config::default();
    assert_eq!(config.indent, 2);
    assert_eq!(config.indent_type, IndentType::Spaces);
    assert_eq!(config.width, 80);
    assert!(config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert!(matches!(config.quotes, Quotes::Double));
    assert!(!config.format_comments);
    assert!(!config.closing_bracket_same_line);
    assert!(matches!(
        config.closing_tag_line_break_for_empty,
        ClosingTagLineBreakForEmpty::Fit
    ));
    assert_eq!(config.max_attrs_per_line, None);
    assert!(!config.prefer_attrs_single_line);
    assert!(config.single_attr_same_line);
    assert!(config.script_indent);
    assert!(config.style_indent);
    assert_eq!(config.tailwind.attributes, ["class", "className"]);
    assert!(config.tailwind_enabled());
}

#[test]
fn markup_options_forward_every_html_specific_option() {
    let config = Config {
        indent: 4,
        indent_type: IndentType::Tabs,
        width: 100,
        quotes: Quotes::Single,
        format_comments: true,
        closing_bracket_same_line: true,
        closing_tag_line_break_for_empty: ClosingTagLineBreakForEmpty::Never,
        max_attrs_per_line: Some(3),
        prefer_attrs_single_line: true,
        single_attr_same_line: false,
        script_indent: false,
        style_indent: false,
        ..Config::default()
    };
    let options = config.markup_options();
    assert_eq!(options.layout.print_width, 100);
    assert!(options.layout.use_tabs);
    assert_eq!(options.layout.indent_width, 4);
    assert!(matches!(options.language.quotes, Quotes::Single));
    assert!(options.language.format_comments);
    assert!(options.language.closing_bracket_same_line);
    assert!(matches!(
        options.language.closing_tag_line_break_for_empty,
        ClosingTagLineBreakForEmpty::Never
    ));
    assert_eq!(
        options.language.max_attrs_per_line.map(|value| value.get()),
        Some(3)
    );
    assert!(options.language.prefer_attrs_single_line);
    assert!(!options.language.single_attr_same_line);
    assert!(!options.language.script_indent);
    assert!(!options.language.style_indent);
    assert_eq!(options.language.ignore_comment_directive, "prettier-ignore");
    assert_eq!(
        options.language.ignore_file_comment_directive,
        "prettier-ignore-file"
    );
}

#[test]
fn settings_cover_layout_options_and_tailwind_attributes() {
    let config = Config::from_settings(&settings(&[
        ("indent", "4"),
        ("indent_type", "tabs"),
        ("width", "100"),
        ("final_newline", "false"),
        ("line_endings", "windows"),
        ("quotes", "single"),
        ("format_comments", "true"),
        ("closing_bracket_same_line", "true"),
        ("closing_tag_line_break_for_empty", "always"),
        ("max_attrs_per_line", "2"),
        ("prefer_attrs_single_line", "true"),
        ("single_attr_same_line", "false"),
        ("script_indent", "false"),
        ("style_indent", "false"),
        ("tailwind", "on"),
        ("tailwind_order", "rust"),
        ("tailwind_attributes", "class, className, tw"),
        ("tailwind_dedupe", "true"),
        ("tailwind_merge", "true"),
        ("dialect", "vue"),
    ]))
    .unwrap();
    assert_eq!(config.indent, 4);
    assert_eq!(config.indent_type, IndentType::Tabs);
    assert_eq!(config.width, 100);
    assert!(!config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Windows);
    assert!(matches!(config.quotes, Quotes::Single));
    assert!(config.format_comments);
    assert!(config.closing_bracket_same_line);
    assert!(matches!(
        config.closing_tag_line_break_for_empty,
        ClosingTagLineBreakForEmpty::Always
    ));
    assert_eq!(config.max_attrs_per_line, Some(2));
    assert!(config.prefer_attrs_single_line);
    assert!(!config.single_attr_same_line);
    assert!(!config.script_indent);
    assert!(!config.style_indent);
    assert!(config.tailwind_enabled());
    assert_eq!(config.tailwind.order, dotfmt_tailwind::OrderSource::Rust);
    assert_eq!(config.tailwind.attributes, ["class", "className", "tw"]);
    assert!(config.tailwind.dedupe);
    assert!(config.tailwind.merge);
    assert_eq!(config.dialect, dotfmt_html::dialect::Dialect::Vue);
}

#[test]
fn invalid_option_values_name_the_setting() {
    for (key, value, expected) in [
        ("indent_type", "wide", "indent_type must be spaces or tabs"),
        ("width", "0", "width must be between 1 and 320"),
        (
            "closing_tag_line_break_for_empty",
            "sometimes",
            "closing_tag_line_break_for_empty must be always, fit or never",
        ),
        (
            "script_indent",
            "sometimes",
            "script_indent must be true or false",
        ),
        ("tailwind", "maybe", "tailwind must be off, on or auto"),
        (
            "tailwind_order",
            "alphabetical",
            "tailwind_order must be auto, project or rust",
        ),
        ("dialect", "razor", "unknown html dialect"),
    ] {
        let error = Config::from_settings(&settings(&[(key, value)])).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Configuration);
        assert!(error.message.contains(expected), "{key}: {error}");
        assert_eq!(error.line, Some(7));
    }
}

#[test]
fn unknown_keys_are_rejected_with_the_setting_source() {
    let error = Config::from_settings(&settings(&[("unknown_key", "1")])).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(
        error.path.as_deref(),
        Some(std::path::Path::new("project/dotfmt.dotfile"))
    );
    assert_eq!(error.line, Some(7));
    assert_eq!(error.message, "unknown setting: unknown_key");
}

#[test]
fn tailwind_can_be_disabled_and_none_clears_the_attribute_limit() {
    let config = Config::from_settings(&settings(&[
        ("tailwind", "off"),
        ("tailwind_attributes", ""),
        ("max_attrs_per_line", "none"),
    ]))
    .unwrap();
    assert!(!config.tailwind_enabled());
    assert!(config.tailwind.attributes.is_empty());
    assert_eq!(config.max_attrs_per_line, None);
}
