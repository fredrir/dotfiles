use crate::config::Config;
use crate::dialect::Dialect;
use crate::native;
use crate::render::Indent;

fn format(body: &str, indent: Indent, final_newline: bool) -> String {
    let config = Config {
        indent,
        final_newline,
        ..Config::default()
    };
    let formatted = native::format("test", body.as_bytes(), &config, false, Dialect::Jsonc)
        .unwrap_or_else(|error| panic!("{body:?}: {error}"));
    assert!(formatted.repairs.is_empty());
    formatted.text
}

#[test]
fn comments_and_trailing_commas_survive_with_their_values() {
    let body = "// header\n{\"a\":1, // why\n/* next */\"b\":[true,],}\n// footer";
    assert_eq!(
        format(body, Indent::Spaces(2), true),
        "// header\n{\n  \"a\": 1, // why\n  /* next */ \"b\": [\n    true,\n  ],\n}\n// footer\n"
    );
}

#[test]
fn preserves_duplicate_keys_number_precision_and_string_escapes() {
    let body = r#"{"x":1e2,/* first */"x":123456789012345678901234567890,"s":"\u0061\\\"///*"}"#;
    let out = format(body, Indent::Spaces(2), true);
    assert!(out.contains("\"x\": 1e2,"));
    assert!(out.contains("\"x\": 123456789012345678901234567890,"));
    assert!(out.contains(r#""s": "\u0061\\\"///*""#));
    assert!(out.contains("/* first */"));
}

#[test]
fn compact_output_keeps_line_comments_from_swallowing_values() {
    assert_eq!(
        format("{\"a\":1,// why\r\n\"b\":2,}", Indent::Compact, false),
        "{\"a\":1, // why\n\"b\":2,}"
    );
    assert_eq!(
        format("[1, // last\n]", Indent::Tabs, false),
        "[\n\t1, // last\n]"
    );
}

#[test]
fn comments_are_allowed_everywhere_whitespace_is_allowed() {
    for body in [
        "/* header */{}/* footer */",
        "{/* empty */}",
        "[// empty\n]",
        "{\"a\"/* key */:/* value */1/* end */,}",
        "[1// before comma\n,2]",
        "{\"a\"// key\n:// value\n1}",
        "[/* first\n  second */1,/* last */]",
        "// header\r{\"a\":1}// footer",
        "[1,/* attached */2,3]",
    ] {
        for indent in [Indent::Compact, Indent::Spaces(2), Indent::Tabs] {
            let once = format(body, indent, true);
            assert_eq!(format(&once, indent, true), once, "{body}");
        }
    }
}

#[test]
fn malformed_dialects_are_refused_instead_of_repaired() {
    for body in [
        "[,]",
        "{,}",
        "[1,,]",
        "{\"a\":1,,}",
        "[1 2]",
        "{a:1}",
        "{'a':1}",
        "/* unfinished",
        "// only a comment",
        "[1] [2]",
        "[+1]",
        "[.5]",
        "[01]",
        "[1.]",
        "[0x10]",
        "[NaN]",
        "[Infinity]",
        "[True]",
        "[1e+]",
        "{\"a\":\"unterminated}",
        "{\"a\":1,\"b\" 2}",
    ] {
        assert!(
            native::format(
                "test",
                body.as_bytes(),
                &Config::default(),
                false,
                Dialect::Hujson
            )
            .is_err(),
            "{body}"
        );
    }
}

#[test]
fn invalid_utf8_in_comments_is_refused() {
    assert!(
        native::format(
            "test",
            b"[1,/*\xff*/]",
            &Config::default(),
            false,
            Dialect::Jsonc
        )
        .is_err()
    );
}

#[test]
fn diagnostics_keep_source_locations_after_multibyte_comments() {
    let error = native::format(
        "test",
        "{/* æ */\n\"a\" 1}".as_bytes(),
        &Config::default(),
        false,
        Dialect::Jsonc,
    )
    .err()
    .unwrap();
    assert_eq!(error, "test:2:5: expected : after a key");
}

#[test]
fn empty_input_and_empty_containers_keep_existing_layout() {
    assert_eq!(format("", Indent::Spaces(2), true), "");
    assert_eq!(
        format("{\"a\":[],\"b\":{}}", Indent::Spaces(2), true),
        "{\n  \"a\": [],\n  \"b\": {}\n}\n"
    );
}

#[test]
fn inline_block_comments_after_commas_keep_the_next_member_on_a_new_line() {
    assert_eq!(
        format("[1,/* note */2]", Indent::Spaces(2), true),
        "[\n  1, /* note */\n  2\n]\n"
    );
}
