#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::config::{Setting, Settings};
use dotfmt_css::config::{Config, LineEndings};
use dotfmt_css::dialect::Dialect;
use dotfmt_css::format;

fn run(input: &str, config: &Config) -> String {
    format(Path::new("style.css"), input, config, Dialect::Auto).unwrap()
}

fn config(entries: &[(&str, &str)]) -> Config {
    let settings: Settings = entries
        .iter()
        .map(|(key, value)| {
            (
                (*key).to_owned(),
                Setting {
                    value: (*value).to_owned(),
                    source: "dotfmt.dotfile".into(),
                    line: 1,
                    global: true,
                },
            )
        })
        .collect();
    Config::from_settings(&settings).unwrap()
}

#[test]
fn a_byte_order_mark_is_preserved() {
    assert_eq!(
        run("\u{feff}a{color:red}", &Config::default()),
        "\u{feff}a {\n  color: red;\n}\n"
    );
    assert_eq!(
        run("\u{feff}a{color:red}\n", &Config::default()),
        "\u{feff}a {\n  color: red;\n}\n"
    );
    assert_eq!(run("\u{feff}", &Config::default()), "\u{feff}");
}

#[test]
fn crlf_input_is_normalized_to_the_configured_line_endings() {
    let input = "a {\r\n  color: red;\r\n}\r\n";
    assert_eq!(run(input, &Config::default()), "a {\n  color: red;\n}\n");
    let windows = config(&[("line_endings", "windows")]);
    assert_eq!(run(input, &windows), "a {\r\n  color: red;\r\n}\r\n");
}

#[test]
fn windows_line_endings_apply_to_every_printed_line() {
    let windows = config(&[("line_endings", "windows")]);
    let output = run("a{color:red;background:blue}", &windows);
    assert!(output.ends_with("\r\n"), "{output:?}");
    assert!(
        output
            .split("\r\n")
            .all(|line| !line.contains('\n') && !line.contains('\r')),
        "{output:?}"
    );
}

#[test]
fn a_bom_is_kept_alongside_windows_line_endings() {
    let windows = config(&[("line_endings", "windows")]);
    assert_eq!(
        run("\u{feff}a{color:red}", &windows),
        "\u{feff}a {\r\n  color: red;\r\n}\r\n"
    );
}

#[test]
fn the_final_newline_setting_controls_exactly_one_trailing_newline() {
    let input = "a{color:red}\n\n\n";
    assert_eq!(run(input, &Config::default()), "a {\n  color: red;\n}\n");
    let none = config(&[("final_newline", "false")]);
    assert_eq!(run(input, &none), "a {\n  color: red;\n}");
    assert_eq!(run("a{color:red}", &none), "a {\n  color: red;\n}");
}

#[test]
fn empty_input_stays_empty_with_any_final_newline_setting() {
    for config in [
        Config::default(),
        config(&[("final_newline", "false")]),
        config(&[("line_endings", "windows")]),
    ] {
        assert_eq!(run("", &config), "");
        assert_eq!(run("   \n\n\t", &config), "");
    }
}

#[test]
fn preservation_choices_remain_idempotent() {
    for config in [
        Config::default(),
        config(&[("final_newline", "false")]),
        config(&[("line_endings", "windows")]),
        config(&[("line_endings", "windows"), ("final_newline", "false")]),
    ] {
        let input = "\u{feff}a{color:red}\r\n\r\n";
        let once = run(input, &config);
        let twice = run(&once, &config);
        assert_eq!(once, twice);
        assert!(once.starts_with('\u{feff}'));
        assert_eq!(
            once.ends_with("\r\n"),
            config.line_endings == LineEndings::Windows && config.final_newline && once.len() > 1
        );
    }
}
