use dotfmt_core::repair::{Fix, Repair, Repairs, apply};

use crate::parse::{Options, Parsed, Problem, parse};
use crate::value::Value;

const REPAIR: Options = Options {
    repair: true,
    commented: false,
};

fn strict(json: &str) -> Result<Parsed, Problem> {
    parse(json.as_bytes(), Options::default())
}

fn repairing(json: &str) -> Result<Parsed, Problem> {
    parse(json.as_bytes(), REPAIR)
}

fn value(json: &str) -> Value {
    strict(json)
        .unwrap_or_else(|problem| panic!("{}: {}", problem.line, problem.message))
        .value
        .expect("the body holds a value")
}

fn repaired(json: &str) -> (Value, Repairs) {
    repaired_bytes(json.as_bytes())
}

fn repaired_bytes(json: &[u8]) -> (Value, Repairs) {
    let parsed = parse(json, REPAIR)
        .unwrap_or_else(|problem| panic!("{}: {}", problem.line, problem.message));
    (
        parsed.value.expect("the body holds a value"),
        parsed.repairs,
    )
}

fn refused(json: &str) -> String {
    match strict(json) {
        Ok(parsed) => panic!("expected a refusal, got {:?}", parsed.value),
        Err(problem) => problem.said(),
    }
}

fn string(text: &str) -> Value {
    Value::String(text.to_string())
}

fn number(text: &str) -> Value {
    Value::Number(text.to_string())
}

fn object(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect(),
    )
}

// ------------------------------------------------------------------- reading

#[test]
fn every_shape_of_value_reads() {
    assert_eq!(value("null"), Value::Null);
    assert_eq!(value("true"), Value::Bool(true));
    assert_eq!(value("false"), Value::Bool(false));
    assert_eq!(value("1"), number("1"));
    assert_eq!(value("\"a\""), string("a"));
    assert_eq!(
        value("[1,true,null]"),
        Value::Array(vec![number("1"), Value::Bool(true), Value::Null])
    );
    assert_eq!(value("{\"a\":1}"), object(&[("a", number("1"))]));
}

#[test]
fn whitespace_around_a_value_is_not_part_of_it() {
    assert_eq!(
        value("\n\t {\"a\" : 1 } \r\n"),
        object(&[("a", number("1"))])
    );
}

#[test]
fn an_empty_body_holds_no_value_rather_than_failing() {
    // jq makes nothing of an empty input and answers with nothing, which is not
    // an error and not a value either.
    for body in ["", "   ", "\n\t\n"] {
        assert_eq!(
            strict(body).expect("nothing to read").value,
            None,
            "{body:?}"
        );
    }
}

#[test]
fn a_repeated_key_keeps_its_first_position_and_its_last_value() {
    // `echo '{"a":1,"b":2,"a":3}' | jq -c .` answers `{"a":3,"b":2}`.
    let mut expected = indexmap::IndexMap::new();
    expected.insert("a".to_string(), number("3"));
    expected.insert("b".to_string(), number("2"));

    assert_eq!(value("{\"a\":1,\"b\":2,\"a\":3}"), Value::Object(expected));
}

#[test]
fn a_string_is_read_as_the_characters_it_names() {
    assert_eq!(value("\"a\\u00e9\\u0041\""), string("aéA"));
    assert_eq!(value("\"a\\/b\""), string("a/b"));
    assert_eq!(value("\"\\ud83d\\ude00\""), string("😀"));
    assert_eq!(value("\"tab\\there\""), string("tab\there"));
    assert_eq!(value("\"café\""), string("café"));
}

#[test]
fn a_mistake_is_reported_where_it_is() {
    // Line and column, so an editor can put the cursor on it.
    let cases = [
        ("{\n  \"a\": 1,\n  @\n}", "3:3: expected a key, found @"),
        ("{\n  \"a\" 1\n}", "2:7: expected : after a key"),
        ("[1, 2", "1:6: expected another array element"),
        ("\"a", "1:3: unterminated string"),
        ("{\"a\": 1} {\"b\": 2}", "1:10: more than one value"),
        ("[@]", "1:2: unexpected @"),
    ];
    for (body, expected) in cases {
        assert_eq!(refused(body), expected, "{body:?}");
    }
}

