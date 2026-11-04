#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_ignore::config::{Config, LineEndings};
use dotfmt_ignore::dialect::Dialect;
use dotfmt_ignore::{format, format_with_dialect};

fn formatted(input: &str, dialect: Dialect, config: &Config) -> String {
    let output = format_with_dialect(input, config, dialect).expect("format");
    let again = format_with_dialect(&output, config, dialect).expect("format again");
    assert_eq!(again, output, "formatting must settle after one pass");
    output
}

fn default(input: &str, dialect: Dialect) -> String {
    formatted(input, dialect, &Config::default())
}

#[test]
fn blank_line_kinds_are_dialect_specific() {
    assert_eq!(default("   \na\n", Dialect::Git), "a");
    assert_eq!(default("a\n   \nb\n", Dialect::Git), "a\n\nb");
    // A tab-only line is a real pattern in gitignore, so it is never removed.
    assert_eq!(default("a\n\t\nb\n", Dialect::Git), "a\n\t\nb");
    assert_eq!(default("a\n\t\nb\n", Dialect::Vscode), "a\n\t\nb");
    // The other dialects treat it as blank and collapse it.
    for dialect in [Dialect::Ignore, Dialect::Prettier, Dialect::Attributes] {
        assert_eq!(default("a\n\t\nb\n", dialect), "a\n\nb", "{dialect:?}");
    }
}

#[test]
fn blank_runs_are_clamped_and_edge_runs_are_dropped() {
    assert_eq!(default("\n\n\n\na\n\n\n\n", Dialect::Git), "a");
    let no_blanks = Config {
        blank_lines: 0,
        ..Config::default()
    };
    assert_eq!(formatted("a\n\n\nb\n", Dialect::Git, &no_blanks), "a\nb");
    let two_blanks = Config {
        blank_lines: 2,
        ..Config::default()
    };
    assert_eq!(
        formatted("a\n\n\n\nb\n", Dialect::Git, &two_blanks),
        "a\n\n\nb"
    );
}

#[test]
fn whitespace_only_input_is_not_emptied() {
    for input in ["", " ", "\n\n", "   \n\n", "\t\n", " \t \n"] {
        for dialect in Dialect::ALL {
            assert_eq!(default(input, dialect), input, "{dialect:?}: {input:?}");
        }
    }
    let final_newline = Config {
        final_newline: true,
        ..Config::default()
    };
    assert_eq!(
        formatted("   \n\n", Dialect::Git, &final_newline),
        "   \n\n"
    );
}

#[test]
fn comments_gain_one_space_when_configured() {
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted(
            "#x\n#   spaced\n### heading\n#!shebang\n#\n",
            Dialect::Git,
            &config
        ),
        "# x\n# spaced\n### heading\n#!shebang\n#"
    );
    assert_eq!(
        formatted("#x\na\n", Dialect::Git, &Config::default()),
        "#x\na"
    );
}

#[test]
fn comment_trailing_whitespace_is_trimmed() {
    assert_eq!(default("# note   \na   \n", Dialect::Git), "# note\na");
    assert_eq!(default("# note\t\n", Dialect::Ignore), "# note");
}

#[test]
fn escaped_hash_lines_are_patterns_not_comments() {
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted("\\#notacomment  \n", Dialect::Git, &config),
        "\\#notacomment"
    );
    // In gitignore only a `#` at byte zero is a comment.
    assert_eq!(formatted("  #x\n", Dialect::Git, &config), "  #x");
}

#[test]
fn git_trims_only_spaces_and_never_escaped_runs() {
    assert_eq!(default("a.txt   \n", Dialect::Git), "a.txt");
    // A tab is pattern data in gitignore.
    assert_eq!(default("a.txt\t\n", Dialect::Git), "a.txt\t");
    assert_eq!(default("a.txt \t\n", Dialect::Git), "a.txt \t");
    // Escaped or trailing backslash runs are left byte-exact.
    assert_eq!(default("b.txt\\ \n", Dialect::Git), "b.txt\\ ");
    assert_eq!(default("a\\  \n", Dialect::Git), "a\\  ");
    assert_eq!(default("b\\\n", Dialect::Git), "b\\");
    assert_eq!(default("\\ \n", Dialect::Git), "\\ ");
    // Internal spaces are pattern data.
    assert_eq!(default("a  b\n", Dialect::Git), "a  b");
}

#[test]
fn ignore_and_prettier_trim_tabs_but_not_escaped_runs() {
    assert_eq!(default("a.txt\t\n", Dialect::Ignore), "a.txt");
    assert_eq!(default("a.txt\t\n", Dialect::Prettier), "a.txt");
    assert_eq!(default("a\\  \n", Dialect::Prettier), "a\\  ");
    assert_eq!(default("a\\ \n", Dialect::Ignore), "a\\ ");
    // VS Code is handled conservatively: without its parser locally, tabs are
    // kept because they may be pattern data.
    assert_eq!(default("a.txt\t\n", Dialect::Vscode), "a.txt\t");
}

