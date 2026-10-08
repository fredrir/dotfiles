#![forbid(unsafe_code)]

use dotfmt_lua::{config::Config, dialect::Dialect, format};

#[test]
fn varied_dialects_and_lexical_boundaries_reparse_and_stay_stable() {
    let cases = [
        (
            Dialect::Lua51,
            "local function f(...) local t={...}; return unpack(t) end; return f(1,nil,3)",
        ),
        (
            Dialect::Lua51,
            "local t={}; function t:f(x) return self,x end; t:f(1)",
        ),
        (
            Dialect::Lua51,
            "local x=1; repeat local y=x; x=x-1 until y==0",
        ),
        (Dialect::Lua51, "local goto=1; return goto"),
        (Dialect::Lua51, "return -2^2,2^-2,2^3^2,1 .. 'x',(1+2)*3"),
        (
            Dialect::Lua52,
            "::again:: local x=1; if x==2 then goto again end",
        ),
        (Dialect::Lua52, "return 'a\\z  \n b', '\\x41'"),
        (Dialect::Lua53, "return 10//3,1<<3,8>>1,4|2,7&3,7~3,~0"),
        (Dialect::Lua53, "return '\\u{1f600}',0x1.fp3,0x.8p2"),
        (
            Dialect::Lua54,
            "local x <const> = 1; local y <close> = nil; return x",
        ),
        (Dialect::Luajit, "return 1LL,2ULL,0xffLL,1i"),
        (Dialect::Luau, "local n: number=1; n+=2; n//=2; return n"),
        (Dialect::Luau, "type T<A...> = (A...) -> (number,string)"),
        (
            Dialect::Luau,
            "export type T = { [string]: number, x: string? }",
        ),
        (
            Dialect::Luau,
            "local x=if true then 1 elseif false then 2 else 3",
        ),
        (
            Dialect::Luau,
            "for i=1,10 do if i%2==0 then continue end print(i) end",
        ),
        (Dialect::Luau, "local s=`hello {1+2} world {`nested {3}`}`"),
        (Dialect::Luau, "local x=({} :: any) :: {number}"),
        (Dialect::Luau, "local function f<T>(x: T): T return x end"),
        (Dialect::Lua54, "-- first\rlocal x=1\rreturn x"),
        (Dialect::Lua54, "local s='a\\\r\nb'; return s"),
        (Dialect::Lua54, "local s='é😀\\255\\000'; return s"),
        (
            Dialect::Lua54,
            "local s=[==[\nhello\r\nworld\n]==]; return s",
        ),
    ];
    for (dialect, input) in cases {
        let config = Config {
            dialect,
            verify: true,
            ..Config::default()
        };
        let first = format(input, &config)
            .unwrap_or_else(|error| panic!("{dialect:?}: {input:?}: {error}"));
        assert_eq!(
            format(&first, &config).unwrap(),
            first,
            "{dialect:?}: {input:?}"
        );
    }
}

#[test]
fn extreme_width_and_combined_options_preserve_literal_contents() {
    let literal = "[==[\nhello\r\nworld\n]==]";
    let input = format!("local t={{first=1,second=2,third=3}}; local s={literal}; return t,s");
    for settings in [
        "width = 1\nindent = 16\nindent_type = tabs",
        "width = 10000\nindent = 1\ncollapse_simple_statement = always",
        "line_endings = windows\nfinal_newline = true\nquote_style = force-single",
        "call_parentheses = none\nspace_after_function_names = always",
    ] {
        let config = configured(&format!("verify = true\n{settings}"));
        let first = format(&input, &config).unwrap();
        // Lua normalizes line breaks inside long strings when loading them.
        assert!(
            first
                .replace("\r\n", "\n")
                .contains(&literal.replace("\r\n", "\n")),
            "{settings}: {first:?}"
        );
        assert_eq!(format(&first, &config).unwrap(), first, "{settings}");
    }
}

#[test]
fn utf8_bom_is_preserved() {
    assert_eq!(
        format("\u{feff}local x=1", &Config::default()).unwrap(),
        "\u{feff}local x = 1"
    );
}

#[test]
fn deeply_nested_valid_tables_format_without_stack_overflow() {
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let input = format!("return {}1{}", "{".repeat(150), "}".repeat(150));
            let first = format(&input, &Config::default()).unwrap();
            assert_eq!(format(&first, &Config::default()).unwrap(), first);
        })
        .unwrap()
        .join()
        .unwrap();
}

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
fn unsupported_luajit_constructs_return_errors_without_panicking() {
    let config = Config {
        dialect: Dialect::Luajit,
        ..Config::default()
    };
    for input in ["return 0x1p-1026", "local goto=1; return goto"] {
        assert!(format(input, &config).is_err(), "{input}");
    }
}
