use crate::lang::Dialect;
use crate::purge::{self, Outcome};

fn purged(source: &str, dialect: Dialect) -> String {
    match purge::purge(source.as_bytes(), dialect) {
        Outcome::Changed(edited, _) => String::from_utf8(edited.content).expect("utf-8"),
        Outcome::Untouched(_) => source.to_string(),
        Outcome::Skipped(reason) => panic!("left alone: {reason}"),
    }
}

fn survives(source: &str, dialect: Dialect) {
    assert_eq!(purged(source, dialect), source);
}

fn counts(source: &str, dialect: Dialect) -> (usize, usize) {
    match purge::purge(source.as_bytes(), dialect) {
        Outcome::Changed(edited, _) => (edited.minus, edited.plus),
        _ => (0, 0),
    }
}

fn left_alone(source: &str, dialect: Dialect) -> Option<&'static str> {
    match purge::purge(source.as_bytes(), dialect) {
        Outcome::Skipped(reason) => Some(reason),
        _ => None,
    }
}

#[test]
fn a_comment_on_its_own_line_takes_the_line_with_it() {
    assert_eq!(
        purged("// note\nlet x = 1;\n", Dialect::Rust),
        "let x = 1;\n"
    );
    assert_eq!(counts("// note\nlet x = 1;\n", Dialect::Rust), (1, 0));
}

#[test]
fn a_comment_beside_code_rewrites_the_line() {
    assert_eq!(
        purged("let x = 1; // note\n", Dialect::Rust),
        "let x = 1;\n"
    );
    assert_eq!(counts("let x = 1; // note\n", Dialect::Rust), (1, 1));
}

#[test]
fn a_slash_inside_a_string_is_not_a_comment() {
    survives("let url = \"https://example.com\";\n", Dialect::Rust);
}

#[test]
fn a_rust_raw_string_keeps_what_looks_like_a_comment() {
    survives("let s = r#\"// not a comment\"#;\n", Dialect::Rust);
    survives("let s = r##\"a \"# b\"##;\n", Dialect::Rust);
}

#[test]
fn a_lifetime_is_not_a_character_literal() {
    assert_eq!(
        purged(
            "fn f<'a>(x: &'a str) -> &'a str { x } // note\n",
            Dialect::Rust
        ),
        "fn f<'a>(x: &'a str) -> &'a str { x }\n"
    );
}

#[test]
fn rust_block_comments_nest() {
    assert_eq!(
        purged("/* a /* b */ c */let x = 1;\n", Dialect::Rust),
        "let x = 1;\n"
    );
}

#[test]
fn a_go_raw_string_keeps_what_looks_like_a_comment() {
    survives("s := `/* not a comment */`\n", Dialect::Go);
}

#[test]
fn division_and_a_regular_expression_are_told_apart() {
    assert_eq!(
        purged("const a = b / c / d; // note\n", Dialect::JavaScript),
        "const a = b / c / d;\n"
    );
    survives("const re = /https:\\/\\//;\n", Dialect::JavaScript);
    survives("const re = /[/]/;\n", Dialect::JavaScript);
}

#[test]
fn a_template_literal_keeps_its_text() {
    survives("const a = `a // b ${c} d`;\n", Dialect::JavaScript);
    assert_eq!(
        purged("const a = `x ${/* note */ y} z`;\n", Dialect::JavaScript),
        "const a = `x ${y} z`;\n"
    );
}

#[test]
fn markup_text_is_not_read_as_a_comment() {
    survives("const a = <div>a // b</div>;\n", Dialect::Jsx);
    assert_eq!(
        purged("const a = <div>{/* note */}x</div>;\n", Dialect::Jsx),
        "const a = <div>{}x</div>;\n"
    );
}

#[test]
fn a_c_line_comment_continues_over_a_backslash() {
    assert_eq!(
        purged(
            "int x = 1; // one \\\n still comment\nint y = 2;\n",
            Dialect::C
        ),
        "int x = 1;\nint y = 2;\n"
    );
}

