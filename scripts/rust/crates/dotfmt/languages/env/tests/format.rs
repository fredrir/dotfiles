#![forbid(unsafe_code)]

use dotfmt_core::config::{Setting, Settings};
use dotfmt_env::config::{Config, LineEndings};
use dotfmt_env::format;

fn configured(settings: &str) -> Config {
    let settings: Settings = settings
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(line, text)| {
            let (key, value) = text.split_once('=').unwrap();
            (
                key.trim().to_owned(),
                Setting {
                    value: value.trim().to_owned(),
                    source: "dotfmt.dotfile".into(),
                    line: line + 1,
                    global: false,
                },
            )
        })
        .collect();
    Config::from_settings(&settings).unwrap()
}

fn stable(input: &str, expected: &str, config: &Config) {
    assert_eq!(
        format(input, config).unwrap(),
        expected,
        "formatting {input:?}"
    );
    assert_eq!(
        format(expected, config).unwrap(),
        expected,
        "reformatting {expected:?}"
    );
}

#[test]
fn leaves_canonical_entries_alone() {
    stable("A=1\nB=2\n", "A=1\nB=2\n", &Config::default());
    stable(
        "A=\nB='x y'\nC=\"z\"\nD=`cmd`\n",
        "A=\nB='x y'\nC=\"z\"\nD=`cmd`\n",
        &Config::default(),
    );
}

#[test]
fn preserves_padding_around_the_equals_sign_unless_aligning() {
    // Whitespace around `=` is a word boundary to a shell: `A = 1` and
    // `A= 1` run a command, while `A=1` assigns. Only alignment may rewrite
    // it, and alignment is opt-in.
    stable(
        "A  =   1\nB\t=\t2\n",
        "A  =   1\nB\t=\t2\n",
        &Config::default(),
    );
    stable(
        "LONG_NAME= \"=.bc#:X/XX\"\nA = 1\nB= 2\nC =3\n",
        "LONG_NAME= \"=.bc#:X/XX\"\nA = 1\nB= 2\nC =3\n",
        &Config::default(),
    );
    stable(
        "A  =   1\nB\t=\t2\n",
        "A = 1\nB = 2\n",
        &configured("align = true"),
    );
}

#[test]
fn aligns_equals_within_contiguous_groups() {
    stable(
        "A=1\nLONG=2\n\nB=3\n",
        "A   = 1\nLONG = 2\n\nB = 3\n",
        &configured("align = true"),
    );
    stable(
        "A=1\nLONG=2\n# split\nB=3\n",
        "A   = 1\nLONG = 2\n# split\nB = 3\n",
        &configured("align = true"),
    );
    stable(
        "A=1\nLONG=2\nnote\nB=3\n",
        "A   = 1\nLONG = 2\nnote\nB = 3\n",
        &configured("align = true"),
    );
    stable(
        "export A=1\nB=2\n",
        "export A = 1\nB       = 2\n",
        &configured("align = true"),
    );
}

#[test]
fn alignment_is_capped_by_align_max() {
    stable(
        "A=1\nLONGKEY=2\n",
        "A   = 1\nLONGKEY = 2\n",
        &configured("align = true\nalign_max = 4"),
    );
    stable(
        "A=1\nLONGKEY=2\n",
        "A = 1\nLONGKEY = 2\n",
        &configured("align = true\nalign_max = 0"),
    );
}

#[test]
fn a_blank_ending_a_continuation_survives_blank_lines_zero() {
    let config = configured("blank_lines = 0");
    let input = "A=foo\\\n\nB=2\n";
    stable(input, input, &config);
}

#[test]
fn limits_blank_lines_and_drops_leading_and_trailing_blanks() {
    stable(
        "\n\nA=1\n\n\n\nB=2\n\n\n",
        "A=1\n\nB=2\n",
        &Config::default(),
    );
    stable(
        "\n\nA=1\n\n\n\nB=2\n\n\n",
        "A=1\nB=2\n",
        &configured("blank_lines = 0"),
    );
    stable(
        "A=1\n\n\n\nB=2\n",
        "A=1\n\n\n\nB=2\n",
        &configured("blank_lines = 4"),
    );
    stable(
        "A=1\n\n\n\nB=2\n",
        "A = 1\nB = 2\n",
        &configured("blank_lines = 0\nalign = true"),
    );
}

#[test]
fn formats_comments_only_when_asked() {
    let input = "#note\nA=1  #trailing\n## heading\n#!shebang\n";
    stable(
        input,
        "#note\nA=1 #trailing\n## heading\n#!shebang\n",
        &Config::default(),
    );
    stable(
        input,
        "# note\nA=1 # trailing\n## heading\n#!shebang\n",
        &configured("format_comments = true"),
    );
    stable("  # note\n", "# note\n", &Config::default());
    stable(
        "#note   \nA=1   # c  \n",
        "#note\nA=1 # c\n",
        &Config::default(),
    );
}

