#![forbid(unsafe_code)]

use dotfmt_core::repair::{Fix, Repair};
use dotfmt_toml::repair;

#[test]
fn commas_missing_between_array_items_and_inline_table_entries_are_added() {
    for (input, expected) in [
        ("a = [1 2]\n", "a = [1, 2]\n"),
        ("a = [\"x\" \"y\" \"z\"]\n", "a = [\"x\", \"y\", \"z\"]\n"),
        ("a = [\n  1\n  2\n]\n", "a = [\n  1,\n  2\n]\n"),
        ("a = { b = 1 c = 2 }\n", "a = { b = 1, c = 2 }\n"),
        (
            "a = [{ b = 1 } { c = 2 }]\n",
            "a = [{ b = 1 }, { c = 2 }]\n",
        ),
    ] {
        assert_eq!(repair(input).text, expected, "{input:?}");
    }
}

#[test]
fn a_comma_never_lands_inside_a_comment() {
    let repaired = repair("a = [\n  1 # one\n  2\n]\n");
    assert_eq!(repaired.text, "a = [\n  1 # one\n  ,2\n]\n");
    assert_eq!(
        repaired.repairs.fixes(),
        [Fix {
            repair: Repair::MissingComma,
            line: 3,
            column: 3
        }]
    );
}

#[test]
fn a_trailing_comma_in_an_inline_table_is_dropped() {
    let repaired = repair("a = { b = 1, }\n");
    assert_eq!(repaired.text, "a = { b = 1 }\n");
    assert_eq!(repaired.repairs.of(Repair::Comma), 1);
}

#[test]
fn other_syntax_errors_and_valid_input_are_left_alone() {
    for input in ["a = = 1\n", "a = [1, 2\n", "[table\n", "a = { b = 1 }\n"] {
        let repaired = repair(input);
        assert_eq!(repaired.text, input);
        assert!(repaired.repairs.is_empty(), "{input:?}");
    }
}