#[test]
fn a_cpp_raw_string_is_left_whole() {
    survives("auto s = R\"tag(// not a comment)tag\";\n", Dialect::Cpp);
}

#[test]
fn a_python_hash_inside_a_string_survives() {
    survives("colour = \"#ffffff\"\n", Dialect::Python);
    survives("pattern = '# not a comment'\n", Dialect::Python);
}

#[test]
fn a_python_comment_goes() {
    assert_eq!(purged("x = 1  # note\n", Dialect::Python), "x = 1\n");
}

#[test]
fn a_shell_hash_needs_to_start_a_word() {
    survives("echo \"${#name}\" $# a#b\n", Dialect::Shell);
    assert_eq!(purged("echo hi # note\n", Dialect::Shell), "echo hi\n");
}

#[test]
fn a_shell_heredoc_keeps_its_hashes() {
    survives("cat <<'EOF'\n# not a comment\nEOF\n", Dialect::Shell);
}

#[test]
fn a_yaml_block_scalar_keeps_its_hashes() {
    survives("script: |\n  # not a comment\n  echo hi\n", Dialect::Yaml);
    assert_eq!(purged("key: value # note\n", Dialect::Yaml), "key: value\n");
}

#[test]
fn a_lua_long_comment_goes_whole() {
    assert_eq!(
        purged("--[==[ note\nstill note ]==]\nlocal x = 1\n", Dialect::Lua),
        "local x = 1\n"
    );
    survives("local s = [==[ -- not a comment ]==]\n", Dialect::Lua);
}

#[test]
fn lua_annotation_descriptions_go() {
    for annotation in [
        "---@param go_to string",
        "---@param target? string?",
        "---@param ... string",
        "---@field name string",
        "---@field private name string",
        "---@field [string] boolean",
        "---@type string, number",
        "---@alias Name string",
        "---@alias (exact) List<T> T[]",
        "---@class (exact) Child<T>: Parent<T>, Other",
        "---@vararg string",
        "---@overload fun(value: string): boolean",
    ] {
        for description in [
            " some comment i made",
            " # some comment i made",
            "#some comment i made",
            "\tbeskrivelse med æøå",
        ] {
            assert_eq!(
                purged(&format!("{annotation}{description}\n"), Dialect::Lua),
                format!("{annotation}\n"),
            );
        }
        survives(&format!("{annotation}\n"), Dialect::Lua);
    }
}

#[test]
fn lua_annotation_descriptions_leave_complex_types_whole() {
    for ty in [
        "string | number | nil",
        "table<string, table<string, number>>",
        "(string | number)[]?",
        "{ name: string, callback: fun(value: number): boolean }",
        "[string, number]",
        "[[some text]]",
        "[=[a]] more text]=]",
        "table<string, [=[a]] more text]=]>",
        r#"'some text' | "other text" | 'it\'s quoted'"#,
        "`T`",
        "-1 | 0 | 1",
        "fun(value: string, ...: number): boolean, string?",
        "fun<T>(value: T): result: T, error: string?",
        "async fun(): (boolean, string?)",
        "fun(): fun(value: string): number",
        "fun(): nil | fun(): nil",
    ] {
        let annotation = format!("---@param value {ty}\n");
        assert_eq!(
            purged(
                &format!("---@param value {ty} some comment i made\n"),
                Dialect::Lua,
            ),
            annotation,
        );
        survives(&annotation, Dialect::Lua);
    }
}

#[test]
fn lua_return_descriptions_leave_result_names_whole() {
    for (source, expected) in [
        ("boolean # whether it worked", "boolean"),
        ("boolean success whether it worked", "boolean success"),
        (
            "string name, number? count # the results",
            "string name, number? count",
        ),
        ("string, number # the results", "string, number"),
    ] {
        assert_eq!(
            purged(&format!("---@return {source}\n"), Dialect::Lua),
            format!("---@return {expected}\n"),
        );
        survives(&format!("---@return {expected}\n"), Dialect::Lua);
    }
}