#[test]
fn trim_trailing_whitespace_can_be_disabled() {
    let config = Config {
        trim_trailing_whitespace: false,
        ..Config::default()
    };
    assert_eq!(formatted("a.txt   \n", Dialect::Git, &config), "a.txt   ");
    assert_eq!(formatted("a.txt\t\n", Dialect::Ignore, &config), "a.txt\t");
    assert_eq!(
        formatted("a.txt\t\n", Dialect::Prettier, &config),
        "a.txt\t"
    );
    assert_eq!(
        formatted("*.c text   \n", Dialect::Attributes, &config),
        "*.c text   "
    );
    assert_eq!(formatted("# note   \n", Dialect::Git, &config), "# note   ");
}

#[test]
fn negation_order_and_duplicates_are_preserved() {
    assert_eq!(default("a\n!a\na\n", Dialect::Git), "a\n!a\na");
    assert_eq!(
        default("!foo/bar\n\\!bang\n", Dialect::Git),
        "!foo/bar\n\\!bang"
    );
    let input = "z\nm\na\n!a\na\nb\nc\ny\nx\nq\n";
    let output = default(input, Dialect::Git);
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        vec!["z", "m", "a", "!a", "a", "b", "c", "y", "x", "q"]
    );
}

#[test]
fn crlf_is_normalized_and_a_lone_cr_is_data() {
    assert_eq!(default("a.txt\r\nb.txt\r\n", Dialect::Git), "a.txt\nb.txt");
    let windows = Config {
        line_endings: LineEndings::Windows,
        ..Config::default()
    };
    assert_eq!(
        formatted("a.txt\r\nb.txt\r\n", Dialect::Git, &windows),
        "a.txt\r\nb.txt"
    );
    // The CR is inside the pattern, not a line ending.
    assert_eq!(default("a\rb\n", Dialect::Git), "a\rb");
    let final_newline = Config {
        final_newline: true,
        ..Config::default()
    };
    assert_eq!(formatted("a\rb\n", Dialect::Git, &final_newline), "a\rb\n");
    // Adding LF after a final CR would change it into CRLF, so the trailer is
    // withheld to keep the pattern bytes intact.
    assert_eq!(formatted("a\r", Dialect::Git, &final_newline), "a\r");
}

#[test]
fn a_bom_is_preserved_but_never_added() {
    assert_eq!(default("\u{feff}#x\n", Dialect::Git), "\u{feff}#x");
    assert_eq!(default("#x\n", Dialect::Git), "#x");
    let config = Config {
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted("\u{feff}#x\n", Dialect::Git, &config),
        "\u{feff}# x"
    );
}

#[test]
fn non_utf8_is_a_syntax_error_with_the_path() {
    let error = format(Path::new("repo/.gitignore"), b"a\xff\n", &Config::default()).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert_eq!(error.path.as_deref(), Some(Path::new("repo/.gitignore")));
    assert!(error.message.contains("not UTF-8"), "{error}");
}

#[test]
fn final_newline_is_honoured() {
    assert_eq!(default("a\n", Dialect::Git), "a");
    let final_newline = Config {
        final_newline: true,
        ..Config::default()
    };
    assert_eq!(formatted("a\n", Dialect::Git, &final_newline), "a\n");
    assert_eq!(formatted("a", Dialect::Git, &final_newline), "a\n");
    assert_eq!(formatted("b\\\n", Dialect::Git, &final_newline), "b\\\n");
    assert_eq!(formatted("b\\", Dialect::Prettier, &final_newline), "b\\\n");
}

#[test]
fn path_auto_detection_selects_the_dialect() {
    let config = Config::default();
    let cases = [
        ("repo/.gitignore", "a\t\n", "a\t"),
        ("repo/.git/info/exclude", "a\t\n", "a\t"),
        ("repo/.ignore", "a\t\n", "a"),
        ("repo/ignore", "a\t\n", "a\t"),
        ("repo/.prettierignore", "a\t\n", "a"),
        ("repo/.vscodeignore", "a\t\n", "a\t"),
        ("repo/.gitattributes", "*.c\ttext\n", "*.c text"),
        ("repo/notes.txt", "a\t\n", "a\t"),
    ];
    for (path, input, expected) in cases {
        assert_eq!(
            format(Path::new(path), input.as_bytes(), &config).unwrap(),
            expected,
            "{path}"
        );
    }
}

#[test]
fn leading_whitespace_is_pattern_data_outside_attributes() {
    let config = Config {
        trim_leading_whitespace: true,
        format_comments: true,
        ..Config::default()
    };
    for dialect in [
        Dialect::Git,
        Dialect::Ignore,
        Dialect::Prettier,
        Dialect::Vscode,
    ] {
        assert_eq!(
            formatted("  pattern\n", dialect, &config),
            "  pattern",
            "{dialect:?}"
        );
    }
    assert_eq!(
        formatted("  *.c text\n", Dialect::Attributes, &config),
        "*.c text"
    );
}

#[test]
fn align_is_a_no_op_for_gitignore_family() {
    let config = Config {
        align: false,
        ..Config::default()
    };
    assert_eq!(formatted("a   \nb\n", Dialect::Git, &config), "a\nb");
}
