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
