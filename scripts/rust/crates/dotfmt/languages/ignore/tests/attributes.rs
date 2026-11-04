#![forbid(unsafe_code)]

use dotfmt_ignore::config::Config;
use dotfmt_ignore::dialect::Dialect;
use dotfmt_ignore::format_with_dialect;

fn formatted(input: &str, config: &Config) -> String {
    let output = format_with_dialect(input, config, Dialect::Attributes).expect("format");
    let again = format_with_dialect(&output, config, Dialect::Attributes).expect("format again");
    assert_eq!(again, output, "formatting must settle after one pass");
    output
}

fn default(input: &str) -> String {
    formatted(input, &Config::default())
}

#[test]
fn attribute_fields_are_split_and_joined_with_one_space() {
    assert_eq!(default("*.txt\ttext\teol=lf\n"), "*.txt text eol=lf");
    assert_eq!(default("*.c    text   -merge\n"), "*.c text -merge");
}

#[test]
fn patterns_are_aligned_to_the_widest_pattern() {
    let input = "*.enc    -text -merge diff=sops\n*.enc.*  -text -merge diff=sops\n";
    assert_eq!(
        default(input),
        "*.enc   -text -merge diff=sops\n*.enc.* -text -merge diff=sops"
    );
}

#[test]
fn align_false_uses_one_space() {
    let config = Config {
        align: false,
        ..Config::default()
    };
    let input = "*.enc    -text -merge diff=sops\n*.enc.*  -text -merge diff=sops\n";
    assert_eq!(
        formatted(input, &config),
        "*.enc -text -merge diff=sops\n*.enc.* -text -merge diff=sops"
    );
}

#[test]
fn align_max_caps_the_aligned_column() {
    let input = "*.enc    -text -merge diff=sops\n*.enc.*  -text -merge diff=sops\n";
    let fits = Config {
        align_max: 7,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &fits),
        "*.enc   -text -merge diff=sops\n*.enc.* -text -merge diff=sops"
    );
    // The widest pattern exceeds the cap, so the group falls back to one
    // space, matching the alignment fallback used by other engines.
    let too_small = Config {
        align_max: 5,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &too_small),
        "*.enc -text -merge diff=sops\n*.enc.* -text -merge diff=sops"
    );
    // 0 removes the cap.
    let uncapped = Config {
        align_max: 0,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &uncapped),
        "*.enc   -text -merge diff=sops\n*.enc.* -text -merge diff=sops"
    );
}

#[test]
fn quoted_patterns_keep_their_bytes_and_count_toward_alignment() {
    assert_eq!(default("\"a b.txt\"   text\n"), "\"a b.txt\" text");
    assert_eq!(default("\"a\\\"b\" text\n"), "\"a\\\"b\" text");
    assert_eq!(
        default("\"a b\" text\n*.enc.* text\n"),
        "\"a b\"   text\n*.enc.* text"
    );
}

#[test]
fn attribute_values_are_preserved() {
    assert_eq!(default("*.q text eol=crlf\n"), "*.q text eol=crlf");
    assert_eq!(default("*.z text=foo\\ bar\n"), "*.z text=foo\\ bar");
}

#[test]
fn macro_lines_are_supported_and_aligned() {
    assert_eq!(
        default("[attr]binary -diff -merge -text\n*.bin   binary\n"),
        "[attr]binary -diff -merge -text\n*.bin        binary"
    );
}

#[test]
fn negated_attribute_lines_are_preserved() {
    assert_eq!(default("!*.d text\n"), "!*.d text");
}

#[test]
fn order_and_duplicates_are_preserved() {
    assert_eq!(default("*.c text\n*.c -text\n"), "*.c text\n*.c -text");
}

#[test]
fn pattern_only_lines_keep_their_pattern() {
    assert_eq!(default("*.p   \n"), "*.p");
    let config = Config {
        trim_trailing_whitespace: false,
        ..Config::default()
    };
    assert_eq!(formatted("*.p   \n", &config), "*.p   ");
}

#[test]
fn comments_are_recognized_after_leading_whitespace() {
    assert_eq!(default("\t#c\r\n"), "\t#c");
    let trim = Config {
        trim_leading_whitespace: true,
        ..Config::default()
    };
    assert_eq!(formatted("\t#c\r\n", &trim), "#c");
    let both = Config {
        trim_leading_whitespace: true,
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(formatted("\t#c\r\n", &both), "# c");
}

#[test]
fn leading_whitespace_on_patterns_is_dropped_only_when_configured() {
    assert_eq!(default("  *.c text\n"), "  *.c text");
    let config = Config {
        trim_leading_whitespace: true,
        ..Config::default()
    };
    assert_eq!(formatted("  *.c text\n", &config), "*.c text");
}

#[test]
fn unterminated_quotes_are_opaque() {
    assert_eq!(default("\"unterminated text\n"), "\"unterminated text");
    let config = Config {
        trim_leading_whitespace: true,
        trim_trailing_whitespace: true,
        format_comments: true,
        ..Config::default()
    };
    assert_eq!(
        formatted("\"unterminated text\n", &config),
        "\"unterminated text"
    );
}

#[test]
fn ambiguous_bare_escapes_are_opaque() {
    assert_eq!(default("a\\ b text\n"), "a\\ b text");
    let config = Config {
        trim_trailing_whitespace: true,
        ..Config::default()
    };
    assert_eq!(formatted("*.c text\\   \n", &config), "*.c text\\   ");
}

#[test]
fn blank_lines_are_collapsed() {
    assert_eq!(default("*.c text\n\n\n*.h text\n"), "*.c text\n\n*.h text");
}

#[test]
fn crlf_is_normalized() {
    assert_eq!(default("*.c\ttext\r\n*.h text\r\n"), "*.c text\n*.h text");
}

#[test]
fn comments_do_not_join_the_alignment_group() {
    assert_eq!(
        default("# note   \n*.enc text\n*.enc.* text\n"),
        "# note\n*.enc   text\n*.enc.* text"
    );
}

#[test]
fn trailing_whitespace_can_be_kept_on_comments_and_attributes() {
    let config = Config {
        trim_trailing_whitespace: false,
        ..Config::default()
    };
    assert_eq!(
        formatted("# note  \n*.c text  \n", &config),
        "# note  \n*.c text  "
    );
}

#[test]
fn format_comments_is_off_by_default() {
    assert_eq!(default("#x text\n*.c text\n"), "#x text\n*.c text");
}
