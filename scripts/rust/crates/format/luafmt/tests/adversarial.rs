#![forbid(unsafe_code)]

use luafmt::{config::Config, dialect::Dialect, format};
use testkit::tree_pairs;

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
        let text = format!("luafmt {{\nverify = true\n{settings}\n}}\n");
        let root = tree_pairs(&[("luafmt.dotfile", &text)]);
        let config = Config::read(&root.path().join("luafmt.dotfile")).unwrap();
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

fn run(root: &std::path::Path, args: &[&str], input: &str) -> testkit::Ran {
    testkit::Bin::new(env!("CARGO_BIN_EXE_luafmt"))
        .args(args)
        .current_dir(root)
        .plain()
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .stdin(input)
        .run()
}

#[test]
fn utf8_bom_is_preserved_when_formatting_files_and_editor_input() {
    let root = tree_pairs(&[("bom.lua", "\u{feff}local x=1")]);
    let expected = "\u{feff}local x = 1";
    let output = run(root.path(), &["-eq"], "\u{feff}local x=1");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert_eq!(output.stdout, expected);
    let output = run(root.path(), &["bom.lua"], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert_eq!(
        std::fs::read_to_string(root.path().join("bom.lua")).unwrap(),
        expected
    );
    assert_eq!(
        run(root.path(), &["--check", "bom.lua"], "").code(),
        Some(0)
    );
}

#[test]
fn unsupported_backticks_report_an_error_and_do_not_stop_other_files() {
    let root = tree_pairs(&[("bad.lua", "local x=`hi`"), ("good.lua", "local x=1")]);
    let output = run(root.path(), &["bad.lua", "good.lua"], "");
    assert_eq!(output.code(), Some(1), "{}", output.stderr);
    assert!(output.stderr.contains("bad.lua"), "{}", output.stderr);
    assert!(!output.stderr.contains("panicked"), "{}", output.stderr);
    assert_eq!(
        std::fs::read_to_string(root.path().join("bad.lua")).unwrap(),
        "local x=`hi`"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("good.lua")).unwrap(),
        "local x = 1"
    );
    let output = run(root.path(), &["-eq"], "local x=`hi`");
    assert_eq!(output.code(), Some(1), "{}", output.stderr);
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.contains("panicked"), "{}", output.stderr);
}

#[test]
fn backticks_in_strings_comments_and_luau_are_preserved() {
    let root = tree_pairs(&[]);
    for input in [
        "local s='`hello`'; return s",
        "local s=[==[`hello`]==]; return s",
        "-- `hello`\nlocal x=1",
        "--[==[`hello`]==]\nlocal x=1",
        "-- `hello`\nreturn 7&3, ~0, 1<<2",
    ] {
        let output = run(root.path(), &["-eq"], input);
        assert_eq!(output.code(), Some(0), "{input}: {}", output.stderr);
        assert!(output.stdout.contains("`hello`"), "{}", output.stdout);
    }
    let output = run(
        root.path(),
        &["-eq", "--stdin", "typed.luau"],
        "local s=`hello {1+2}`",
    );
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    assert!(
        output.stdout.contains("`hello {1 + 2}`"),
        "{}",
        output.stdout
    );
}

#[test]
fn backend_luajit_limitations_fail_cleanly_without_overwriting_source() {
    // These are valid LuaJIT constructs the embedded parser currently rejects.
    // Until the backend supports them, rejecting them must not damage the files.
    for input in ["return 0x1p-1026", "local goto=1; return goto"] {
        let root = tree_pairs(&[("input.lua", input)]);
        let output = run(root.path(), &["--dialect", "luajit", "input.lua"], "");
        assert_eq!(output.code(), Some(1), "{input}: {}", output.stderr);
        assert!(output.stderr.contains("input.lua"), "{}", output.stderr);
        assert!(!output.stderr.contains("panicked"), "{}", output.stderr);
        assert_eq!(
            std::fs::read_to_string(root.path().join("input.lua")).unwrap(),
            input
        );
    }
}

#[test]
fn deeply_nested_valid_tables_format_in_all_modes_without_stack_overflow() {
    let input = format!("return {}1{}", "{".repeat(150), "}".repeat(150));
    let root = tree_pairs(&[("a.lua", &input), ("b.lua", &input)]);
    let streamed = run(root.path(), &["-eq"], &input);
    assert_eq!(streamed.code(), Some(0), "{}", streamed.stderr);
    let single = run(root.path(), &["-q", "a.lua"], "");
    assert_eq!(single.code(), Some(0), "{}", single.stderr);
    assert_eq!(
        std::fs::read_to_string(root.path().join("a.lua")).unwrap(),
        streamed.stdout
    );
    let output = run(root.path(), &["-q", "a.lua", "b.lua"], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
    let first = std::fs::read_to_string(root.path().join("a.lua")).unwrap();
    assert_eq!(
        first,
        std::fs::read_to_string(root.path().join("b.lua")).unwrap()
    );
    let output = run(root.path(), &["--check", "a.lua", "b.lua"], "");
    assert_eq!(output.code(), Some(0), "{}", output.stderr);
}
