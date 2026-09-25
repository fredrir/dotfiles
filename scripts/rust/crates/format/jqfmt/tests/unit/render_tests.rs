use crate::render::{Indent, Layout};
use crate::value::Value;

fn laid_out(json: &str) -> String {
    laid_out_with(json, Layout::default())
}

fn laid_out_with(json: &str, layout: Layout) -> String {
    let parsed = crate::parse::parse(json.as_bytes(), crate::parse::Options::default())
        .unwrap_or_else(|problem| panic!("{}: {}", problem.line, problem.message));
    crate::render::write(&parsed.value.expect("the body holds a value"), layout)
}

fn compact() -> Layout {
    Layout {
        indent: Indent::Compact,
        final_newline: true,
    }
}

#[test]
fn a_member_a_line_and_two_spaces_a_level() {
    assert_eq!(
        laid_out("{\"a\":[1,{\"b\":2}],\"c\":{}}"),
        "{\n  \"a\": [\n    1,\n    {\n      \"b\": 2\n    }\n  ],\n  \"c\": {}\n}\n"
    );
}

#[test]
fn an_empty_container_stays_on_one_line() {
    assert_eq!(
        laid_out("{\"a\":{},\"b\":[]}"),
        "{\n  \"a\": {},\n  \"b\": []\n}\n"
    );
    assert_eq!(laid_out("[]"), "[]\n");
    assert_eq!(laid_out("{}"), "{}\n");
}

#[test]
fn indent_zero_is_one_line_and_indent_minus_one_is_a_tab() {
    assert_eq!(
        laid_out_with("{\"a\":[1,{\"b\":2}]}", compact()),
        "{\"a\":[1,{\"b\":2}]}\n"
    );
    assert_eq!(
        laid_out_with(
            "{\"a\":1}",
            Layout {
                indent: Indent::Tabs,
                final_newline: true
            }
        ),
        "{\n\t\"a\": 1\n}\n"
    );
}

#[test]
fn final_newline_off_ends_at_the_last_byte_of_the_value() {
    assert_eq!(
        laid_out_with(
            "{\"a\":1}",
            Layout {
                final_newline: false,
                ..Layout::default()
            }
        ),
        "{\n  \"a\": 1\n}"
    );
}

#[test]
fn a_key_is_written_where_it_was_written_rather_than_in_order() {
    let parsed = crate::parse::parse(
        b"{\"b\":1,\"a\":2,\"c\":3}",
        crate::parse::Options::default(),
    )
    .expect("the body parses");

    assert_eq!(
        parsed.value,
        Some(Value::Object(
            [
                ("b".to_string(), Value::Number("1".into())),
                ("a".to_string(), Value::Number("2".into())),
                ("c".to_string(), Value::Number("3".into())),
            ]
            .into_iter()
            .collect()
        ))
    );
}

#[test]
fn a_string_is_written_with_the_escapes_jq_writes() {
    // The quote, the backslash, the five control characters with names, every
    // other control character as `\u00xx` in lower case, and DEL — which JSON
    // allows raw and jq does not.
    assert_eq!(
        laid_out("\"a\\\"b\\\\c\\bd\\fe\\nf\\rg\\th\\u0001i\\u007fj\""),
        "\"a\\\"b\\\\c\\bd\\fe\\nf\\rg\\th\\u0001i\\u007fj\"\n"
    );
}

#[test]
fn a_character_json_can_hold_raw_is_not_escaped_on_the_way_out() {
    // `\/`, `\u00e9` and a surrogate pair all arrive as themselves, which is
    // what jq writes and what a diff after a format wants to see.
    assert_eq!(laid_out("\"a\\/b\""), "\"a/b\"\n");
    assert_eq!(laid_out("\"\\u00e9\""), "\"é\"\n");
    assert_eq!(laid_out("\"\\ud83d\\ude00\""), "\"😀\"\n");
    assert_eq!(laid_out("\"café\""), "\"café\"\n");
}

#[test]
fn a_number_is_written_as_the_parser_canonicalised_it() {
    assert_eq!(
        laid_out("[1.10,1e2,0.000001]"),
        "[\n  1.10,\n  1E+2,\n  0.000001\n]\n"
    );
}

#[test]
fn a_scalar_stands_alone() {
    assert_eq!(laid_out("null"), "null\n");
    assert_eq!(laid_out("true"), "true\n");
    assert_eq!(laid_out("false"), "false\n");
    assert_eq!(laid_out("\"a\""), "\"a\"\n");
}