#[test]
fn lua_annotation_description_cuts_preserve_layout_and_count_as_comments() {
    let source = "  ---@param go_to string some comment i made\r\nlocal value ---@type string # a value\r\n---@param name string description";
    let expected =
        "  ---@param go_to string\r\nlocal value ---@type string\r\n---@param name string";
    assert_eq!(purged(source, Dialect::Lua), expected);
    survives(expected, Dialect::Lua);
    match purge::purge(source.as_bytes(), Dialect::Lua) {
        Outcome::Changed(edited, saved) => {
            assert_eq!((edited.minus, edited.plus, edited.comments), (3, 3, 3));
            assert_eq!(saved.directives, 3);
        }
        _ => panic!("expected annotation descriptions to be purged"),
    }
}

#[test]
fn unsupported_or_incomplete_lua_annotations_stay() {
    for source in [
        "---@diagnostic disable-next-line: undefined-global\n",
        "---@generic T: string, U\n",
        "---@custom name type more arguments\n",
        "---@param value\n",
        "---@param value string   \n",
        "---@param value table<string, number description\n",
        "---@param value (string | number] description\n",
        "---@param value string |\n",
        "---@param value fun():\n",
        "---@param value 'unterminated description\n",
        "---@param value [=[unterminated description\n",
        "--[[-@param value string block contents\nmore contents]]\n",
        "local text = '---@param value string some description'\n",
    ] {
        survives(source, Dialect::Lua);
    }
    survives("-- -@param value string description\n", Dialect::Sql);
}

#[test]
fn a_haskell_arrow_is_not_a_comment() {
    survives("a --> b\n", Dialect::Haskell);
    assert_eq!(purged("a = 1 -- note\n", Dialect::Haskell), "a = 1\n");
}

#[test]
fn a_sql_dollar_quote_keeps_its_dashes() {
    survives(
        "CREATE FUNCTION f() AS $$ -- not a comment $$;\n",
        Dialect::Sql,
    );
    assert_eq!(purged("SELECT 1; -- note\n", Dialect::Sql), "SELECT 1;\n");
}

#[test]
fn a_shebang_stays() {
    survives("#!/usr/bin/env bash\n", Dialect::Shell);
    assert_eq!(
        purged("#!/usr/bin/env bash\n# note\necho hi\n", Dialect::Shell),
        "#!/usr/bin/env bash\necho hi\n"
    );
}

#[test]
fn tool_directives_stay() {
    survives("x = untyped()  # type: ignore\n", Dialect::Python);
    survives("const a = b;  // @ts-ignore\n", Dialect::JavaScript);
    survives("//go:build linux\n", Dialect::Go);
    survives("# shellcheck disable=SC2086\n", Dialect::Shell);
    survives("---@param name string\n", Dialect::Lua);
}

#[test]
fn a_leading_licence_header_stays() {
    survives(
        "// SPDX-License-Identifier: MIT\nlet x = 1;\n",
        Dialect::Rust,
    );
    assert_eq!(
        purged("let x = 1;\n// Copyright someone\n", Dialect::Rust),
        "let x = 1;\n"
    );
}

#[test]
fn glyphs_go_from_string_literals() {
    assert_eq!(
        purged("let a = \"the plan \u{2014} as written\";\n", Dialect::Rust),
        "let a = \"the plan as written\";\n"
    );
    assert_eq!(
        purged(
            "let a = \"one \u{2014} two \u{2014} three\";\n",
            Dialect::Rust
        ),
        "let a = \"one two three\";\n"
    );
    assert_eq!(
        purged("let a = \"\u{2014} leading\";\n", Dialect::Rust),
        "let a = \"leading\";\n"
    );
    assert_eq!(
        purged("let a = \"trailing \u{2014}\";\n", Dialect::Rust),
        "let a = \"trailing\";\n"
    );
}

#[test]
fn indentation_in_front_of_a_glyph_survives() {
    assert_eq!(
        purged("let a = \"  \u{2014} and 4 more\";\n", Dialect::Rust),
        "let a = \"  and 4 more\";\n"
    );
}

