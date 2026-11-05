#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::config::{Setting, Settings};
use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_css::config::{Config, IndentType, LineEndings};
use dotfmt_css::dialect::Dialect;
use dotfmt_css::format;
use malva::config::{
    BlockSelectorLineBreak, DeclarationOrder, HexCase, KeyframeSelectorNotation, OperatorLineBreak,
    Quotes,
};

fn settings(entries: &[(&str, &str)]) -> Settings {
    entries
        .iter()
        .map(|(key, value)| {
            (
                (*key).to_owned(),
                Setting {
                    value: (*value).to_owned(),
                    source: "project/dotfmt.dotfile".into(),
                    line: 7,
                    global: false,
                },
            )
        })
        .collect()
}

fn config(entries: &[(&str, &str)]) -> Config {
    Config::from_settings(&settings(entries)).unwrap()
}

fn format_with(config: &Config, input: &str) -> String {
    format(Path::new("style.css"), input, config, Dialect::Auto).unwrap()
}

#[test]
fn defaults_match_the_shared_formatter_defaults() {
    let config = Config::default();
    assert_eq!(config.dialect, Dialect::Auto);
    assert_eq!(config.indent, 2);
    assert_eq!(config.indent_type, IndentType::Spaces);
    assert_eq!(config.width, 80);
    assert!(config.final_newline);
    assert_eq!(config.line_endings, LineEndings::Unix);
    assert!(matches!(config.hex_case, HexCase::Lower));
    assert!(matches!(config.quotes, Quotes::AlwaysDouble));
    assert!(!config.format_comments);
    assert!(config.align_comments);
    assert_eq!(config.single_line_block_threshold, None);
    assert!(matches!(
        config.operator_linebreak,
        OperatorLineBreak::After
    ));
    assert!(matches!(
        config.block_selector_linebreak,
        BlockSelectorLineBreak::Consistent
    ));
    assert!(!config.omit_number_leading_zero);
    assert!(config.keyframe_selector_notation.is_none());
    assert!(!config.prefer_single_line);
    assert!(config.declaration_order.is_none());
    assert!(config.tailwind_enabled());
    assert!(config.tailwind_sorter.is_none());
}

#[test]
fn malva_options_map_every_supported_key() {
    let config = config(&[
        ("indent", "4"),
        ("indent_type", "tabs"),
        ("width", "40"),
        ("hex_case", "upper"),
        ("quotes", "single"),
        ("format_comments", "true"),
        ("align_comments", "false"),
        ("single_line_block_threshold", "2"),
        ("operator_linebreak", "before"),
        ("block_selector_linebreak", "wrap"),
        ("omit_number_leading_zero", "true"),
        ("keyframe_selector_notation", "percentage"),
        ("prefer_single_line", "true"),
        ("declaration_order", "alphabetical"),
    ]);
    let options = config.malva_options();
    assert_eq!(options.layout.indent_width, 4);
    assert!(options.layout.use_tabs);
    assert_eq!(options.layout.print_width, 40);
    assert!(matches!(options.language.hex_case, HexCase::Upper));
    assert!(matches!(options.language.quotes, Quotes::AlwaysSingle));
    assert!(options.language.format_comments);
    assert!(!options.language.align_comments);
    assert_eq!(options.language.single_line_block_threshold, Some(2));
    assert!(matches!(
        options.language.operator_linebreak,
        OperatorLineBreak::Before
    ));
    assert!(matches!(
        options.language.block_selector_linebreak,
        BlockSelectorLineBreak::Wrap
    ));
    assert!(options.language.omit_number_leading_zero);
    assert!(matches!(
        options.language.keyframe_selector_notation,
        Some(KeyframeSelectorNotation::Percentage)
    ));
    assert!(options.language.prefer_single_line);
    assert!(matches!(
        options.language.declaration_order,
        Some(DeclarationOrder::Alphabetical)
    ));
    assert_eq!(options.language.ignore_comment_directive, "prettier-ignore");
    assert_eq!(
        options.language.ignore_file_comment_directive,
        "prettier-ignore-file"
    );
}

#[test]
fn indent_and_indent_type_change_printed_indentation() {
    let spaces = config(&[("indent", "4")]);
    assert_eq!(
        format_with(&spaces, "a{color:red}"),
        "a {\n    color: red;\n}\n"
    );
    let tabs = config(&[("indent_type", "tabs")]);
    assert_eq!(
        format_with(&tabs, "a{color:red}"),
        "a {\n\tcolor: red;\n}\n"
    );
}

