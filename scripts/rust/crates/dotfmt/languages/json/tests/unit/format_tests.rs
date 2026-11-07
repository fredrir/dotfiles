use std::path::Path;

use crate::config::Config;
use crate::dialect::Dialect;
use crate::format::format;
use dotfmt_core::repair::Repair;

fn written(body: &str) -> String {
    format(
        Path::new("stdin"),
        body.as_bytes(),
        &Config::default(),
        false,
        Dialect::Json,
    )
    .unwrap_or_else(|message| panic!("{message}"))
    .text
}

fn refused(body: &str) -> String {
    match format(
        Path::new("stdin"),
        body.as_bytes(),
        &Config::default(),
        false,
        Dialect::Json,
    ) {
        Ok(formatted) => panic!("expected a refusal, got {:?}", formatted.text),
        Err(message) => message.to_string(),
    }
}

fn refused_with_repairs(body: &str) -> String {
    match format(
        Path::new("stdin"),
        body.as_bytes(),
        &Config::default(),
        true,
        Dialect::Json,
    ) {
        Ok(formatted) => panic!("expected a refusal, got {:?}", formatted.text),
        Err(message) => message.to_string(),
    }
}

#[test]
fn a_body_is_laid_out() {
    assert_eq!(written("{\"a\":1}"), "{\n  \"a\": 1\n}\n");
}

#[test]
fn a_body_that_is_already_laid_out_is_returned_as_it_was() {
    let body = "{\n  \"a\": [\n    1\n  ]\n}\n";

    assert_eq!(written(body), body);
}

#[test]
fn an_empty_file_becomes_an_empty_file_rather_than_a_failure() {
    assert_eq!(written(""), "");
}

#[test]
fn a_body_that_holds_no_value_is_refused_rather_than_emptied() {
    assert_eq!(refused("   \n\n"), "stdin:1:1: expected a value");
    assert_eq!(
        refused("// just a note\n"),
        "stdin:1:1: comments are not JSON"
    );
    assert_eq!(
        refused_with_repairs("// just a note\n"),
        "stdin:1:1: expected a value"
    );
}

#[test]
fn what_jq_refuses_is_refused_with_the_position_it_is_at() {
    assert_eq!(refused("{\n  \"a\": 1,\n}"), "stdin:3:1: stray comma");
    assert_eq!(refused("[1,]"), "stdin:1:4: stray comma");
}

#[test]
fn what_no_repair_can_take_is_a_failure_with_nothing_on_stdout() {
    assert_eq!(refused("{\"a\":}"), "stdin:1:6: unexpected }");
    assert_eq!(refused_with_repairs("{\"a\":}"), "stdin:1:6: unexpected }");
}

#[test]
fn the_repairs_are_counted_for_the_caller_to_report() {
    let formatted = format(
        Path::new("stdin"),
        b"{\n  'a': 1, // why\n}\n",
        &Config::default(),
        true,
        Dialect::Json,
    )
    .expect("the body reads with the flag");

    assert_eq!(formatted.text, "{\n  \"a\": 1\n}\n");
    assert_eq!(formatted.repairs.of(Repair::Quote), 1);
    assert_eq!(formatted.repairs.of(Repair::Comment), 1);
}

fn jsonl(body: &str, config: &Config, repair: bool) -> Result<String, String> {
    format(
        Path::new("stdin"),
        body.as_bytes(),
        config,
        repair,
        Dialect::Jsonl,
    )
    .map(|formatted| formatted.text)
    .map_err(|message| message.to_string())
}

#[test]
fn jsonl_writes_one_compact_record_a_line() {
    let config = Config::default();
    assert_eq!(
        jsonl(
            "{\"a\": 1,\n \"b\": [1, 2]}\n\n\n{\"c\":\"x\\ny\"} 1e2\n",
            &config,
            false
        ),
        Ok("{\"a\":1,\"b\":[1,2]}\n{\"c\":\"x\\ny\"}\n1E+2\n".to_owned())
    );
    let body = "{\"a\":1}\n[true]\n";
    assert_eq!(jsonl(body, &config, false), Ok(body.to_owned()));
}

#[test]
fn jsonl_ignores_indent_and_follows_final_newline() {
    let config = Config {
        indent: crate::render::Indent::Spaces(4),
        final_newline: false,
        ..Config::default()
    };
    assert_eq!(
        jsonl("{\"a\": [1]}\n{}\n", &config, false),
        Ok("{\"a\":[1]}\n{}".to_owned())
    );
}

#[test]
fn jsonl_without_records_is_empty_unless_something_was_written() {
    let config = Config::default();
    assert_eq!(jsonl("", &config, false), Ok(String::new()));
    assert_eq!(jsonl(" \n\n", &config, false), Ok(String::new()));
    assert_eq!(
        jsonl("// just a note\n", &config, true),
        Err("stdin:1:1: expected a value".to_owned())
    );
}

#[test]
fn jsonl_errors_point_at_the_record_and_repairs_reach_each_one() {
    let config = Config::default();
    assert_eq!(
        jsonl("{\"a\":1}\n{\"b\":}\n", &config, false),
        Err("stdin:2:6: unexpected }".to_owned())
    );
    assert_eq!(
        jsonl("{a: 1,}\n{'b': True}\n", &config, true),
        Ok("{\"a\":1}\n{\"b\":true}\n".to_owned())
    );
}