#[test]
fn the_separator_dot_and_the_ellipsis_are_left_alone() {
    survives("let a = \"a\u{b7}b\";\n", Dialect::Rust);
    survives("let a = \"\u{2026}/report.txt\";\n", Dialect::Rust);
    survives("let a = \"move \u{b7} quit\";\n", Dialect::Rust);
}

#[test]
fn curly_quotes_fold_and_a_clash_is_escaped() {
    assert_eq!(
        purged("let a = \"\u{201c}quoted\u{201d}\";\n", Dialect::Rust),
        "let a = \"\\\"quoted\\\"\";\n"
    );
    assert_eq!(
        purged("let a = \"it\u{2019}s\";\n", Dialect::Rust),
        "let a = \"it's\";\n"
    );
}

#[test]
fn a_glyph_stays_where_folding_it_would_break_the_literal() {
    survives("echo 'it\u{2019}s'\n", Dialect::Shell);
}

#[test]
fn glyphs_outside_a_string_are_left_alone() {
    survives("const a = <div>a \u{2014} b</div>;\n", Dialect::Jsx);
    survives("const re = /a\u{2014}b/;\n", Dialect::JavaScript);
}

#[test]
fn a_comment_between_code_leaves_one_space() {
    assert_eq!(
        purged("let x = a /* note */ + b;\n", Dialect::Rust),
        "let x = a + b;\n"
    );
    assert_eq!(purged("foo(/* note */ y);\n", Dialect::Rust), "foo(y);\n");
}

#[test]
fn a_python_doc_string_goes_when_the_body_survives() {
    assert_eq!(
        purged(
            "def f():\n    \"\"\"Doc.\"\"\"\n    return 1\n",
            Dialect::Python
        ),
        "def f():\n    return 1\n"
    );
}

#[test]
fn a_sole_doc_string_keeps_its_first_sentence() {
    assert_eq!(
        purged(
            "def f():\n    \"\"\"Do the thing. More detail here.\"\"\"\n",
            Dialect::Python
        ),
        "def f():\n    \"\"\"Do the thing.\"\"\"\n"
    );
}

#[test]
fn a_module_doc_string_goes_when_the_module_has_code() {
    assert_eq!(
        purged(
            "\"\"\"Module doc.\n\nMore.\n\"\"\"\nimport os\n",
            Dialect::Python
        ),
        "import os\n"
    );
}

#[test]
fn a_string_that_is_not_a_doc_string_is_left_alone() {
    survives("x = 1\n\"just a string\"\n", Dialect::Python);
}

#[test]
fn a_file_the_scanner_cannot_finish_is_left_alone() {
    assert_eq!(
        left_alone("let a = \"unterminated;\n", Dialect::Rust),
        Some("unterminated string")
    );
}

#[test]
fn a_comment_between_blank_lines_does_not_leave_a_hole() {
    assert_eq!(
        purged("let a = 1;\n\n// note\n\nlet b = 2;\n", Dialect::Rust),
        "let a = 1;\n\nlet b = 2;\n"
    );
}

#[test]
fn css_comments_go() {
    assert_eq!(
        purged("/* note */\n.a { color: red; }\n", Dialect::Css),
        ".a { color: red; }\n"
    );
    assert_eq!(
        counts("/* note */\n.a { color: red; }\n", Dialect::Css),
        (1, 0)
    );
    assert_eq!(
        purged("a { color: /* note */ red; }\n", Dialect::Css),
        "a { color: red; }\n"
    );
    survives(".a { color: red; }\n", Dialect::Css);
}

#[test]
fn plain_css_has_no_line_comments() {
    survives(
        "a { background: url(//cdn.example/x.png); }\n",
        Dialect::Css,
    );
    survives("a { --x: //not-a-comment; }\n", Dialect::Css);
    survives("a { color: red; // not a comment\n}\n", Dialect::Css);
}