// ------------------------------------------------------------- refusing JSON

#[test]
fn what_json_refuses_is_refused_where_it_is() {
    let cases = [
        ("{\"a\":1,}", "1:8: stray comma"),
        ("[1,]", "1:4: stray comma"),
        ("// a note\n1", "1:1: comments are not JSON"),
        ("/* a note */ 1", "1:1: comments are not JSON"),
        ("'a'", "1:1: single-quoted strings are not JSON"),
        ("{a:1}", "1:2: expected a key"),
        ("True", "1:1: not a JSON literal: True"),
        ("NaN", "1:1: not a JSON literal: NaN"),
        ("{\"a\":\"b\tc\"}", "1:8: control character in a string"),
        ("\"\\ud800\"", "1:8: lone surrogate"),
        (
            "{\"a\":1;\"b\":2}",
            "1:7: expected , or } between key-value pairs",
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(refused(body), expected, "{body:?}");
    }
}

#[test]
fn an_unexpected_character_is_named_whole() {
    assert_eq!(refused("[\u{201c}a\u{201d}]"), "1:2: unexpected \u{201c}");
    assert_eq!(refused("{\"a\": é}"), "1:7: unexpected é");
    assert_eq!(refused("{é: 1}"), "1:2: expected a key, found é");
}

#[test]
fn a_value_a_json_reader_cannot_hold_is_refused_outright() {
    for body in ["nul", "tru", "falsey", "undefined", "{\"a\":1 \"b\":2}"] {
        let said = refused(body);
        assert!(said.contains(':'), "{body:?} should be reported: {said}");
    }
}

#[test]
fn nesting_past_the_limit_is_a_message_rather_than_a_crash() {
    let deep = format!("{}1{}", "[".repeat(600), "]".repeat(600));

    assert_eq!(refused(&deep), "1:513: too deeply nested");
}

// -------------------------------------------------------------- repairing

#[test]
fn a_trailing_comma_is_counted_and_dropped() {
    let (read, repairs) = repaired("{\"a\":1,\"b\":[1,2,],}");

    assert_eq!(
        read,
        object(&[
            ("a", number("1")),
            ("b", Value::Array(vec![number("1"), number("2")]))
        ])
    );
    assert_eq!(repairs.of(Repair::Comma), 2);
}

#[test]
fn a_comma_between_two_members_is_added_where_it_was_left_out() {
    let (read, repairs) = repaired("{\"a\":1 \"b\":2}");

    assert_eq!(read, object(&[("a", number("1")), ("b", number("2"))]));
    assert_eq!(repairs.of(Repair::MissingComma), 1);
    assert_eq!(
        repaired("[1 2 3]").0,
        Value::Array(vec![number("1"), number("2"), number("3")])
    );
}

#[test]
fn a_comment_is_counted_and_dropped() {
    let (read, repairs) = repaired("// why\n{\n  \"a\": 1, /* and */\n  \"b\": 2\n}\n");

    assert_eq!(read, object(&[("a", number("1")), ("b", number("2"))]));
    assert_eq!(repairs.of(Repair::Comment), 2);
}

#[test]
fn a_single_quoted_string_is_counted_and_rewritten() {
    let (read, repairs) = repaired("'a \"quoted\" \\'b\\''");

    assert_eq!(read, string("a \"quoted\" 'b'"));
    assert_eq!(repairs.of(Repair::Quote), 1);
}

#[test]
fn a_single_quoted_key_is_taken_the_same_way() {
    let (read, repairs) = repaired("{'a':1}");

    assert_eq!(read, object(&[("a", number("1"))]));
    assert_eq!(repairs.of(Repair::Quote), 1);
}

#[test]
fn an_unquoted_key_is_counted_and_quoted() {
    let (read, repairs) = repaired("{a:1,$b_2:2,3:3}");

    assert_eq!(
        read,
        object(&[
            ("a", number("1")),
            ("$b_2", number("2")),
            ("3", number("3"))
        ])
    );
    assert_eq!(repairs.of(Repair::Key), 3);
}

#[test]
fn the_names_python_and_javascript_give_are_counted_and_translated() {
    let (read, repairs) = repaired("[True,False,None,NaN,Infinity,-Infinity,undefined]");

    assert_eq!(
        read,
        Value::Array(vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ])
    );
    assert_eq!(repairs.of(Repair::Literal), 7);
}