#[test]
fn jsonl_drops_a_closing_bracket_between_records() {
    assert_eq!(
        jsonl("{\"a\":1}}\n[2]]\n", &Config::default(), true),
        Ok("{\"a\":1}\n[2]\n".to_owned())
    );
}

#[test]
fn a_json_file_with_comments_keeps_them_unless_its_dialect_was_chosen() {
    let body = b"{// note\n\"a\": 1 \"b\": 2,}";
    let detected = format(
        Path::new("settings.json"),
        body,
        &Config::default(),
        true,
        Dialect::Auto,
    )
    .unwrap();
    assert_eq!(detected.text, "{\n  // note\n  \"a\": 1,\n  \"b\": 2,\n}\n");
    assert_eq!(detected.repairs.of(Repair::MissingComma), 1);
    assert_eq!(detected.repairs.of(Repair::Comment), 0);
    let chosen = format(
        Path::new("settings.json"),
        body,
        &Config::default(),
        true,
        Dialect::Json,
    )
    .unwrap();
    assert_eq!(chosen.text, "{\n  \"a\": 1,\n  \"b\": 2\n}\n");
    assert_eq!(chosen.repairs.of(Repair::Comment), 1);
}

fn written_with_repairs(body: &str) -> String {
    format(
        Path::new("stdin"),
        body.as_bytes(),
        &Config::default(),
        true,
        Dialect::Json,
    )
    .unwrap_or_else(|message| panic!("{message}"))
    .text
}

#[test]
fn every_member_of_an_object_is_written_out_even_when_a_key_repeats() {
    assert_eq!(
        written("{\"a\":1,\"a\":2}"),
        "{\n  \"a\": 1,\n  \"a\": 2\n}\n"
    );
    assert_eq!(
        written("{\"a\":{\"x\":1},\"a\":[2],\"b\":null,\"a\":\"three\"}"),
        "{\n  \"a\": {\n    \"x\": 1\n  },\n  \"a\": [\n    2\n  ],\n  \"b\": null,\n  \"a\": \"three\"\n}\n"
    );
    assert_eq!(
        jsonl(
            "{\"a\":1,\"b\":[2],\"a\":3}\n{\"a\":4,\"a\":5}\n",
            &Config::default(),
            false
        ),
        Ok("{\"a\":1,\"b\":[2],\"a\":3}\n{\"a\":4,\"a\":5}\n".to_owned())
    );
}

#[test]
fn a_repeated_key_survives_a_repair_run_as_well() {
    let formatted = format(
        Path::new("stdin"),
        b"{a: 1, a: 2, 'a': 3,}\n",
        &Config::default(),
        true,
        Dialect::Json,
    )
    .expect("the body reads with the flag");

    assert_eq!(
        formatted.text,
        "{\n  \"a\": 1,\n  \"a\": 2,\n  \"a\": 3\n}\n"
    );
    assert_eq!(formatted.repairs.of(Repair::Key), 2);
    assert_eq!(formatted.repairs.of(Repair::Quote), 1);
    assert_eq!(formatted.repairs.of(Repair::Comma), 1);
}

#[test]
fn an_extreme_exponent_is_written_and_settles_on_what_it_wrote() {
    for literal in [
        "1e9223372036854775807",
        "1e-9223372036854775807",
        "1e309",
        "1E+400",
    ] {
        let body = format!("[{literal}]");
        let once = written(&body);
        assert_eq!(written(&once), once, "{literal} does not settle");
    }
    assert_eq!(
        written("[1e9223372036854775807,1e-9223372036854775807]"),
        "[\n  1E+9223372036854775807,\n  1E-9223372036854775807\n]\n"
    );
}

#[test]
fn a_negative_zero_keeps_its_sign_when_a_hex_number_is_repaired() {
    assert_eq!(
        written_with_repairs("[-0x0, 0x0, -0x10]"),
        "[\n  -0,\n  0,\n  -16\n]\n"
    );
}

fn auto(path: &str, body: &str, repair: bool) -> Result<String, String> {
    format(
        Path::new(path),
        body.as_bytes(),
        &Config::default(),
        repair,
        Dialect::Auto,
    )
    .map(|formatted| formatted.text)
    .map_err(|message| message.to_string())
}

#[test]
fn a_json_file_with_comments_is_jsonc_whatever_repair_says() {
    for repair in [true, false] {
        assert_eq!(
            auto("settings.json", "// c\n{\"a\": 1}\n", repair),
            Ok("// c\n{\n  \"a\": 1\n}\n".to_owned()),
            "repair {repair}"
        );
        assert_eq!(
            auto("settings.json", "/* b */ {\"a\":1}\n", repair),
            Ok("/* b */ {\n  \"a\": 1\n}\n".to_owned()),
            "repair {repair}"
        );
        assert_eq!(
            auto("settings.json", "{\"a\": 1, // t\n}\n", repair),
            Ok("{\n  \"a\": 1, // t\n}\n".to_owned()),
            "repair {repair} keeps the trailing comma JSONC allows"
        );
    }
}

#[test]
fn a_comment_marker_inside_a_string_does_not_make_a_file_jsonc() {
    assert_eq!(
        auto("settings.json", "{\"u\":\"http://x\"}\n", false),
        Ok("{\n  \"u\": \"http://x\"\n}\n".to_owned())
    );
    assert_eq!(
        auto("settings.json", "{\"u\":\"http://x\",}\n", false),
        Err("settings.json:1:17: stray comma".to_owned()),
        "with repairs off a trailing comma is still JSON's business"
    );
}
