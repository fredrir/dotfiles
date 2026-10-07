#![forbid(unsafe_code)]

use luafmt::{config::Config, dialect::Dialect, format};

fn configured(settings: &str) -> Config {
    let settings = settings
        .lines()
        .enumerate()
        .map(|(line, text)| {
            let (key, value) = text.split_once('=').unwrap();
            (
                key.trim().to_owned(),
                dotfmt_core::config::Setting {
                    value: value.trim().to_owned(),
                    source: "dotfmt.dotfile".into(),
                    line: line + 1,
                    global: false,
                },
            )
        })
        .collect();
    Config::from_settings(&settings).unwrap()
}

#[test]
fn formats_blocks_tables_and_expressions_idempotently() {
    let input = "local function f(x) if x then return {a=1,b=2} end end\n";
    let expected = "local function f(x)\n  if x then\n    return { a = 1, b = 2 }\n  end\nend";
    let config = Config::default();
    assert_eq!(format(input, &config).unwrap(), expected);
    assert_eq!(format(expected, &config).unwrap(), expected);
}

#[test]
fn preserves_semantics_comments_long_strings_and_ignore_directives() {
    let config = configured("verify = true");
    for input in [
        "-- café\nlocal s=[==[\n  literal \\ \' \"\n\n]==]\nreturn s",
        "local n=0x1.fp3; return n .. 'units', -2^2, (1+2)*3",
        "--[=[ long comment ]=]\nlocal t={ [true]=1, ['a-b']=2 }; return t",
        "#!/usr/bin/env lua\nprint('hello')",
        "local function f(...) return ... end; return f(1,2)",
        "-- stylua: ignore\nlocal   untouched={1,2,3}\nlocal x=1",
        "-- stylua: ignore start\nlocal   untouched={1,2,3}\n-- stylua: ignore end\nlocal x=1",
    ] {
        let output = format(input, &config).unwrap();
        assert_eq!(format(&output, &config).unwrap(), output, "{input}");
        if input.contains("untouched") {
            assert!(output.contains("local   untouched={1,2,3}"));
        }
        if input.contains("literal") {
            assert!(output.contains("[==[\n  literal \\ \' \"\n\n]==]"));
        }
    }
}

#[test]
fn supports_each_lua_dialect_and_rejects_wrong_syntax() {
    for (dialect, input) in [
        (Dialect::Lua51, "return function(...) return ... end"),
        (Dialect::Lua52, "::again:: goto again"),
        (Dialect::Lua53, "return 7 // 2, 1 << 3"),
        (Dialect::Lua54, "local x <const> = 1; return x"),
        (Dialect::Luajit, "return 1LL, 2ULL"),
        (Dialect::Luau, "local x: number = 1; x += 1; return x"),
    ] {
        let config = Config {
            dialect,
            verify: true,
            ..Config::default()
        };
        let output = format(input, &config).unwrap();
        assert_eq!(format(&output, &config).unwrap(), output);
    }
    let config = Config {
        dialect: Dialect::Lua51,
        ..Config::default()
    };
    assert!(format("local x: number = 1", &config).is_err());
    for input in [
        "local =",
        "function f(",
        "return 'unfinished",
        "if true then",
    ] {
        assert!(format(input, &Config::default()).is_err(), "{input}");
    }
}

#[test]
fn formatting_options_change_the_requested_behavior() {
    let cases = [
        ("indent = 4", "if x then f() end", "\n    f()\n"),
        ("indent_type = tabs", "if x then f() end", "\n\tf()\n"),
        ("quote_style = force-single", "local s=\"hello\"", "'hello'"),
        ("quote_style = force-double", "local s='hello'", "\"hello\""),
        ("call_parentheses = always", "f 'hello'", "f(\"hello\")"),
        ("call_parentheses = no-single-table", "f({1})", "f { 1 }"),
        (
            "collapse_simple_statement = function-only",
            "function f()\nreturn 1\nend",
            "function f() return 1 end",
        ),
        (
            "collapse_simple_statement = conditional-only",
            "if x then\nreturn 1\nend",
            "if x then return 1 end",
        ),
        (
            "space_after_function_names = definitions",
            "function f() end",
            "function f ()",
        ),
        ("space_after_function_names = calls", "f(1)", "f (1)"),
        (
            "block_newline_gaps = preserve",
            "if x then\n\nf()\n\nend",
            "then\n\n",
        ),
        ("width = 20", "local t={first=1,second=2,third=3}", "{\n"),
        (
            "line_endings = windows\nfinal_newline = true",
            "local x=1",
            "local x = 1\r\n",
        ),
    ];
    for (settings, input, expected) in cases {
        let config = configured(settings);
        let output = format(input, &config).unwrap();
        assert!(output.contains(expected), "{settings}: {output:?}");
        assert_eq!(format(&output, &config).unwrap(), output, "{settings}");
    }
}

#[test]
fn require_sorting_is_opt_in() {
    let input = "local z = require 'z'\nlocal a = require 'a'";
    let unsorted = format(input, &Config::default()).unwrap();
    assert!(unsorted.starts_with("local z"));
    let sorted = format(input, &configured("sort_requires = true\nverify = true")).unwrap();
    assert!(sorted.starts_with("local a"));
}

#[test]
fn empty_input_and_final_newline_are_stable() {
    for settings in ["", "final_newline = true", "line_endings = windows"] {
        let config = configured(settings);
        for input in ["", "\n\n", "-- comment\n", "return 1\n\n"] {
            let output = format(input, &config).unwrap();
            assert_eq!(format(&output, &config).unwrap(), output);
            if !config.final_newline {
                assert!(!output.ends_with('\n'));
            }
        }
    }
}