#[test]
fn preserves_whitespace_when_trimming_is_disabled() {
    let input = "#note   \nA=1   # c  \nB=2   \n";
    stable(
        input,
        input,
        &configured("trim_trailing_whitespace = false"),
    );
}

#[test]
fn preserves_export_and_its_spacing() {
    stable(
        "export  A=1\nexport B=2\n",
        "export  A=1\nexport B=2\n",
        &Config::default(),
    );
    stable(
        "export A=1\n",
        "export A = 1\n",
        &configured("align = true"),
    );
}

#[test]
fn keeps_hashes_that_are_part_of_a_value() {
    let input = "URL=https://host/#frag\nTOKEN=abc#def\nHASH=#value\nCOMMENTED= #note\n";
    stable(input, input, &Config::default());
    stable(
        "COMMENTED= #note\n",
        "COMMENTED= # note\n",
        &configured("format_comments = true"),
    );
}

#[test]
fn keeps_quotes_byte_for_byte() {
    let input = r#"SINGLE='a b # c'
DOUBLE="a \"quoted\" # c"
BACKTICK=`echo hello`
"#;
    stable(input, input, &Config::default());
}

#[test]
fn keeps_multiline_double_quoted_values() {
    let input = "A=\"line one\nline two # not a comment\"\nB=2\n";
    stable(input, input, &Config::default());
    stable(
        "A=\"one\ntwo\" #note\nB=2\n",
        "A=\"one\ntwo\" # note\nB=2\n",
        &configured("format_comments = true"),
    );
}

#[test]
fn normalizes_structural_crlf_but_not_cr_inside_values() {
    stable("A=1\r\nB=2\r\n", "A=1\nB=2\n", &Config::default());
    stable(
        "A=1\r\nB=2\r\n",
        "A=1\r\nB=2\r\n",
        &configured("line_endings = windows"),
    );

    let multiline = "A=\"x\r\ny\"\r\nB=2\r\n";
    stable(multiline, "A=\"x\r\ny\"\nB=2\n", &Config::default());
    stable(multiline, multiline, &configured("line_endings = windows"));
}

#[test]
fn preserves_a_bom() {
    stable("\u{feff}A=1\n", "\u{feff}A=1\n", &Config::default());
    stable("\u{feff}A=1", "\u{feff}A=1\n", &Config::default());
}

#[test]
fn applies_final_newline() {
    stable("A=1", "A=1\n", &Config::default());
    stable("A=1\n\n", "A=1", &configured("final_newline = false"));
    stable("# note", "# note\n", &Config::default());
    assert_eq!(format("", &Config::default()).unwrap(), "");
    assert_eq!(format("   \n\n", &Config::default()).unwrap(), "   \n\n");
    assert_eq!(format("\u{feff}", &Config::default()).unwrap(), "\u{feff}");
}

#[test]
fn preserves_verbatim_lines() {
    let input = "KEY\n  shell-ish line  \nif [ -f x ]; then\n  echo \"A=1\"\nfi\n";
    stable(input, input, &Config::default());
}

#[test]
fn keeps_empty_values_empty() {
    stable("A=\nB= \nC=   \n", "A=\nB=\nC=\n", &Config::default());
    stable(
        "A=\nB= \nC=   \n",
        "A=\nB= \nC=   \n",
        &configured("trim_trailing_whitespace = false"),
    );
    stable("A =\nB = \nC =   \n", "A =\nB =\nC =\n", &Config::default());
}

#[test]
fn alignment_never_turns_a_value_starting_with_a_hash_into_a_comment() {
    stable(
        "A=#c\nB=# c\nLONG=1\n",
        "A   =#c\nB   =# c\nLONG = 1\n",
        &configured("align = true"),
    );
    stable(
        "A=#frag #note\nB=#frag\n",
        "A =#frag # note\nB =#frag\n",
        &configured("align = true\nformat_comments = true"),
    );
}

#[test]
fn measures_alignment_in_display_width() {
    stable(
        "CLÉ=\"naïve ☕\"\nX=1\n",
        "CLÉ = \"naïve ☕\"\nX  = 1\n",
        &configured("align = true"),
    );
}

#[test]
fn applies_windows_line_endings_from_a_config_value() {
    let config = Config {
        line_endings: LineEndings::Windows,
        ..Config::default()
    };
    assert_eq!(format("A=1\n", &config).unwrap(), "A=1\r\n");
}

#[test]
fn leading_indentation_is_preserved() {
    stable("  A=1\n\tB=2\n", "  A=1\n\tB=2\n", &Config::default());
}
