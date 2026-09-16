use crate::parse::{Options, Parsed, Problem, parse};
use crate::repair::{Repair, Repairs};
use crate::value::Value;

fn strict(json: &str) -> Result<Parsed, Problem> {
    parse(json.as_bytes(), Options::default())
}

fn editor(json: &str) -> Result<Parsed, Problem> {
    parse(json.as_bytes(), Options { editor: true })
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
    let parsed = parse(json, Options { editor: true })
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
fn what_json_refuses_is_refused_with_the_flag_that_would_take_it() {
    // The message names `--editor` only where `--editor` is the answer.
    let cases = [
        ("{\"a\":1,}", "1:8: stray comma; --editor fixes this"),
        ("[1,]", "1:4: stray comma; --editor fixes this"),
        (
            "// a note\n1",
            "1:1: comments are not JSON; --editor fixes this",
        ),
        (
            "/* a note */ 1",
            "1:1: comments are not JSON; --editor fixes this",
        ),
        (
            "'a'",
            "1:1: single-quoted strings are not JSON; --editor fixes this",
        ),
        ("{a:1}", "1:2: expected a key; --editor fixes this"),
        ("True", "1:1: not a JSON literal: True; --editor fixes this"),
        ("NaN", "1:1: not a JSON literal: NaN; --editor fixes this"),
        (
            "{\"a\":\"b\tc\"}",
            "1:8: control character in a string; --editor fixes this",
        ),
        ("\"\\ud800\"", "1:8: lone surrogate; --editor fixes this"),
    ];
    for (body, expected) in cases {
        assert_eq!(refused(body), expected, "{body:?}");
    }
}

#[test]
fn a_mistake_no_flag_can_take_is_reported_without_advice() {
    for body in ["{", "[", "\"\\x41\"", "1 2 3", "@"] {
        let said = refused(body);
        assert!(
            !said.contains("--editor"),
            "{body:?} promised a repair: {said}"
        );
    }
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

// -------------------------------------------------------------- --editor

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
    let (read, repairs) = repaired("[True,False,None,NaN,Infinity,-Infinity]");

    assert_eq!(
        read,
        Value::Array(vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        ])
    );
    assert_eq!(repairs.of(Repair::Literal), 6);
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
fn a_repair_that_would_have_to_guess_is_still_a_refusal() {
    // Missing structural pieces an editor cannot invent: guessing here would
    // write a file nobody wrote.
    for body in [
        "{\"a\" 1}",
        "{\"a\":}",
        "[\"a\",\"b\"",
        "{\"a\":1",
        "/* unterminated",
    ] {
        assert!(editor(body).is_err(), "{body:?} should be refused");
    }
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
