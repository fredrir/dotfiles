#![forbid(unsafe_code)]

use std::fmt::Write as _;

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_env::config::{Config, LineEndings};
use dotfmt_env::{format, format_bytes};

#[test]
fn keeps_interpolation_escapes_and_shell_punctuation() {
    let input = r#"PLAIN=$HOME/bin
DEFAULT=${VAR:-default}
DOLLAR=\$literal
ESCAPE="a\nb"
ESCAPED="back\\slash\tand\"quotes\""
EMPTY=${}
END=value$
HASH=value#fragment
SPACED=value with spaces
COMMAND=`printf '%s' "$HOME"`
"#;
    assert_eq!(format(input, &Config::default()).unwrap(), input);
}

#[test]
fn keeps_quotes_and_hashes_inside_quoted_values() {
    let input = r##"SINGLE='a $b # c'
DOUBLE="a \"quoted\" # c"
COMMAND=`echo "#"`
MULTI="first
second # still text"
"##;
    assert_eq!(format(input, &Config::default()).unwrap(), input);
}

#[test]
fn preserves_duplicate_keys_and_written_order() {
    let input = "A=1\nB=2\nA=3\nB=4\n";
    assert_eq!(format(input, &Config::default()).unwrap(), input);
}

#[test]
fn preserves_comments_in_place() {
    let input = "# top\nA=1 # one\n# middle\nB=2\n# bottom\n";
    assert_eq!(format(input, &Config::default()).unwrap(), input);
}

#[test]
fn preserves_a_generated_matrix_of_value_styles() {
    let mut input = String::new();
    for i in 0..100 {
        match i % 5 {
            0 => writeln!(input, "KEY_{i}=value_{i}").unwrap(),
            1 => writeln!(input, "KEY_{i}=\"value {i} # not comment\"").unwrap(),
            2 => writeln!(input, "KEY_{i}='value ${{{i}}}'").unwrap(),
            3 => writeln!(input, "export KEY_{i}=value\\t{i}").unwrap(),
            _ => writeln!(input, "KEY_{i}=value#{i}").unwrap(),
        }
    }
    let output = format(&input, &Config::default()).unwrap();
    assert_eq!(output, input);
    assert_eq!(format(&output, &Config::default()).unwrap(), output);
}

#[test]
fn preserves_bom_crlf_and_multiline_bytes() {
    let input = "\u{feff}A=\"one\r\ntwo\"\r\nB=2\r\n";
    assert_eq!(
        format(input, &Config::default()).unwrap(),
        "\u{feff}A=\"one\r\ntwo\"\nB=2\n"
    );
    let windows = Config {
        line_endings: LineEndings::Windows,
        ..Config::default()
    };
    assert_eq!(format(input, &windows).unwrap(), input);
}

#[test]
fn preserves_export_prefix_and_its_spacing() {
    assert_eq!(
        format("export  A=1\n", &Config::default()).unwrap(),
        "export  A=1\n"
    );
}

#[test]
fn preserves_verbatim_shell_lines() {
    let input = "if [ -f x ]; then\n  echo \"A=1\"\nfi\n";
    assert_eq!(format(input, &Config::default()).unwrap(), input);
}

#[test]
fn rejects_input_that_cannot_be_classified_without_rewriting() {
    let cases = [
        "A=\"unterminated\n",
        "A='unterminated\n",
        "A=`unterminated\n",
        "A=\"closed\" trailing\n",
        "A='closed'#comment\n",
        "A=\"closed\"#comment\n",
        "A=`closed`#comment\n",
        "A=unquoted\rreturn\n",
    ];
    for input in cases {
        let error = format(input, &Config::default()).unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input:?}");
        assert!(error.line.is_some(), "{input:?}");
        assert!(error.column.is_some(), "{input:?}");
    }
}

#[test]
fn rejects_non_utf8_input() {
    let error = format_bytes(b"A=\xff\n", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.message.contains("UTF-8"));

    assert_eq!(format_bytes(b"A=1\n", &Config::default()).unwrap(), "A=1\n");
}