#[test]
fn a_byte_order_mark_and_the_spaces_a_word_processor_leaves_are_dropped() {
    let (read, repairs) = repaired("\u{feff}{\u{00a0}\"a\"\u{2009}:1}");

    assert_eq!(read, object(&[("a", number("1"))]));
    assert_eq!(repairs.of(Repair::Space), 3);
}

#[test]
fn a_control_character_and_a_lone_surrogate_become_something_a_reader_can_hold() {
    let (read, repairs) = repaired("[\"a\tb\",\"\\ud800\"]");

    assert_eq!(read, Value::Array(vec![string("a\tb"), string("\u{fffd}")]));
    assert_eq!(repairs.of(Repair::Control), 1);
    assert_eq!(repairs.of(Repair::Surrogate), 1);
}

#[test]
fn a_byte_a_string_cannot_hold_becomes_a_replacement_character() {
    let (read, repairs) = repaired_bytes(b"[1,\"bad\xff byte\"]");

    assert_eq!(
        read,
        Value::Array(vec![number("1"), string("bad\u{fffd} byte")])
    );
    assert_eq!(repairs.of(Repair::Utf8), 1);
}

#[test]
fn a_repair_that_would_have_to_invent_a_value_is_still_a_refusal() {
    for body in [
        "{\"a\":}",
        "{\"a\":",
        "{\"a\"",
        "{\"a\",\"b\":1}",
        "\"unterminated",
        "/* unterminated",
        "[`a${b}`]",
        "[1abc]",
        "[0x]",
    ] {
        assert!(repairing(body).is_err(), "{body:?} should be refused");
    }
}

#[test]
fn curly_quotes_and_backticks_are_read_as_strings() {
    let (read, repairs) = repaired("{\u{201c}a\u{201d}: \u{2018}b\u{2019}, c: `d\ne`}");

    assert_eq!(read, object(&[("a", string("b")), ("c", string("d\ne"))]));
    assert_eq!(repairs.of(Repair::Quote), 3);
    assert_eq!(repairs.of(Repair::Control), 0);
}

#[test]
fn hash_comments_and_code_fences_are_dropped() {
    let (read, repairs) = repaired("```json\n{\"a\": 1 # why\n}\n```\n");

    assert_eq!(read, object(&[("a", number("1"))]));
    assert_eq!(repairs.of(Repair::Fence), 2);
    assert_eq!(repairs.of(Repair::Comment), 1);
}

#[test]
fn keys_with_dashes_and_dots_are_quoted() {
    let (read, repairs) = repaired("{a-b: 1, c.d: 2}");

    assert_eq!(read, object(&[("a-b", number("1")), ("c.d", number("2"))]));
    assert_eq!(repairs.of(Repair::Key), 2);
}

#[test]
fn hex_numbers_become_decimal() {
    let (read, repairs) = repaired("[0x1F, -0X10, +0xff, -0x0]");

    assert_eq!(
        read,
        Value::Array(vec![
            number("31"),
            number("-16"),
            number("255"),
            number("0")
        ])
    );
    assert_eq!(repairs.of(Repair::Number), 4);
}

#[test]
fn semicolons_and_equals_signs_become_separators() {
    let (read, repairs) = repaired("{\"a\" = 1; \"b\": [1; 2]}");

    assert_eq!(
        read,
        object(&[
            ("a", number("1")),
            ("b", Value::Array(vec![number("1"), number("2")]))
        ])
    );
    assert_eq!(repairs.of(Repair::Separator), 3);
}

#[test]
fn a_missing_colon_is_added_before_a_value() {
    let (read, repairs) = repaired("{\"a\" 1, b \"c\"}");

    assert_eq!(read, object(&[("a", number("1")), ("b", string("c"))]));
    assert_eq!(repairs.of(Repair::Colon), 2);
}

