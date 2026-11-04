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

#[test]
fn preserves_escaped_trailing_whitespace_and_glued_hashes() {
    // A backslash escapes the following whitespace in a shell, so it is value
    // content, not layout. The `#` after an escaped space is not a comment.
    let input =
        "A=foo\\ \nB=bar\\\t\nC=foo\\ #not a comment\nD=foo\\  #comment\nE=foo\\\\ \nF=foo\\\\\t\n";
    let output = format(input, &Config::default()).unwrap();
    assert_eq!(
        output,
        "A=foo\\ \nB=bar\\\t\nC=foo\\ #not a comment\nD=foo\\  #comment\nE=foo\\\\\nF=foo\\\\\n"
    );
    assert_eq!(format(&output, &Config::default()).unwrap(), output);

    let untrimmed = Config {
        trim_trailing_whitespace: false,
        ..Config::default()
    };
    let input = "A=foo\\ \nC=foo\\   #comment\nE=foo\\\\ \n";
    assert_eq!(format(input, &untrimmed).unwrap(), input);
}

#[test]
fn keeps_a_glued_hash_verbatim_when_formatting_comments() {
    let input = "A=foo\\ #not a comment\nB=foo\\  #comment\n";
    let output = format(
        input,
        &Config {
            format_comments: true,
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(output, "A=foo\\ #not a comment\nB=foo\\  # comment\n");
}

#[test]
fn a_non_leading_bom_never_becomes_the_file_bom() {
    let input = "\n\u{feff}A=1\nB=2\n";
    let output = format(input, &Config::default()).unwrap();
    assert_eq!(output, input);
    assert_eq!(format(&output, &Config::default()).unwrap(), output);

    let config = Config {
        blank_lines: 0,
        format_comments: true,
        ..Config::default()
    };
    let input = "\n\n\n\u{feff}#not-a-comment\n";
    let output = format(input, &config).unwrap();
    assert_eq!(output, "\n\u{feff}#not-a-comment\n");
    assert_eq!(format(&output, &config).unwrap(), output);
}

#[test]
fn preserves_two_leading_boms() {
    let input = "\u{feff}\u{feff}A=1\n";
    let output = format(input, &Config::default()).unwrap();
    assert_eq!(output, input);
    assert_eq!(format(&output, &Config::default()).unwrap(), output);
}

#[test]
fn keeps_comment_lines_verbatim_when_a_backslash_joins_them() {
    let input = "A=foo\\\n  #joined\nB=2\n";
    for config in [
        Config::default(),
        Config {
            format_comments: true,
            ..Config::default()
        },
    ] {
        let output = format(input, &config).unwrap();
        assert_eq!(output, input);
        assert_eq!(format(&output, &config).unwrap(), output);
    }

    // With no indentation the joined `#` is part of the value, so formatting
    // the comment text would rewrite that value.
    let glued = "A=foo\\\n#bar\nC=3\n";
    let output = format(
        glued,
        &Config {
            format_comments: true,
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(output, glued);

    // A blank line ends the join; the next comment is ordinary layout again.
    let ended = "A=foo\\\n\n  #standalone\n";
    assert_eq!(
        format(ended, &Config::default()).unwrap(),
        "A=foo\\\n\n#standalone\n"
    );
}

#[test]
fn keeps_joined_comment_trailing_whitespace() {
    let input = "A=foo\\\n  #joined  \nB=2\n";
    assert_eq!(format(input, &Config::default()).unwrap(), input);
}

#[test]
fn generated_matrix_preserves_order_duplicates_and_layout() {
    let mut input = String::new();
    for i in 0..240 {
        let key = ["A", "B", "C", "LONG_NAME"][i % 4];
        match i % 10 {
            0 => writeln!(input, "{key}=value_{i}").unwrap(),
            1 => writeln!(input, "export {key}=value_{i}").unwrap(),
            2 => writeln!(input, "  {key} = value_{i}").unwrap(),
            3 => writeln!(input, "{key}=  value_{i}").unwrap(),
            4 => writeln!(input, "{key}= 'single {i} # text'").unwrap(),
            5 => writeln!(input, "{key}= \"double {i} # text\"").unwrap(),
            6 => writeln!(input, "{key}=#frag_{i}").unwrap(),
            7 => writeln!(input, "{key}=value_{i} # comment").unwrap(),
            8 => writeln!(input, "{key}=https://host/#frag_{i}").unwrap(),
            _ => writeln!(input, "# comment {i}").unwrap(),
        }
    }
    let output = format(&input, &Config::default()).unwrap();
    assert_eq!(output, input);
    assert_eq!(format(&output, &Config::default()).unwrap(), output);
}