#[test]
fn css_strings_keep_comment_markers() {
    survives("a { content: \"/* not a comment */\"; }\n", Dialect::Css);
    survives("a { content: '*/'; }\n", Dialect::Css);
    survives("a { content: \"a\\\\b\"; }\n", Dialect::Css);
    survives(
        "a { content: \"one\\\ntwo // not a comment\"; }\n",
        Dialect::Css,
    );
    assert_eq!(
        left_alone("a { content: \"one\ntwo\"; }\n", Dialect::Css),
        Some("unterminated string")
    );
}

#[test]
fn css_urls_keep_comment_markers() {
    survives(
        "a { background: url(//cdn.example/x.png); }\n",
        Dialect::Css,
    );
    survives(
        "a { background: url(/*not-a-comment*/bg.png); }\n",
        Dialect::Css,
    );
    survives("a { background: url(/**/\"bg.jpg\"); }\n", Dialect::Css);
    survives(
        "a { background: url(\"https://example.com/a.png\"); }\n",
        Dialect::Css,
    );
    survives("a { background: url(foo\\)bar.png); }\n", Dialect::Css);
    assert_eq!(
        purged("a { background: url(\"x\u{2014}y.png\"); }\n", Dialect::Css),
        "a { background: url(\"xy.png\"); }\n"
    );
}

#[test]
fn css_inline_comments_keep_token_boundaries() {
    assert_eq!(
        purged(
            "@media screen/* note */and (min-width: 1px) {}\n",
            Dialect::Css
        ),
        "@media screen and (min-width: 1px) {}\n"
    );
    assert_eq!(
        purged(".a/* note */.b { color: red; }\n", Dialect::Css),
        ".a.b { color: red; }\n"
    );
    assert_eq!(
        purged(".a /* note */ .b { color: red; }\n", Dialect::Css),
        ".a .b { color: red; }\n"
    );
    assert_eq!(
        purged("a { --x: 1/* note */2; }\n", Dialect::Css),
        "a { --x: 1 2; }\n"
    );
    assert_eq!(
        purged("a { color: red/* note */; }\n", Dialect::Css),
        "a { color: red; }\n"
    );
    survives("@media screen and (min-width: 1px) {}\n", Dialect::Css);
}

#[test]
fn css_comments_do_not_nest() {
    assert_eq!(
        purged("/* one /* two */ .a { color: red; }\n", Dialect::Css),
        ".a { color: red; }\n"
    );
}

#[test]
fn css_glyphs_go_from_strings() {
    assert_eq!(
        purged(
            "a { content: \"the plan \u{2014} as written\"; }\n",
            Dialect::Css
        ),
        "a { content: \"the plan as written\"; }\n"
    );
    assert_eq!(
        purged("a { content: \"\u{201c}quoted\u{201d}\"; }\n", Dialect::Css),
        "a { content: \"\\\"quoted\\\"\"; }\n"
    );
    survives("a { content: \"a\u{b7}b\u{2026}\"; }\n", Dialect::Css);
    survives("a { content: \"\\\u{2014}\"; }\n", Dialect::Css);
    assert_eq!(
        purged("a { content: \"\\\\\u{2014}\"; }\n", Dialect::Css),
        "a { content: \"\\\\\"; }\n"
    );
}

#[test]
fn css_important_and_directive_comments_stay() {
    survives("/*! keep */\n.a { color: red; }\n", Dialect::Css);
    survives("/* @license MIT */\n.a { color: red; }\n", Dialect::Css);
    survives("/* @preserve keep */\n.a { color: red; }\n", Dialect::Css);
    survives(
        ".a { color: red; }\n/* stylelint-disable */\n.b { color: blue; }\n",
        Dialect::Css,
    );
    survives(".a { color: red; /* autoprefixer: off */ }\n", Dialect::Css);
    survives(
        ".a { color: red; }\n/* purgecss start ignore */\n.b { color: blue; }\n/* purgecss end ignore */\n",
        Dialect::Css,
    );
    survives(
        ".a { color: red; }\n/*# sourceMappingURL=a.css.map */\n",
        Dialect::Css,
    );
    survives(".a { color: red; }\n/*# sourceURL=a.css */\n", Dialect::Css);
    match purge::purge(
        b"/*! keep */\n/* gone */\n.a { color: red; }\n",
        Dialect::Css,
    ) {
        Outcome::Changed(_, saved) => assert_eq!(saved.important, 1),
        _ => panic!("expected the plain comment to go and the important one to stay"),
    }
}

