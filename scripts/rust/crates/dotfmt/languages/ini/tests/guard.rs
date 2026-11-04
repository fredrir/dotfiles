#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_ini::config::{Config, LineEndings};
use dotfmt_ini::dialect::Dialect;
use dotfmt_ini::{format, format_with_dialect};

fn check(input: &[u8], path: &str, config: &Config, dialect: Dialect) -> String {
    let output = format(Path::new(path), input, config).unwrap();
    assert_eq!(
        format(Path::new(path), output.as_bytes(), config).unwrap(),
        output,
        "formatting must settle after one pass"
    );
    if dialect != Dialect::Auto {
        assert_eq!(
            format_with_dialect(&output, config, dialect).unwrap(),
            output,
            "the explicit dialect must agree with path detection"
        );
    }
    output
}

#[test]
fn non_utf8_input_is_rejected_as_syntax() {
    let error = format(Path::new("settings.ini"), b"key=\xff\n", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.message.contains("UTF-8"), "{error}");
}

#[test]
fn bom_is_preserved_and_included_in_the_guard() {
    let input = "\u{feff}[section]\nkey=value\n";
    let output = check(
        input.as_bytes(),
        "settings.ini",
        &Config::default(),
        Dialect::Generic,
    );
    assert!(output.starts_with('\u{feff}'));
    assert_eq!(output, "\u{feff}[section]\nkey = value\n");
}

#[test]
fn crlf_is_normalized_and_line_endings_are_selected_after_the_guard() {
    let unix = Config {
        final_newline: false,
        ..Config::default()
    };
    let input = b"[s]\r\nkey=value\r\n";
    let output = check(input, "settings.ini", &unix, Dialect::Generic);
    assert_eq!(output, "[s]\nkey = value");

    let windows = Config {
        line_endings: LineEndings::Windows,
        ..Config::default()
    };
    let output = check(input, "settings.ini", &windows, Dialect::Generic);
    assert_eq!(output, "[s]\r\nkey = value\r\n");
    assert_eq!(output.matches("\r\n").count(), 2);
}

#[test]
fn lone_carriage_returns_inside_values_are_content() {
    let input = "[s]\nkey=a\rb\n";
    let output = check(
        input.as_bytes(),
        "settings.ini",
        &Config::default(),
        Dialect::Generic,
    );
    assert_eq!(output, "[s]\nkey = a\rb\n");
}

#[test]
fn empty_and_whitespace_only_input_is_returned_unchanged() {
    for input in ["", " ", "\n\n", "\t\n \n", "\u{feff}  \n"] {
        assert_eq!(
            format(
                Path::new("settings.ini"),
                input.as_bytes(),
                &Config::default(),
            )
            .unwrap(),
            input
        );
    }
}

#[test]
fn values_that_look_like_layout_are_not_stripped() {
    let input = "[s]\nkey = \"  # ; @Variant(\\\\0)  \"\nraw line with \\\\ trailing\n";
    let output = check(
        input.as_bytes(),
        "settings.ini",
        &Config::default(),
        Dialect::Generic,
    );
    assert!(output.contains("\"  # ; @Variant(\\\\0)  \""));
    assert!(output.contains("raw line with \\\\ trailing"));
}

#[test]
fn formatting_is_idempotent_across_configurations_and_dialects() {
    let inputs = [
        "[s]\na=1\n\n\nlonger=two\n",
        "Host x\nUser = alice\n#note\n",
        "[Service]\nExecStart = /bin/x --y=z\n",
        "[*]\nindent_size = 2\n",
    ];
    let configs = [
        Config::default(),
        Config {
            blank_lines: 0,
            align: false,
            format_comments: true,
            final_newline: false,
            ..Config::default()
        },
        Config {
            align_max: 0,
            indent_type: Some(dotfmt_ini::config::IndentType::Tabs),
            line_endings: LineEndings::Windows,
            ..Config::default()
        },
    ];
    for input in inputs {
        for config in &configs {
            for dialect in [
                Dialect::Generic,
                Dialect::Git,
                Dialect::EditorConfig,
                Dialect::Systemd,
                Dialect::Ssh,
            ] {
                let output = format_with_dialect(input, config, dialect).unwrap();
                assert_eq!(
                    format_with_dialect(&output, config, dialect).unwrap(),
                    output,
                    "{dialect:?} {input:?}"
                );
            }
        }
    }
}
