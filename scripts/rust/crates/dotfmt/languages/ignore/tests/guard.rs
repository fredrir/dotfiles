#![forbid(unsafe_code)]

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_ignore::config::Config;
use dotfmt_ignore::dialect::Dialect;
use dotfmt_ignore::format_with_dialect;
use dotfmt_ignore::verify::{Fingerprint, check};

fn fingerprint(input: &str, dialect: Dialect, blank_lines: usize) -> Fingerprint {
    Fingerprint::of(input, dialect, blank_lines)
}

fn allows(before: &str, after: &str, dialect: Dialect, blank_lines: usize) {
    let result = check(
        &fingerprint(before, dialect, blank_lines),
        &fingerprint(after, dialect, blank_lines),
    );
    assert!(result.is_ok(), "{before:?} -> {after:?}: {result:?}");
}

fn rejects(before: &str, after: &str, dialect: Dialect, blank_lines: usize) {
    let error = check(
        &fingerprint(before, dialect, blank_lines),
        &fingerprint(after, dialect, blank_lines),
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Internal);
    assert!(error.message.contains("would change"), "{error}");
}

#[test]
fn pattern_byte_changes_are_rejected() {
    rejects("a.txt\n", "b.txt\n", Dialect::Git, 1);
    rejects("a  b\n", "a b\n", Dialect::Git, 1);
    rejects("a\t\n", "a \n", Dialect::Git, 1);
    rejects("  #x\n", "#x\n", Dialect::Git, 1);
    rejects("  a\n", "a\n", Dialect::Git, 1);
}

#[test]
fn reordering_patterns_is_rejected() {
    rejects("a\n!a\na\n", "a\na\n!a\n", Dialect::Git, 1);
    rejects("z\nm\na\n", "a\nm\nz\n", Dialect::Git, 1);
}

#[test]
fn adding_or_dropping_patterns_is_rejected() {
    rejects("a\nb\n", "a\n", Dialect::Git, 1);
    rejects("a\n", "a\nb\n", Dialect::Git, 1);
    rejects("!a\n", "a\n", Dialect::Git, 1);
}

#[test]
fn dropped_comments_are_rejected() {
    rejects("# note\na\n", "a\n", Dialect::Git, 1);
    rejects("# note\n", "# other\n", Dialect::Git, 1);
}

#[test]
fn adding_blank_lines_is_rejected() {
    rejects("a\nb\n", "a\n\nb\n", Dialect::Git, 1);
    rejects("a\nb\n", "a\n\n\nb\n", Dialect::Git, 1);
}

#[test]
fn collapsing_and_edge_blank_lines_are_allowed() {
    allows("a\n\n\nb\n", "a\n\nb\n", Dialect::Git, 1);
    allows("\n\na\n\n", "a\n", Dialect::Git, 1);
    allows("a\n\nb\n", "a\nb\n", Dialect::Git, 0);
}

#[test]
fn trailing_whitespace_outside_the_pattern_is_allowed() {
    allows("a   \n", "a\n", Dialect::Git, 1);
    allows("a\t\n", "a\n", Dialect::Ignore, 1);
    allows("*.c   text\n", "*.c text\n", Dialect::Attributes, 1);
}

#[test]
fn comment_spacing_is_allowed() {
    allows("#x\n", "# x\n", Dialect::Git, 1);
    allows("#   spaced\n", "# spaced\n", Dialect::Git, 1);
    allows("  #x text\n", "#x text\n", Dialect::Attributes, 1);
}

#[test]
fn tab_only_lines_are_semantic_only_when_the_dialect_says_so() {
    rejects("\t\n", "\n", Dialect::Git, 1);
    allows("\t\n", "\n", Dialect::Prettier, 1);
    allows("\t\n", "\n", Dialect::Attributes, 1);
}

#[test]
fn attribute_pattern_and_value_changes_are_rejected() {
    rejects("*.c text\n", "*.h text\n", Dialect::Attributes, 1);
    rejects("*.c text\n", "*.c -text\n", Dialect::Attributes, 1);
    rejects(
        "*.c diff=sops\n",
        "*.c diff=other\n",
        Dialect::Attributes,
        1,
    );
    rejects("*.c text\n", "*.c text -merge\n", Dialect::Attributes, 1);
}

#[test]
fn attribute_order_is_semantic() {
    rejects(
        "*.c text\n*.c -text\n",
        "*.c -text\n*.c text\n",
        Dialect::Attributes,
        1,
    );
    rejects(
        "[attr]binary -diff\n*.bin binary\n",
        "*.bin binary\n[attr]binary -diff\n",
        Dialect::Attributes,
        1,
    );
}

#[test]
fn opaque_bytes_are_rejected() {
    rejects(
        "\"unterminated text\n",
        "\"unterminated tex\n",
        Dialect::Attributes,
        1,
    );
    rejects("a\\ b text\n", "a\\  b text\n", Dialect::Attributes, 1);
}

#[test]
fn line_endings_and_bom_do_not_change_the_fingerprint() {
    allows("a\r\nb\r\n", "a\nb\n", Dialect::Git, 1);
    allows("\u{feff}a\n", "a\n", Dialect::Git, 1);
}

#[test]
fn fingerprint_mismatch_is_an_internal_error() {
    let error = check(
        &fingerprint("a\n", Dialect::Git, 1),
        &fingerprint("b\n", Dialect::Git, 1),
    )
    .unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Internal);
    assert!(error.message.contains("nothing was written"), "{error}");
}

#[test]
fn formatter_output_keeps_the_semantic_fingerprint() {
    for (dialect, input) in [
        (Dialect::Git, "a\n!a\na\n\n# note\nb.txt   \n"),
        (Dialect::Ignore, "a\n!a\na\t\n"),
        (Dialect::Prettier, "a\n!a\na\t\n"),
        (Dialect::Attributes, "*.c   text\n[attr]binary -diff\n"),
    ] {
        let output = format_with_dialect(input, &Config::default(), dialect).unwrap();
        assert_eq!(
            fingerprint(input, dialect, 1),
            fingerprint(&output, dialect, 1),
            "{dialect:?}"
        );
    }
}
