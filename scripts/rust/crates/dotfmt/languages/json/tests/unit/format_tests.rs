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