#[test]
fn width_and_operator_linebreak_control_wrapping() {
    let input = "@media (min-width: 600px) and (max-width: 900px) {a{color:red}}";
    let before = config(&[("width", "30"), ("operator_linebreak", "before")]);
    let output = format_with(&before, input);
    assert!(
        output.contains("\n    and (max-width: 900px) {"),
        "{output}"
    );
    let after = config(&[("width", "30"), ("operator_linebreak", "after")]);
    let output = format_with(&after, input);
    assert!(
        output.contains(" and\n    (max-width: 900px) {"),
        "{output}"
    );
}

#[test]
fn block_selector_linebreak_always_splits_selectors() {
    let config = config(&[("block_selector_linebreak", "always")]);
    assert_eq!(
        format_with(&config, "a,b,c{color:red}"),
        "a,\nb,\nc {\n  color: red;\n}\n"
    );
}

#[test]
fn single_line_block_threshold_only_collapses_within_the_limit() {
    let collapsed = config(&[("single_line_block_threshold", "2")]);
    assert_eq!(
        format_with(&collapsed, "a{color:red;margin:0}"),
        "a { color: red; margin: 0; }\n"
    );
    let bounded = config(&[("single_line_block_threshold", "1")]);
    assert_eq!(
        format_with(&bounded, "a{color:red;margin:0}"),
        "a {\n  color: red;\n  margin: 0;\n}\n"
    );
    let disabled = config(&[("single_line_block_threshold", "none")]);
    assert_eq!(
        format_with(&disabled, "a{color:red;margin:0}"),
        "a {\n  color: red;\n  margin: 0;\n}\n"
    );
}

#[test]
fn number_and_keyframe_options_match_malva() {
    let omit_zero = config(&[("omit_number_leading_zero", "true")]);
    assert!(format_with(&omit_zero, "a{opacity:0.5}").contains("opacity: .5;"));
    let keyword = config(&[("keyframe_selector_notation", "keyword")]);
    assert_eq!(
        format_with(&keyword, "@keyframes pulse{0%{opacity:.5}100%{opacity:1}}"),
        "@keyframes pulse {\n  from {\n    opacity: 0.5;\n  }\n  to {\n    opacity: 1;\n  }\n}\n"
    );
    let percentage = config(&[("keyframe_selector_notation", "percentage")]);
    assert_eq!(
        format_with(
            &percentage,
            "@keyframes pulse{from{opacity:.5}to{opacity:1}}"
        ),
        "@keyframes pulse {\n  0% {\n    opacity: 0.5;\n  }\n  100% {\n    opacity: 1;\n  }\n}\n"
    );
}

#[test]
fn prefer_single_line_collapses_selector_lists() {
    let config = config(&[("prefer_single_line", "true")]);
    assert_eq!(
        format_with(&config, "a,\nb {\n  color: red;\n  margin: 0;\n}\n"),
        "a, b {\n  color: red;\n  margin: 0;\n}\n"
    );
}

#[test]
fn declaration_order_is_opt_in_and_alphabetical() {
    let input = "a{z-index:1;color:red;align-items:center}";
    let default = format_with(&Config::default(), input);
    assert_eq!(
        default,
        "a {\n  z-index: 1;\n  color: red;\n  align-items: center;\n}\n"
    );
    let none = config(&[("declaration_order", "none")]);
    assert_eq!(format_with(&none, input), default);
    let alphabetical = config(&[("declaration_order", "alphabetical")]);
    assert_eq!(
        format_with(&alphabetical, input),
        "a {\n  align-items: center;\n  color: red;\n  z-index: 1;\n}\n"
    );
}

#[test]
fn values_map_to_language_case_quotes_and_comment_options() {
    let upper = config(&[("hex_case", "upper")]);
    assert!(format_with(&upper, "a{color:#aabbcc}").contains("#AABBCC"));
    let ignore = config(&[("hex_case", "ignore")]);
    assert!(format_with(&ignore, "a{color:#AaBbCc}").contains("#AaBbCc"));

    let single = config(&[("quotes", "single")]);
    assert_eq!(
        format_with(&single, "a{content:\"x\"}"),
        "a {\n  content: 'x';\n}\n"
    );
    let double = config(&[("quotes", "double")]);
    assert_eq!(
        format_with(&double, "a{content:'x'}"),
        "a {\n  content: \"x\";\n}\n"
    );
    let prefer_single = config(&[("quotes", "prefer-single")]);
    assert_eq!(
        format_with(&prefer_single, "a{content:\"x\"}"),
        "a {\n  content: 'x';\n}\n"
    );
    let prefer_double = config(&[("quotes", "prefer-double")]);
    assert_eq!(
        format_with(&prefer_double, "a{content:'x'}"),
        "a {\n  content: \"x\";\n}\n"
    );

    let comments = config(&[("format_comments", "true")]);
    assert_eq!(
        format_with(&comments, "a{/*hello*/color:red}"),
        "a {\n  /* hello */\n  color: red;\n}\n"
    );
}

