#![forbid(unsafe_code)]

use dotfmt_core::repair::{Fix, Repair};
use dotfmt_lua::{dialect::Dialect, repair};

#[test]
fn commas_missing_between_table_fields_are_added() {
    for (input, expected) in [
        ("local t = {a = 1 b = 2}\n", "local t = {a = 1, b = 2}\n"),
        ("local t = {1 2 3}\n", "local t = {1, 2, 3}\n"),
        (
            "local t = {\n  a = 1\n  b = 2,\n}\n",
            "local t = {\n  a = 1,\n  b = 2,\n}\n",
        ),
        ("f({x = {1 2} y = 3})\n", "f({x = {1, 2}, y = 3})\n"),
    ] {
        assert_eq!(repair(input, Dialect::Lua54).text, expected, "{input:?}");
    }
}

#[test]
fn a_comma_never_lands_inside_a_comment() {
    let repaired = repair("local t = {\n  a = 1 -- one\n  b = 2\n}\n", Dialect::Lua54);
    assert_eq!(repaired.text, "local t = {\n  a = 1 -- one\n  ,b = 2\n}\n");
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
fn other_syntax_errors_backticks_and_valid_input_are_left_alone() {
    for input in [
        "local =\n",
        "if x then\n",
        "local s = `a` {a = 1 b = 2}\n",
        "local t = {a = 1}\n",
    ] {
        let repaired = repair(input, Dialect::Lua54);
        assert_eq!(repaired.text, input);
        assert!(repaired.repairs.is_empty(), "{input:?}");
    }
}