#[test]
fn css_modules_and_tailwind_constructs_stay() {
    let source = r#":global(.foo) { color: red; }
.bar {
  composes: baz from "./x.module.css";
}
:export { primary: #fff; }

@theme {
  --color-*: initial;
  --color-mint-500: oklch(0.72 0.11 178);
  --spacing: 0.25rem;
}
@custom-variant dark (&:where(.dark, .dark *));
@utility tab-4 { tab-size: 4; }
@source inline("underline");
@reference "../app.css";
@tailwind base;
@tailwind components;
@tailwind utilities;
.button { @apply text-sm font-medium; }
"#;
    survives(source, Dialect::Css);
    let commented = source.replace(".button", "/* note */\n.button");
    assert_eq!(purged(&commented, Dialect::Css), source);
}

#[test]
fn scss_comments_go() {
    assert_eq!(
        purged("// note\n.a { color: red; }\n", Dialect::Scss),
        ".a { color: red; }\n"
    );
    assert_eq!(
        purged(".a { color: red; // note\n}\n", Dialect::Scss),
        ".a { color: red;\n}\n"
    );
    assert_eq!(
        purged("/* note */ .a { color: red; }\n", Dialect::Scss),
        ".a { color: red; }\n"
    );
    assert_eq!(counts("// note\n.a {}\n", Dialect::Scss), (1, 0));
}

#[test]
fn scss_custom_properties_keep_double_slashes() {
    survives(".a { --x: //literal; }\n", Dialect::Scss);
    survives(".a { --x: https://example.com; }\n", Dialect::Scss);
    assert_eq!(
        purged(".a { --x: /* note */ red; }\n", Dialect::Scss),
        ".a { --x: red; }\n"
    );
    assert_eq!(
        purged(".a { --x: //literal; // gone\n}\n", Dialect::Scss),
        ".a { --x: //literal;\n}\n"
    );
}

#[test]
fn scss_urls_and_interpolation_keep_literal_comment_markers() {
    survives(
        ".a { background: url(http://example.com/x.png); }\n",
        Dialect::Scss,
    );
    survives(
        ".a { background: url(//cdn.example/x.png); }\n",
        Dialect::Scss,
    );
    survives(".a { background: url(/*x*/pic.png); }\n", Dialect::Scss);
    survives(".a { background: url(#{$path}/x.png); }\n", Dialect::Scss);
    survives(".a { background: url(#{fn($x)}/x.png); }\n", Dialect::Scss);
    survives(".a { background: url($p + \"//x\"); }\n", Dialect::Scss);
    survives(
        ".a { content: \"#{map-get((a: \"x\"), a)}\"; }\n",
        Dialect::Scss,
    );
    survives(
        ".a { content: \"#{if(true, \"//\", \"x\")}\"; }\n",
        Dialect::Scss,
    );
    assert_eq!(
        purged(".a { background: url(/*c*/\"x\"); }\n", Dialect::Scss),
        ".a { background: url(\"x\"); }\n"
    );
    assert_eq!(
        purged(
            ".a { background: url($p /* gone */ + \"/x\"); }\n",
            Dialect::Scss
        ),
        ".a { background: url($p + \"/x\"); }\n"
    );
}

#[test]
fn scss_important_and_lint_comments_stay() {
    survives("/*! keep */\n.a { color: red; }\n", Dialect::Scss);
    survives("// stylelint-disable\n.a { color: red; }\n", Dialect::Scss);
    survives(
        "/* stylelint-disable-next-line */\n.a { color: red; }\n",
        Dialect::Scss,
    );
}

#[test]
fn sass_silent_comments_take_their_indented_block() {
    assert_eq!(
        purged("// note\n  still note\n.a\n  color: red\n", Dialect::Sass),
        ".a\n  color: red\n"
    );
    assert_eq!(
        purged("// note\n.a\n  color: red\n", Dialect::Sass),
        ".a\n  color: red\n"
    );
    assert_eq!(
        purged(".a\n  // note\n  color: red\n", Dialect::Sass),
        ".a\n  color: red\n"
    );
    assert_eq!(
        purged(".a\n  color: red // note\n", Dialect::Sass),
        ".a\n  color: red\n"
    );
}

#[test]
fn sass_loud_comments_take_their_indented_block() {
    assert_eq!(
        purged("/* note\n   still note\n.a\n  color: red\n", Dialect::Sass),
        ".a\n  color: red\n"
    );
    assert_eq!(
        purged(".a\n  /* note */\n  color: red\n", Dialect::Sass),
        ".a\n  color: red\n"
    );
    assert_eq!(
        purged(
            ".a\n  /* note\n     still note\n  color: red\n",
            Dialect::Sass
        ),
        ".a\n  color: red\n"
    );
    assert_eq!(
        purged(
            ".a\n  /* note\n     still note\n  */\n  color: red\n",
            Dialect::Sass
        ),
        ".a\n  color: red\n"
    );
}

#[test]
fn sass_custom_properties_keep_double_slashes() {
    survives(".a\n  --x: //literal\n  color: var(--x)\n", Dialect::Sass);
}

#[test]
fn sass_urls_and_interpolation_keep_literal_comment_markers() {
    survives(
        ".a\n  background: url(http://example.com/x.png)\n",
        Dialect::Sass,
    );
    survives(".a\n  background: url(/*x*/pic.png)\n", Dialect::Sass);
    survives(".a\n  background: url(#{$path}/x.png)\n", Dialect::Sass);
    survives(
        ".a\n  content: \"#{map-get((a: \"x\"), a)}\"\n",
        Dialect::Sass,
    );
}

#[test]
fn sass_important_comment_stays() {
    survives("/*! keep\n  still keep\n.a\n  color: red\n", Dialect::Sass);
}

#[test]
fn less_comments_go_and_urls_stay() {
    assert_eq!(
        purged("// note\n.a { color: red; }\n", Dialect::Less),
        ".a { color: red; }\n"
    );
    assert_eq!(
        purged(".a { color: red; // note\n}\n", Dialect::Less),
        ".a { color: red;\n}\n"
    );
    survives(
        ".a { background: url(http://example.com/x.png); }\n",
        Dialect::Less,
    );
    survives(
        ".a { background: url(//cdn.example/x.png); }\n",
        Dialect::Less,
    );
    survives(".a { width: ~`x // y /* z */`; }\n", Dialect::Less);
    survives(
        "@s: ~\"http://example/x\";\n.a { background: url(@s); }\n",
        Dialect::Less,
    );
    survives("/*! keep */\n.a { color: red; }\n", Dialect::Less);
}

#[test]
fn stylus_comments_go_and_literals_stay() {
    assert_eq!(
        purged("// note\nbody\n  color red\n", Dialect::Stylus),
        "body\n  color red\n"
    );
    assert_eq!(
        purged("body\n  color red // note\n", Dialect::Stylus),
        "body\n  color red\n"
    );
    assert_eq!(
        purged("/* note */\nbody\n  color red\n", Dialect::Stylus),
        "body\n  color red\n"
    );
    survives(
        "body\n  background url(//cdn.example/x.png)\n",
        Dialect::Stylus,
    );
    survives(
        "@css {\n  .a { x: //raw; }\n}\nbody\n  color red\n",
        Dialect::Stylus,
    );
    survives(
        "@css {\n  .a { background: url(/*literal*/bg.png); }\n}\nbody\n  color red\n",
        Dialect::Stylus,
    );
    assert_eq!(
        purged(
            "@css {\n  .a { color: red; /* note */ }\n}\nbody\n  color red\n",
            Dialect::Stylus
        ),
        "@css {\n  .a { color: red; }\n}\nbody\n  color red\n"
    );
    survives("/*! keep */\nbody\n  color red\n", Dialect::Stylus);
}

#[test]
fn sugarss_comments_go_and_urls_stay() {
    assert_eq!(
        purged("// note\n.a\n  color: red\n", Dialect::Sugarss),
        ".a\n  color: red\n"
    );
    assert_eq!(
        purged(".a\n  color: red // note\n", Dialect::Sugarss),
        ".a\n  color: red\n"
    );
    survives(
        ".a\n  background: url(//cdn.example/x.png)\n",
        Dialect::Sugarss,
    );
}

#[test]
fn css_family_types_and_variations_resolve() {
    use crate::lang::{for_path, for_token};
    use std::path::Path;

    for (token, dialect) in [
        ("css", Dialect::Css),
        ("pcss", Dialect::Css),
        ("postcss", Dialect::Css),
        ("scss", Dialect::Scss),
        ("sass", Dialect::Sass),
        ("less", Dialect::Less),
        ("styl", Dialect::Stylus),
        ("sss", Dialect::Sugarss),
    ] {
        assert_eq!(
            for_token(token).map(|language| language.dialect),
            Some(dialect)
        );
    }
    for token in [
        "module.css",
        "style.module.scss",
        "tailwindcss",
        "css-modules",
    ] {
        assert!(for_token(token).is_some(), "unresolved type: {token}");
    }
    assert_eq!(
        for_path(Path::new("button.module.css")).map(|language| language.dialect),
        Some(Dialect::Css)
    );
    assert_eq!(
        for_path(Path::new("button.module.scss")).map(|language| language.dialect),
        Some(Dialect::Scss)
    );
}

#[test]
fn css_an_unterminated_comment_skips_the_file() {
    for dialect in [
        Dialect::Css,
        Dialect::Scss,
        Dialect::Sass,
        Dialect::Less,
        Dialect::Sugarss,
    ] {
        assert_eq!(
            left_alone("a { /* note\n", dialect),
            Some("unterminated block comment"),
            "{dialect:?}"
        );
    }
}

#[test]
fn stylus_an_unterminated_comment_runs_to_eof() {
    assert_eq!(
        purged("a\n  color red\n/*\nb\n  color blue\n", Dialect::Stylus),
        "a\n  color red\n"
    );
}

#[test]
fn css_crlf_lines_survive_a_purge() {
    assert_eq!(
        purged("// note\r\n.a { color: red; }\r\n", Dialect::Scss),
        ".a { color: red; }\r\n"
    );
    assert_eq!(
        purged(".a\r\n  // note\r\n  color: red\r\n", Dialect::Sass),
        ".a\r\n  color: red\r\n"
    );
    assert_eq!(
        purged("/* note */\r\n.a { color: red; }\r\n", Dialect::Css),
        ".a { color: red; }\r\n"
    );
}

#[test]
fn scss_custom_property_braces_keep_their_double_slashes() {
    survives(".a { --x: { a: //literal; b: c }; }\n", Dialect::Scss);
    assert_eq!(
        purged(
            ".a { --x: { a: b /* note */ }; --y: red; }\n",
            Dialect::Scss
        ),
        ".a { --x: { a: b }; --y: red; }\n"
    );
}

#[test]
fn css_important_after_other_comments_stays() {
    assert_eq!(
        purged(
            "/* first */\n/*! keep */\n.a { color: red; }\n",
            Dialect::Css
        ),
        "/*! keep */\n.a { color: red; }\n"
    );
}

#[test]
fn css_license_after_charset_stays() {
    survives(
        "@charset \"UTF-8\";\n/* Copyright someone */\n.a { color: red; }\n",
        Dialect::Css,
    );
}

#[test]
fn css_tool_directives_include_rtl_and_noinspection() {
    survives(
        ".a { color: red; }\n/*rtl:ignore*/\n.b { color: blue; }\n/* noinspection CssInvalidPropertyValue */\n.c { color: green; }\n",
        Dialect::Css,
    );
}