#[test]
fn brackets_left_open_at_the_end_are_closed() {
    for (body, expected) in [
        ("{\"a\":1", object(&[("a", number("1"))])),
        ("[1,2,", Value::Array(vec![number("1"), number("2")])),
        (
            "{\"a\":[1,{\"b\":2",
            object(&[(
                "a",
                Value::Array(vec![number("1"), object(&[("b", number("2"))])]),
            )]),
        ),
        ("[", Value::Array(vec![])),
    ] {
        assert_eq!(repaired(body).0, expected, "{body:?}");
    }
}

#[test]
fn a_wrong_closing_bracket_closes_the_container_it_ends() {
    let array = Value::Array(vec![number("1"), number("2")]);
    for body in [
        "{\"a\":[1,2}",
        "{\"a\":[1,2}}",
        "{\"a\":[1,2]]",
        "{\"a\":[1,2]}}",
    ] {
        assert_eq!(
            repaired(body).0,
            object(&[("a", array.clone())]),
            "{body:?}"
        );
    }
    assert_eq!(
        repaired("{\"a\": [1, 2}, \"b\": 3}").0,
        object(&[("a", array.clone()), ("b", number("3"))])
    );
    assert_eq!(
        repaired("[{\"a\":1]").0,
        Value::Array(vec![object(&[("a", number("1"))])])
    );
}

#[test]
fn each_repair_is_located_where_it_was_made() {
    let parsed = repairing("{\n  'a': 1\n  \"b\": True,\n}").unwrap();
    let fixes: Vec<Fix> = parsed.repairs.fixes().to_vec();

    assert_eq!(
        fixes,
        [
            Fix {
                repair: Repair::Quote,
                line: 2,
                column: 3
            },
            Fix {
                repair: Repair::MissingComma,
                line: 2,
                column: 9
            },
            Fix {
                repair: Repair::Literal,
                line: 3,
                column: 8
            },
            Fix {
                repair: Repair::Comma,
                line: 3,
                column: 12
            },
        ]
    );
}

#[test]
fn the_edits_of_a_repair_spell_out_the_same_value_in_strict_json() {
    for body in [
        "{\"a\":1 \"b\":2,}",
        "// why\n{a: 'b', /* c */ \"d\": [1,,2,]}",
        "\u{feff}{\u{00a0}\"a\"\u{2009}:1}",
        "[True,None,NaN,-Infinity,undefined]",
        "[\"a\\'b\", \"c\td\", \"\\ud800\"]",
        "```\n{\u{201c}a\u{201d}: `b`} # tail\n```",
        "{a-b: 0x1F; \"c\" = [1 2}",
        "{\"a\" 1, \"b\": [1, {\"c\": 2",
        "[1]]",
    ] {
        let parsed = repairing(body).unwrap_or_else(|problem| panic!("{body:?}: {problem:?}"));
        let edited = apply(body.as_bytes(), &parsed.edits);
        let strict = parse(&edited, Options::default()).unwrap_or_else(|problem| {
            panic!(
                "{body:?} edited to {:?}: {problem:?}",
                String::from_utf8_lossy(&edited)
            )
        });
        assert_eq!(strict.value, parsed.value, "{body:?}");
    }
}

#[test]
fn a_commented_dialect_keeps_comments_and_trailing_commas() {
    let commented = Options {
        repair: true,
        commented: true,
    };
    let body = "// why\n{\"a\": 1 /* c */ \"b\": [.5,],}";
    let parsed = parse(body.as_bytes(), commented).unwrap();
    let edited = String::from_utf8(apply(body.as_bytes(), &parsed.edits)).unwrap();

    assert_eq!(edited, "// why\n{\"a\": 1, /* c */ \"b\": [0.5,],}");
    assert_eq!(parsed.repairs.of(Repair::MissingComma), 1);
    assert_eq!(parsed.repairs.of(Repair::Number), 1);
    assert_eq!(parsed.repairs.of(Repair::Comment), 0);
    assert_eq!(parsed.repairs.of(Repair::Comma), 0);
}

#[test]
fn a_body_that_needs_nothing_is_left_alone() {
    let (read, repairs) = repaired("{\"a\":[1,2],\"b\":\"c\"}");

    assert_eq!(
        read,
        object(&[
            ("a", Value::Array(vec![number("1"), number("2")])),
            ("b", string("c"))
        ])
    );
    assert!(repairs.is_empty());
}
