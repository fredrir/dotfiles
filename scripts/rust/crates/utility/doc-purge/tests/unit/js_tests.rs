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

fn left_alone(source: &str, dialect: Dialect) -> Option<&'static str> {
    match purge::purge(source.as_bytes(), dialect) {
        Outcome::Skipped(reason) => Some(reason),
        _ => None,
    }
}

#[test]
fn javascript_comments_go() {
    assert_eq!(
        purged("const x = 1; // note\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
    assert_eq!(
        purged("// note\nconst x = 1;\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
    assert_eq!(
        purged("const x = /* note */ 1;\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
}

#[test]
fn comment_markers_inside_strings_and_regexes_survive() {
    survives("const a = \"// not a comment\";\n", Dialect::JavaScript);
    survives("const a = \"/* not a comment */\";\n", Dialect::JavaScript);
    survives("const re = /https?:\\/\\//;\n", Dialect::JavaScript);
    survives("const re = /\\/\\*.*?\\*\\//;\n", Dialect::JavaScript);
    survives("const re = /[/]\\/*/;\n", Dialect::JavaScript);
}

#[test]
fn regex_and_division_are_told_apart() {
    survives("const a = b / c / d;\n", Dialect::JavaScript);
    survives("const re = /a\\/b/gi;\n", Dialect::JavaScript);
    survives("if (ok) /a\\/b/.test(value);\n", Dialect::JavaScript);
    survives("while (ok) /a\\/b/.test(value);\n", Dialect::JavaScript);
    survives("for (;;) /a\\/b/.test(value);\n", Dialect::JavaScript);
    survives("if (ok) {} /a\\/b/.test(value);\n", Dialect::JavaScript);
    survives("const x = { a: 1 } / 2 / 3;\n", Dialect::JavaScript);
    survives("x++ / 2 / 3;\n", Dialect::JavaScript);
    assert_eq!(
        purged("const re = /a\\/b/; // note\n", Dialect::JavaScript),
        "const re = /a\\/b/;\n"
    );
}

#[test]
fn expression_keywords_allow_a_regex() {
    for prefix in [
        "const r = ",
        "return ",
        "throw ",
        "case ",
        "delete ",
        "void ",
        "typeof ",
        "new ",
        "instanceof ",
        "in ",
        "of ",
        "yield ",
        "await ",
        "else ",
        "do ",
        "default ",
    ] {
        let source = format!("{prefix}/a\\/b/; // note\n");
        assert_eq!(
            purged(&source, Dialect::JavaScript),
            format!("{prefix}/a\\/b/;\n"),
            "prefix: {prefix}"
        );
    }
}

#[test]
fn templates_nest_and_keep_their_text() {
    survives("const a = `a // b ${c} d`;\n", Dialect::JavaScript);
    survives("const a = `a ${`b ${c} d`} e`;\n", Dialect::JavaScript);
    survives("const a = `a \\${not code} b`;\n", Dialect::JavaScript);
    survives("const a = `a \\` b`;\n", Dialect::JavaScript);
    assert_eq!(
        purged("const a = `x ${/* note */ y} z`;\n", Dialect::JavaScript),
        "const a = `x ${y} z`;\n"
    );
    assert_eq!(
        purged(
            "const a = `x ${{ a: 1 /* note */ }.a} z`;\n",
            Dialect::JavaScript
        ),
        "const a = `x ${{ a: 1 }.a} z`;\n"
    );
    assert_eq!(
        purged(
            "const a = `x ${/a\\/b/.test(s)} z`; // note\n",
            Dialect::JavaScript
        ),
        "const a = `x ${/a\\/b/.test(s)} z`;\n"
    );
}

#[test]
fn jsx_text_and_attributes_keep_comment_markers() {
    survives("const a = <div>a // b /* c */</div>;\n", Dialect::Jsx);
    survives(
        "const a = <div title=\"// not a comment\" />;\n",
        Dialect::Jsx,
    );
    survives(
        "const a = <div title='/* not a comment */' />;\n",
        Dialect::Jsx,
    );
    assert_eq!(
        purged("const a = <div>{/* note */}x</div>;\n", Dialect::Jsx),
        "const a = <div>{}x</div>;\n"
    );
}

#[test]
fn jsx_elements_fragments_and_expressions_parse() {
    survives("const a = <>text</>;\n", Dialect::Jsx);
    survives("const a = <Foo.Bar />;\n", Dialect::Jsx);
    survives("const a = <svg:path xlinkHref=\"x\" />;\n", Dialect::Jsx);
    survives("const a = <my-element data-x=\"y\" />;\n", Dialect::Jsx);
    survives(
        "const a = <div {...props} disabled={true} />;\n",
        Dialect::Jsx,
    );
    survives(
        "const a = <div {...props}>{...children}</div>;\n",
        Dialect::Jsx,
    );
    survives(
        "const a = <div>{cond ? <A /> : <B />}</div>;\n",
        Dialect::Jsx,
    );
    survives("const a = <div>{() => <A />}</div>;\n", Dialect::Jsx);
    survives("const a = <div title=\"a > b\" />;\n", Dialect::Jsx);
    survives("const a = <div>{/a\\/b/.test(x)}</div>;\n", Dialect::Jsx);
}

#[test]
fn tsx_generic_arrows_and_type_arguments_parse() {
    survives("const f = <T,>(x: T) => x;\n", Dialect::Tsx);
    survives("const f = <T extends object>(x: T) => x;\n", Dialect::Tsx);
    survives("const f = <const T,>(x: T) => x;\n", Dialect::Tsx);
    survives("const f = <in T,>(x: T) => x;\n", Dialect::Tsx);
    survives("const a = <Foo<Bar> prop={x} />;\n", Dialect::Tsx);
    survives("const a = <Foo<Bar<Baz>> prop={x} />;\n", Dialect::Tsx);
    survives("const a = <Foo<(x: T) => U> prop={x} />;\n", Dialect::Tsx);
    survives("const a = <T extends=\"x\" />;\n", Dialect::Tsx);
    survives("const a = <T prop={x}>text</T>;\n", Dialect::Tsx);
    survives("const a = <Foo<Bar> prop={x} />;\n", Dialect::Jsx);
    survives("const a = <Foo<Bar<Baz>> prop={x} />;\n", Dialect::Jsx);
}

#[test]
fn type_assertions_in_ts_are_not_jsx() {
    assert_eq!(
        purged("const x = <string>value; // note\n", Dialect::TypeScript),
        "const x = <string>value;\n"
    );
}

#[test]
fn unicode_line_terminators_end_line_comments() {
    assert_eq!(
        purged("// note\u{2028}const x = 1;\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
    assert_eq!(
        purged("// note\u{2029}const x = 1;\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
}

#[test]
fn crlf_and_line_continuations_survive() {
    assert_eq!(
        purged("// note\r\nconst x = 1; // tail\r\n", Dialect::JavaScript),
        "const x = 1;\r\n"
    );
    survives("const a = \"one\\\r\ntwo\";\n", Dialect::JavaScript);
    survives("const a = \"one\\\ntwo\";\n", Dialect::JavaScript);
}

#[test]
fn hashbang_is_kept_and_skipped_lexically() {
    survives("#!/usr/bin/env node\n", Dialect::JavaScript);
    survives("#!/usr/bin/env node --x=//y\n", Dialect::JavaScript);
    assert_eq!(
        purged(
            "#!/usr/bin/env node\n// note\nconst x = 1;\n",
            Dialect::JavaScript
        ),
        "#!/usr/bin/env node\nconst x = 1;\n"
    );
    survives("\u{feff}#!/usr/bin/env node\n", Dialect::JavaScript);
}

#[test]
fn html_comments_are_purged_in_javascript() {
    assert_eq!(
        purged("var x = 1; <!-- note\nvar y = 2;\n", Dialect::JavaScript),
        "var x = 1;\nvar y = 2;\n"
    );
    assert_eq!(
        purged("<!-- note\nvar y = 2;\n", Dialect::JavaScript),
        "var y = 2;\n"
    );
    assert_eq!(
        purged("var x = 1;\n--> note\nvar y = 2;\n", Dialect::JavaScript),
        "var x = 1;\nvar y = 2;\n"
    );
    survives("var x = 1; --> 0;\n", Dialect::JavaScript);
}

#[test]
fn typescript_comments_and_directives() {
    survives(
        "/// <reference types=\"node\" />\nconst x: number = 1;\n",
        Dialect::TypeScript,
    );
    survives("// @ts-ignore\nconst x: number = y;\n", Dialect::TypeScript);
    survives(
        "// @ts-expect-error\nconst x: number = y;\n",
        Dialect::TypeScript,
    );
    survives("// @ts-nocheck\n", Dialect::TypeScript);
    survives("// @ts-check\n", Dialect::TypeScript);
    survives("/* @__PURE__ */ factory();\n", Dialect::TypeScript);
    survives("/*#__PURE__*/ factory();\n", Dialect::TypeScript);
    survives("/*! license */\n", Dialect::TypeScript);
    survives("/* @license MIT */\n", Dialect::TypeScript);
    survives("//#region name\n", Dialect::JavaScript);
    survives("//#endregion\n", Dialect::JavaScript);
}

#[test]
fn glyphs_go_from_javascript_strings_templates_and_jsx_attributes() {
    assert_eq!(
        purged(
            "const a = \"the plan \u{2014} as written\";\n",
            Dialect::JavaScript
        ),
        "const a = \"the plan as written\";\n"
    );
    assert_eq!(
        purged(
            "const a = `the plan \u{2014} as written`;\n",
            Dialect::JavaScript
        ),
        "const a = `the plan as written`;\n"
    );
    assert_eq!(
        purged(
            "const a = <div title=\"the plan \u{2014} as written\" />;\n",
            Dialect::Jsx
        ),
        "const a = <div title=\"the plan as written\" />;\n"
    );
    survives("const a = /a\u{2014}b/;\n", Dialect::JavaScript);
    survives("const a = <div>a \u{2014} b</div>;\n", Dialect::Jsx);
}

#[test]
fn inline_comments_keep_token_boundaries() {
    assert_eq!(
        purged("const x = a + /* note */ +b;\n", Dialect::JavaScript),
        "const x = a + +b;\n"
    );
    assert_eq!(
        purged("const x = a - /* note */ -b;\n", Dialect::JavaScript),
        "const x = a - -b;\n"
    );
    assert_eq!(
        purged("const x = a+/* note */+b;\n", Dialect::JavaScript),
        "const x = a+ +b;\n"
    );
    assert_eq!(
        purged("const x = a-/* note */-b;\n", Dialect::JavaScript),
        "const x = a- -b;\n"
    );
    assert_eq!(
        purged("const x = a/* note */b;\n", Dialect::JavaScript),
        "const x = a b;\n"
    );
    assert_eq!(
        purged("const x = a</* note */!--b;\n", Dialect::JavaScript),
        "const x = a< !--b;\n"
    );
}

#[test]
fn broken_javascript_is_left_alone() {
    assert_eq!(
        left_alone("const a = \"unterminated;\n", Dialect::JavaScript),
        Some("unterminated string")
    );
    assert_eq!(
        left_alone("const a = `unterminated;\n", Dialect::JavaScript),
        Some("unterminated template literal")
    );
    assert_eq!(
        left_alone("const a = /* unterminated\n", Dialect::JavaScript),
        Some("unterminated block comment")
    );
    assert_eq!(
        left_alone("const a = <div>x</div\n", Dialect::Jsx),
        Some("unterminated element")
    );
    assert_eq!(
        left_alone("const a = <div>{\"unterminated }</div>;\n", Dialect::Jsx),
        Some("unterminated string")
    );
}

#[test]
fn regex_can_start_with_an_equals_sign() {
    survives("const re = /=a/;\n", Dialect::JavaScript);
    survives("const re = /=/g;\n", Dialect::JavaScript);
    survives("const a = b /= c;\n", Dialect::JavaScript);
}

#[test]
fn multiline_block_comments_preserve_a_line_terminator() {
    assert_eq!(
        purged("function f() { return /*\n*/ 1; }\n", Dialect::JavaScript),
        "function f() { return\n1; }\n"
    );
    assert_eq!(
        purged(
            "const a = 1; /* one\r\ntwo */ const b = 2;\n",
            Dialect::JavaScript
        ),
        "const a = 1;\r\nconst b = 2;\n"
    );
    assert_eq!(
        purged(
            "const a = 1; /* one\u{2028}two */ const b = 2;\n",
            Dialect::JavaScript
        ),
        "const a = 1;\u{2028}const b = 2;\n"
    );
}

#[test]
fn modules_do_not_treat_html_like_syntax_as_comments() {
    survives("const a = b<!--c;\n", Dialect::Mjs);
    survives("const a = b<!--c;\n", Dialect::Jsx);
    survives("const a = b<!--c;\n", Dialect::Tsx);
    survives("const a = b<!--c;\n", Dialect::TypeScript);
    assert_eq!(
        purged("const a = b<!--c;\n", Dialect::JavaScript),
        "const a = b\n"
    );
}

#[test]
fn mismatched_jsx_closing_tags_are_left_alone() {
    assert_eq!(
        left_alone("const a = <div>x</span>;\n", Dialect::Jsx),
        Some("mismatched closing element")
    );
    assert_eq!(
        left_alone("const a = <>x</div>;\n", Dialect::Jsx),
        Some("mismatched closing element")
    );
}

#[test]
fn javascript_extensions_resolve_to_the_right_dialect() {
    use crate::lang::{for_path, for_token};
    use std::path::Path;

    for (name, dialect) in [
        ("a.js", Dialect::JavaScript),
        ("a.cjs", Dialect::JavaScript),
        ("a.mjs", Dialect::Mjs),
        ("a.ts", Dialect::TypeScript),
        ("a.mts", Dialect::TypeScript),
        ("a.cts", Dialect::TypeScript),
        ("a.jsx", Dialect::Jsx),
        ("a.tsx", Dialect::Tsx),
    ] {
        assert_eq!(
            for_path(Path::new(name)).map(|language| language.dialect),
            Some(dialect),
            "{name}"
        );
    }
    use crate::lang::extensions_for_token;
    assert_eq!(extensions_for_token("node"), Some(vec!["cjs", "js", "mjs"]));
    assert_eq!(extensions_for_token("jsx"), Some(vec!["jsx", "tsx"]));
    assert_eq!(
        extensions_for_token("typescript"),
        Some(vec!["cts", "mts", "ts"])
    );
    for (token, dialect) in [
        ("js", Dialect::JavaScript),
        ("cjs", Dialect::JavaScript),
        ("mjs", Dialect::Mjs),
        ("ts", Dialect::TypeScript),
        ("mts", Dialect::TypeScript),
        ("cts", Dialect::TypeScript),
        ("jsx", Dialect::Jsx),
        ("tsx", Dialect::Tsx),
        ("javascript", Dialect::JavaScript),
        ("typescript", Dialect::TypeScript),
    ] {
        assert_eq!(
            for_token(token).map(|language| language.dialect),
            Some(dialect),
            "{token}"
        );
    }
}

#[test]
fn contextual_keywords_and_private_names_do_not_confuse_regex_detection() {
    assert_eq!(
        purged("const of = value / 2; // note\n", Dialect::JavaScript),
        "const of = value / 2;\n"
    );
    survives("const x = obj.of / 2;\n", Dialect::JavaScript);
    survives("const x = obj.#return / 2;\n", Dialect::TypeScript);
    survives("for (const x of /a\\/b/) {}\n", Dialect::JavaScript);
    survives("for (const { x } of /a\\/b/) {}\n", Dialect::JavaScript);
    survives("for (const [x] of /a\\/b/) {}\n", Dialect::JavaScript);
}

#[test]
fn postfix_increment_still_divides() {
    survives("x++ / 2 / 3;\n", Dialect::JavaScript);
    survives("x-- / 2 / 3;\n", Dialect::JavaScript);
    survives("++x / 2 / 3;\n", Dialect::JavaScript);
}

#[test]
fn javascript_tool_directives_stay() {
    for source in [
        "/* @__PURE__ */ factory();\n",
        "/*#__PURE__*/ factory();\n",
        "/*#__NO_SIDE_EFFECTS__*/ factory();\n",
        "/* @__NO_SIDE_EFFECTS__ */ factory();\n",
        "/* @__INLINE__ */ function f() {}\n",
        "/* @__NOINLINE__ */ function f() {}\n",
        "/* @__KEY__ */ class C {}\n",
        "/* @__MANGLE_PROP__ */ obj.prop = 1;\n",
        "//#region name\n",
        "//#endregion\n",
        "// webpackChunkName: \"name\"\n",
        "/* webpackIgnore: true */ import(\"x\");\n",
        "/* turbopackIgnore: true */ import(\"x\");\n",
        "/* @vite-ignore */ import(\"x\");\n",
        "// @vitest-environment jsdom\n",
        "// @jest-environment jsdom\n",
        "// oxlint-disable-next-line no-var\n",
        "/* jshint ignore:line */\n",
        "/* jslint browser */\n",
        "// @noflow\n",
        "// @flow strict\n",
        "/* global foo, bar */\n",
        "/* globals foo:readonly */\n",
        "/* exported foo */\n",
        "// @generated\n",
        "/* node:coverage ignore next */\n",
        "/* node:coverage disable */\n",
        "/* node:coverage enable */\n",
        "// @cc_on\n",
        "/*! minifier keep */\n",
        "/* @license MIT */\n",
        "/* @preserve keep */\n",
        "// @license MIT\n",
        "/// <reference path=\"./x.d.ts\" />\n",
        "/// <amd-dependency path=\"x\" />\n",
    ] {
        survives(source, Dialect::TypeScript);
    }
}

#[test]
fn a_bom_does_not_hide_a_licence_or_hashbang() {
    survives(
        "\u{feff}// Copyright someone\nconst x = 1;\n",
        Dialect::JavaScript,
    );
    survives(
        "\u{feff}/*! license */\nconst x = 1;\n",
        Dialect::JavaScript,
    );
    survives(
        "\u{feff}#!/usr/bin/env node\n// Copyright someone\nconst x = 1;\n",
        Dialect::JavaScript,
    );
}

#[test]
fn a_non_leading_javascript_licence_comment_goes() {
    assert_eq!(
        purged("const x = 1;\n// Copyright someone\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
}

#[test]
fn markerless_generic_types_and_flow_arrows_are_not_jsx() {
    for (source, expected) in [
        (
            "type Handler = <T>(event: T) => void;\n// note\n",
            "type Handler = <T>(event: T) => void;\n",
        ),
        (
            "type F = { <T>(x: T): T };\n// note\n",
            "type F = { <T>(x: T): T };\n",
        ),
        (
            "interface F { <T>(x: T): T } // note\n",
            "interface F { <T>(x: T): T }\n",
        ),
        (
            "let f: <T>(x: T) => T;\n// note\n",
            "let f: <T>(x: T) => T;\n",
        ),
        (
            "const g = function <T>(this: void, x: T) { return x; }; // note\n",
            "const g = function <T>(this: void, x: T) { return x; };\n",
        ),
        (
            "const f = <T>(x: T): T => x; // note\n",
            "const f = <T>(x: T): T => x;\n",
        ),
    ] {
        assert_eq!(purged(source, Dialect::Tsx), expected, "{source}");
        assert_eq!(purged(source, Dialect::Jsx), expected, "{source}");
        assert_eq!(purged(source, Dialect::JavaScript), expected, "{source}");
    }
    assert_eq!(
        purged("const el = <div>(x) => y</div>; // note\n", Dialect::Tsx),
        "const el = <div>(x) => y</div>;\n"
    );
}

#[test]
fn jsx_text_that_looks_like_a_typed_parameter_list_stays_jsx() {
    assert_eq!(
        purged(
            "const el = <div>(x: number) => y</div>; // note\n",
            Dialect::Tsx
        ),
        "const el = <div>(x: number) => y</div>;\n"
    );
    assert_eq!(
        purged(
            "const el = <T>(x: T) => x; // note\nconst other = <T></T>;\n",
            Dialect::Tsx
        ),
        "const el = <T>(x: T) => x;\nconst other = <T></T>;\n"
    );
}

#[test]
fn multiline_jsx_text_and_semicolonless_generic_arrows_are_told_apart() {
    assert_eq!(
        purged(
            "const el = <div>\n(x: number) => y\n</div>; // note\n",
            Dialect::Tsx
        ),
        "const el = <div>\n(x: number) => y\n</div>;\n"
    );
    assert_eq!(
        purged(
            "const f = <T>(x: T) => x\nconst other = <T></T>; // note\n",
            Dialect::Tsx
        ),
        "const f = <T>(x: T) => x\nconst other = <T></T>;\n"
    );
}

#[test]
fn line_terminator_variants_remove_a_comment_only_line_cleanly() {
    assert_eq!(
        purged("// note\rconst x = 1;\r", Dialect::JavaScript),
        "const x = 1;\r"
    );
    assert_eq!(
        purged("// note\u{2028}const x = 1;\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
    assert_eq!(
        purged("// note\u{2029}const x = 1;\n", Dialect::JavaScript),
        "const x = 1;\n"
    );
    assert_eq!(
        purged("\u{feff}// note\nconst a = 1;\n", Dialect::JavaScript),
        "\u{feff}const a = 1;\n"
    );
}

#[test]
fn indentation_before_a_leading_inline_comment_survives() {
    assert_eq!(
        purged("  /* leading */ return (\n", Dialect::JavaScript),
        "  return (\n"
    );
    assert_eq!(
        purged("  /* leading */ return (\n", Dialect::Rust),
        "  return (\n"
    );
}

#[test]
fn function_and_class_bodies_keep_the_surrounding_expression_context() {
    survives("const f = function () {} / 2 / 3;\n", Dialect::JavaScript);
    survives("const C = class {} / 2 / 3;\n", Dialect::JavaScript);
    survives("const C = class D {} / 2 / 3;\n", Dialect::JavaScript);
    survives("function f() {} /a\\/\\/b/.test(x);\n", Dialect::JavaScript);
    survives("class C {} /a\\/\\/b/.test(x);\n", Dialect::JavaScript);
    assert_eq!(
        purged(
            "const f = function () {} / 2; // note\n",
            Dialect::JavaScript
        ),
        "const f = function () {} / 2;\n"
    );
    assert_eq!(
        purged("const C = class D {} / 2; // note\n", Dialect::JavaScript),
        "const C = class D {} / 2;\n"
    );
    assert_eq!(
        purged(
            "function f() {} /a\\/\\/b/.test(x); // note\n",
            Dialect::JavaScript
        ),
        "function f() {} /a\\/\\/b/.test(x);\n"
    );
    assert_eq!(
        purged(
            "class C {} /a\\/\\/b/.test(x); // note\n",
            Dialect::JavaScript
        ),
        "class C {} /a\\/\\/b/.test(x);\n"
    );
}

#[test]
fn jsx_attributes_are_not_mistaken_for_type_parameters() {
    for source in [
        "const el = <div title=\"x\">(text)</div>; // note\n",
        "const el = <T a=\"b\">(text)</T>; // note\n",
        "const el = <T extends=\"x\">(text)</T>; // note\n",
        "const el = <T {...props}>(text)</T>; // note\n",
        "const el = <T = \"x\">(text)</T>; // note\n",
        "const el = <T extends X>(text)</T>; // note\n",
    ] {
        assert_eq!(purged(source, Dialect::Tsx), source.replace(" // note", ""));
    }
}

#[test]
fn a_marked_generic_arrow_may_return_an_element_named_like_its_type_parameter() {
    assert_eq!(
        purged("const f = <T,>(x: T) => <T></T>; // note\n", Dialect::Tsx),
        "const f = <T,>(x: T) => <T></T>;\n"
    );
}

#[test]
fn keywords_used_as_property_names_do_not_leak_scanner_state() {
    assert_eq!(
        purged(
            "const o = { class: 1 }\nif (x) {}\n/[//]/.test(y); // note\n",
            Dialect::JavaScript
        ),
        "const o = { class: 1 }\nif (x) {}\n/[//]/.test(y);\n"
    );
    assert_eq!(
        purged(
            "const o = { function: 1 }\nif (x) {}\n/[//]/.test(y); // note\n",
            Dialect::JavaScript
        ),
        "const o = { function: 1 }\nif (x) {}\n/[//]/.test(y);\n"
    );
    assert_eq!(
        purged(
            "const { class, function: f } = o\nif (x) {}\n/[//]/.test(y); // note\n",
            Dialect::JavaScript
        ),
        "const { class, function: f } = o\nif (x) {}\n/[//]/.test(y);\n"
    );
}

#[test]
fn class_heritage_with_function_types_does_not_lose_the_class_body() {
    assert_eq!(
        purged(
            "class C<T extends () => void> {}\n/[//]/.test(x); // note\n",
            Dialect::TypeScript
        ),
        "class C<T extends () => void> {}\n/[//]/.test(x);\n"
    );
    assert_eq!(
        purged(
            "const C = class<T extends () => void> {} / 2; // note\n",
            Dialect::TypeScript
        ),
        "const C = class<T extends () => void> {} / 2;\n"
    );
}

#[test]
fn typed_function_expressions_keep_the_body_context() {
    assert_eq!(
        purged(
            "const f = function (): { a: string } { return { a: \"\" }; } / 2; // note\n",
            Dialect::TypeScript
        ),
        "const f = function (): { a: string } { return { a: \"\" }; } / 2;\n"
    );
    assert_eq!(
        purged(
            "function f(): { a: string } { return { a: \"\" }; }\n/[//]/.test(x); // note\n",
            Dialect::TypeScript
        ),
        "function f(): { a: string } { return { a: \"\" }; }\n/[//]/.test(x);\n"
    );
}

#[test]
fn generic_arrows_with_regex_defaults_are_recognised() {
    assert_eq!(
        purged("const f = <T,>(x = /)/) => x; // note\n", Dialect::Tsx),
        "const f = <T,>(x = /)/) => x;\n"
    );
    assert_eq!(
        purged("const f = <T,>(x = /:/) => x; // note\n", Dialect::Tsx),
        "const f = <T,>(x = /:/) => x;\n"
    );
}

#[test]
fn comments_before_function_bodies_do_not_confuse_return_types() {
    assert_eq!(
        purged(
            "function f() /* comment */ { return 1; }\n/[//]/.test(x); // note\n",
            Dialect::JavaScript
        ),
        "function f() { return 1; }\n/[//]/.test(x);\n"
    );
    assert_eq!(
        purged(
            "const f = function () /* comment */ { return 1; } / 2; // note\n",
            Dialect::JavaScript
        ),
        "const f = function () { return 1; } / 2;\n"
    );
    assert_eq!(
        purged(
            "function f(): keyof { a: string } { return \"a\"; }\n/[//]/.test(x); // note\n",
            Dialect::TypeScript
        ),
        "function f(): keyof { a: string } { return \"a\"; }\n/[//]/.test(x);\n"
    );
}

#[test]
fn jsx_attribute_quotes_are_only_folded_when_that_stays_valid() {
    survives(
        "const a = <div title=\"\u{201c}quoted\u{201d}\" />;\n",
        Dialect::Jsx,
    );
    survives("const a = <div title='it\u{2019}s' />;\n", Dialect::Jsx);
    assert_eq!(
        purged("const a = <div title=\"it\u{2019}s\" />;\n", Dialect::Jsx),
        "const a = <div title=\"it's\" />;\n"
    );
}

#[test]
fn regexes_with_comment_markers_survive_every_expression_context() {
    for source in [
        "const r = /[//]/;\n",
        "if (x) /[//]/.test(s);\n",
        "while (x) /[//]/.test(s);\n",
        "for (;;) /[//]/.test(s);\n",
        "if (x) {} /[//]/.test(s);\n",
        "const f = () => /[//]/;\n",
        "const x = cond ? /[//]/ : /x/;\n",
        "const x = [ /[//]/ ];\n",
        "const x = { a: /[//]/ };\n",
        "const x = foo(/[//]/);\n",
        "const x = y || /[//]/;\n",
        "const x = y ?? /[//]/;\n",
        "const x = y && /[//]/;\n",
        "function f() { return /[//]/; }\n",
        "function f() { throw /[//]/; }\n",
        "switch (x) { case /[//]/: break; }\n",
        "switch (x) { default: /[//]/.test(s); }\n",
        "const x = typeof /[//]/;\n",
        "const x = void /[//]/;\n",
        "const x = delete /[//]/.x;\n",
        "const x = y instanceof /[//]/;\n",
        "const x = 'a' in /[//]/;\n",
        "for (const x of /[//]/) {}\n",
        "if (x) {} else /[//]/.test(s);\n",
        "do /[//]/.test(s); while (x);\n",
        "const f = function () {} / /[//]/.test(s);\n",
        "const x = {} / /[//]/.test(s);\n",
        "const x = [] / /[//]/.test(s);\n",
    ] {
        survives(source, Dialect::JavaScript);
    }
}

#[test]
fn division_by_a_regex_literal_survives_after_values() {
    for source in [
        "const x = a / /[//]/.test(s);\n",
        "const x = f() / /[//]/.test(s);\n",
        "const x = a++ / /[//]/.test(s);\n",
        "const x = a-- / /[//]/.test(s);\n",
        "const x = a?.[0] / /[//]/.test(s);\n",
        "const x = `t` / /[//]/.test(s);\n",
        "const x = <A /> / /[//]/.test(s);\n",
        "const x = class {} / /[//]/.test(s);\n",
        "const x = function () {} / /[//]/.test(s);\n",
    ] {
        survives(source, Dialect::Jsx);
    }
}

#[test]
fn typescript_generic_type_arguments_followed_by_division_remove_comments() {
    for source in [
        "const x = value as T<U> / 2; // note\n",
        "const x = foo<T> / 2; // note\n",
        "const x = foo<Bar<Baz>> / 2; // note\n",
        "const x = foo<T>().bar / 2; // note\n",
        "const x = foo<T>.bar / 2; // note\n",
    ] {
        assert_eq!(
            purged(source, Dialect::TypeScript),
            source.replace(" // note", "")
        );
    }
    survives("const x = a < b > /[//]/.test(s);\n", Dialect::TypeScript);
}

#[test]
fn an_unclosed_comparison_does_not_swallow_later_jsx() {
    assert_eq!(
        purged(
            "const x = a<b;\nconst y = <div>{/* c */}x</div>; // note\n",
            Dialect::Tsx
        ),
        "const x = a<b;\nconst y = <div>{}x</div>;\n"
    );
}

#[test]
fn async_function_declarations_and_expressions_keep_their_context() {
    assert_eq!(
        purged(
            "async function f() {}\n/[//]/.test(x); // note\n",
            Dialect::JavaScript
        ),
        "async function f() {}\n/[//]/.test(x);\n"
    );
    assert_eq!(
        purged(
            "export async function f() {}\n/[//]/.test(x); // note\n",
            Dialect::JavaScript
        ),
        "export async function f() {}\n/[//]/.test(x);\n"
    );
    assert_eq!(
        purged(
            "const g = async function () {} / 2; // note\n",
            Dialect::JavaScript
        ),
        "const g = async function () {} / 2;\n"
    );
    assert_eq!(
        purged(
            "const h = async function () {} / /[//]/.test(x); // note\n",
            Dialect::JavaScript
        ),
        "const h = async function () {} / /[//]/.test(x);\n"
    );
    assert_eq!(
        purged(
            "const f = async () => {} / 2; // note\n",
            Dialect::JavaScript
        ),
        "const f = async () => {} / 2;\n"
    );
    assert_eq!(
        purged(
            "async function f() /* same line */ {} /[//]/.test(x); // note\n",
            Dialect::JavaScript
        ),
        "async function f() {} /[//]/.test(x);\n"
    );
}

#[test]
fn declarations_after_a_semicolonless_statement_are_still_declarations() {
    for source in [
        "const a = 1\nfunction f() {}\n/[//]/.test(a)\n",
        "const a = 1\nclass C {}\n/[//]/.test(a)\n",
        "const a = 1\nasync function f() {}\n/[//]/.test(a)\n",
        "const a = 1\nfunction f() {}\n/[\"']/.test(a)\n",
    ] {
        survives(source, Dialect::JavaScript);
    }
}

#[test]
fn a_multiline_block_comment_at_line_end_does_not_leave_a_blank_line() {
    assert_eq!(
        purged(
            "const a = 1; /* one\ntwo */\nconst b = 2;\n",
            Dialect::JavaScript
        ),
        "const a = 1;\nconst b = 2;\n"
    );
    assert_eq!(
        purged(
            "const a = 1; /* one\r\ntwo */\r\nconst b = 2;\r\n",
            Dialect::JavaScript
        ),
        "const a = 1;\r\nconst b = 2;\r\n"
    );
}

#[test]
fn flow_style_angle_casts_are_not_mistaken_for_unclosed_jsx() {
    for dialect in [Dialect::JavaScript, Dialect::Mjs, Dialect::Jsx] {
        for source in [
            "const y = <Foo>x; // note\n",
            "const y = <Foo.Bar>x; // note\n",
            "const y = <Array<string>>x; // note\n",
            "const y = <Foo>(x); // note\n",
            "const y = <Foo>{ a: 1 }; // note\n",
            "const y = <Foo>/re/.test(x); // note\n",
            "const y = <Foo><Bar>x; // note\n",
            "const y = <{| a: string |}>x; // note\n",
        ] {
            assert_eq!(
                purged(source, dialect),
                source.replace(" // note", ""),
                "{dialect:?}: {source}"
            );
        }
    }
    assert_eq!(
        purged("const el = <Foo>x</Foo>; // note\n", Dialect::Jsx),
        "const el = <Foo>x</Foo>;\n"
    );
    assert_eq!(
        purged("const el = <Foo>{x}</Foo>; // note\n", Dialect::Jsx),
        "const el = <Foo>{x}</Foo>;\n"
    );
}

#[test]
fn self_closing_jsx_is_not_treated_as_a_flow_cast() {
    assert_eq!(
        purged(
            "const el = <Foo />\nconst y = <Bar>x; // note\n",
            Dialect::Jsx
        ),
        "const el = <Foo />\nconst y = <Bar>x;\n"
    );
    assert_eq!(
        purged(
            "const el = <Foo<Bar> />\nconst y = 1; // note\n",
            Dialect::Tsx
        ),
        "const el = <Foo<Bar> />\nconst y = 1;\n"
    );
}

#[test]
fn jsx_text_with_semicolons_and_comment_markers_is_text() {
    survives("const el = <div>a; // b /* c */</div>;\n", Dialect::Jsx);
    assert_eq!(
        purged(
            "const el = <div>a; // b /* c */</div>; // note\n",
            Dialect::Jsx
        ),
        "const el = <div>a; // b /* c */</div>;\n"
    );
    assert_eq!(
        purged("const el = <div>a; b</div>; // note\n", Dialect::Jsx),
        "const el = <div>a; b</div>;\n"
    );
}

#[test]
fn typed_looking_jsx_text_with_a_semicolon_stays_jsx() {
    assert_eq!(
        purged(
            "const el = <div>(x: number) => y; // text</div>; // note\n",
            Dialect::Jsx
        ),
        "const el = <div>(x: number) => y; // text</div>;\n"
    );
}

#[test]
fn jsx_text_with_statement_words_stays_text() {
    survives("const el = <div>a; const text</div>;\n", Dialect::Jsx);
    survives("const el = <div>a; function text</div>;\n", Dialect::Jsx);
}

#[test]
fn member_named_jsx_text_with_statement_words_stays_text() {
    survives(
        "const el = <Foo.Bar>a; const text</Foo.Bar>;\n",
        Dialect::Jsx,
    );
}

#[test]
fn typed_looking_jsx_text_with_statement_words_stays_jsx() {
    survives(
        "const el = <div>(x: number) => y; const text</div>;\n",
        Dialect::Jsx,
    );
}

#[test]
fn html_close_comments_at_the_start_of_a_line_or_after_comments() {
    for source in [
        "--> note\nconst x = 1;\n",
        "/* block */ --> note\nconst x = 1;\n",
        "/*\n*/ --> note\nconst x = 1;\n",
    ] {
        assert!(
            purged(source, Dialect::JavaScript).ends_with("const x = 1;\n"),
            "{source}"
        );
    }
}