#[test]
fn final_newline_line_endings_and_dialect_are_parsed() {
    let parsed = config(&[
        ("final_newline", "false"),
        ("line_endings", "windows"),
        ("dialect", "SCSS"),
    ]);
    assert!(!parsed.final_newline);
    assert_eq!(parsed.line_endings, LineEndings::Windows);
    assert_eq!(parsed.dialect, Dialect::Scss);

    let unix = config(&[("line_endings", "lf")]);
    assert_eq!(unix.line_endings, LineEndings::Unix);
    let windows = config(&[("line_endings", "crlf")]);
    assert_eq!(windows.line_endings, LineEndings::Windows);
}

#[test]
fn tailwind_settings_map_modes_order_and_paths() {
    let tailwind = config(&[
        ("tailwind", "off"),
        ("tailwind_order", "project"),
        ("tailwind_dedupe", "true"),
        ("tailwind_merge", "true"),
        ("tailwind_stylesheet", "app.css"),
        ("tailwind_config", "tailwind.config.js"),
    ]);
    assert!(!tailwind.tailwind_enabled());
    assert_eq!(tailwind.tailwind.mode, dotfmt_tailwind::Mode::Off);
    assert_eq!(
        tailwind.tailwind.order,
        dotfmt_tailwind::OrderSource::Project
    );
    assert!(tailwind.tailwind.dedupe);
    assert!(tailwind.tailwind.merge);
    assert_eq!(
        tailwind.tailwind.stylesheet.as_deref(),
        Some(Path::new("app.css"))
    );
    assert_eq!(
        tailwind.tailwind.config_file.as_deref(),
        Some(Path::new("tailwind.config.js"))
    );

    assert_eq!(
        config(&[("tailwind", "on")]).tailwind.mode,
        dotfmt_tailwind::Mode::On
    );
    assert_eq!(
        config(&[("tailwind", "false")]).tailwind.mode,
        dotfmt_tailwind::Mode::Off
    );
    assert_eq!(
        config(&[("tailwind_order", "rust")]).tailwind.order,
        dotfmt_tailwind::OrderSource::Rust
    );
}

#[test]
fn unknown_settings_name_the_source_and_line() {
    let error = Config::from_settings(&settings(&[("unknown_setting", "1")])).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Configuration);
    assert_eq!(
        error.path.as_deref(),
        Some(Path::new("project/dotfmt.dotfile"))
    );
    assert_eq!(error.line, Some(7));
    assert!(error.message.contains("unknown setting"), "{error}");
    assert!(error.message.contains("unknown_setting"), "{error}");
}

#[test]
fn quote_style_auto_variants_map_to_preferred_quotes() {
    for (value, expected) in [
        ("auto", Quotes::PreferDouble),
        ("auto-prefer-double", Quotes::PreferDouble),
        ("auto-prefer-single", Quotes::PreferSingle),
    ] {
        for key in ["quotes", "quote_style"] {
            let config = config(&[(key, value)]);
            assert_eq!(
                std::mem::discriminant(&config.quotes),
                std::mem::discriminant(&expected),
                "{key}={value}"
            );
        }
    }
}

#[test]
fn invalid_values_are_rejected_with_the_key_name() {
    for (key, value, expected) in [
        ("indent", "many", "number"),
        ("width", "0", "between"),
        ("final_newline", "sometimes", "true or false"),
        ("line_endings", "mac", "unix or windows"),
        ("hex_case", "mixed", "ignore, lower or upper"),
        ("quotes", "angled", "quotes must be"),
        ("indent_type", "mixed", "spaces or tabs"),
        ("operator_linebreak", "middle", "before, after"),
        (
            "block_selector_linebreak",
            "sometimes",
            "always, consistent, wrap",
        ),
        (
            "keyframe_selector_notation",
            "pixels",
            "none, keyword or percentage",
        ),
        ("declaration_order", "smacss", "none or alphabetical"),
        ("tailwind", "maybe", "off, on or auto"),
        ("dialect", "stylus", "unknown css dialect"),
    ] {
        let error = Config::from_settings(&settings(&[(key, value)])).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Configuration, "{key}={value}");
        assert_eq!(error.line, Some(7), "{key}={value}");
        assert!(
            error.message.contains(expected) || error.message.contains(key),
            "{key}={value}: {error}"
        );
    }
}
