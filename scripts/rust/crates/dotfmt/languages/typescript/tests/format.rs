#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_typescript::config::{Config, IndentType};
use dotfmt_typescript::dialect::Dialect;
use dotfmt_typescript::format;
use oxc_formatter::{ArrowParentheses, QuoteStyle, Semicolons, TrailingCommas};

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

fn formatted(path: &str, input: &str) -> String {
    format(Path::new(path), input, &Config::default(), Dialect::Auto).unwrap()
}

#[test]
fn formats_typescript_idempotently() {
    let input = "const x={a:1,b:[2,3]};function f(a:number):number{return a+1}";
    let expected =
        "const x = { a: 1, b: [2, 3] };\nfunction f(a: number): number {\n  return a + 1;\n}\n";
    assert_eq!(formatted("a.ts", input), expected);
    assert_eq!(formatted("a.ts", expected), expected);
}

#[test]
fn formats_tsx_jsx_decorators_satisfies_and_import_attributes() {
    for (path, input, expected_fragment) in [
        (
            "a.tsx",
            "const C=()=> <div className=\"x\">{1+2}</div>",
            "<div className=\"x\">",
        ),
        ("a.tsx", "const F=()=> <><span>hi</span></>", "<>"),
        (
            "a.jsx",
            "const C=()=> <div>{value}</div>",
            "<div>{value}</div>",
        ),
        (
            "a.ts",
            "@sealed class C{@dec m(@arg x:number){return x}}",
            "@sealed",
        ),
        (
            "a.ts",
            "const x={a:1} satisfies Record<string,number>",
            "satisfies Record<string, number>",
        ),
        (
            "a.ts",
            "import data from \"./x.json\" with { type: \"json\" };",
            "with { type: \"json\" }",
        ),
    ] {
        let output = formatted(path, input);
        assert!(output.contains(expected_fragment), "{path}: {output}");
        assert_eq!(formatted(path, &output), output, "{path}");
    }
}

#[test]
fn prettier_ignore_preserves_the_next_node() {
    let input = "// prettier-ignore\nconst   untouched={a:1};\nconst y={a:1};";
    let output = formatted("a.ts", input);
    assert!(output.contains("const   untouched={a:1};"), "{output}");
    assert!(output.contains("const y = { a: 1 };"), "{output}");
}

#[test]
fn layout_options_change_the_requested_behavior() {
    let config = configured(
        "indent = 4\nindent_type = tabs\nquote_style = single\nsemicolons = as-needed\ntrailing_commas = none\narrow_parentheses = as-needed\nbracket_spacing = false",
    );
    let output = format(
        Path::new("a.ts"),
        "const f=(x)=>{\"hi\";return {a: [1,2,3]}}",
        &config,
        Dialect::Auto,
    )
    .unwrap();
    assert!(output.contains("const f = x =>"), "{output}");
    assert!(output.contains("\t'hi'"), "{output}");
    assert!(!output.contains("'hi';"), "{output}");
    assert!(output.contains("{a: [1, 2, 3]}"), "{output}");
    assert_eq!(
        output,
        format(Path::new("a.ts"), &output, &config, Dialect::Auto).unwrap()
    );

    let config = Config {
        quote_style: QuoteStyle::Single,
        semicolons: Semicolons::AsNeeded,
        trailing_commas: TrailingCommas::None,
        arrow_parentheses: ArrowParentheses::AsNeeded,
        indent: 3,
        indent_type: IndentType::Spaces,
        ..Config::default()
    };
    let output = format(
        Path::new("a.ts"),
        "const f = (x) => ({ a: 1, b: 2 });",
        &config,
        Dialect::Auto,
    )
    .unwrap();
    assert!(output.contains("x =>"), "{output}");
}

#[test]
fn import_sorting_option_is_honored() {
    let config = configured("sort_imports = true");
    let input = "import b from \"b\";\nimport a from \"a\";\na();b();\n";
    let output = format(Path::new("a.ts"), input, &config, Dialect::Auto).unwrap();
    let a = output.find("from \"a\"").unwrap();
    let b = output.find("from \"b\"").unwrap();
    assert!(a < b, "{output}");
    assert_eq!(
        format(Path::new("a.ts"), &output, &config, Dialect::Auto).unwrap(),
        output
    );
}

#[test]
fn jsdoc_option_wraps_and_reflows_jsdoc() {
    let config = configured("jsdoc = true");
    let input = "/** a very long description that keeps going and going and should be wrapped by the formatter because the line is too long */\nfunction f() {}";
    let output = format(Path::new("a.ts"), input, &config, Dialect::Auto).unwrap();
    assert!(output.contains("\n * "), "{output}");
    assert_eq!(
        format(Path::new("a.ts"), &output, &config, Dialect::Auto).unwrap(),
        output
    );
}

#[test]
fn every_supported_dialect_formats_its_syntax() {
    for (dialect, path, input) in [
        (Dialect::Ts, "a.ts", "const x: number = 1;"),
        (Dialect::Tsx, "a.tsx", "const x = <div />;"),
        (Dialect::Js, "a.js", "const x = 1;"),
        (Dialect::Jsx, "a.jsx", "const x = <div />;"),
    ] {
        let config = Config {
            dialect,
            ..Config::default()
        };
        let output = format(Path::new(path), input, &config, Dialect::Auto).unwrap();
        assert!(!output.is_empty(), "{dialect:?}");
    }
}

#[test]
fn invalid_typescript_is_a_syntax_error() {
    assert!(
        format(
            Path::new("a.ts"),
            "const x: = 1",
            &Config::default(),
            Dialect::Auto
        )
        .is_err()
    );
}
#[test]
fn expand_trailing_commas_attribute_position_and_bracket_spacing_are_applied() {
    let config = configured("trailing_commas = none\nexpand = never\nbracket_spacing = false");
    let input = "const a = [\n  1,\n  2,\n];\nconst o = {\n  a: 1,\n};\n";
    let output = format(Path::new("a.ts"), input, &config, Dialect::Auto).unwrap();
    assert_eq!(output, "const a = [1, 2];\nconst o = {a: 1};\n");

    let config = configured("attribute_position = multiline");
    let input = "const C = () => (\n  <div a={1} b={2}>\n    x\n  </div>\n);\n";
    let output = format(Path::new("a.tsx"), input, &config, Dialect::Auto).unwrap();
    assert!(output.contains("<div\n"), "{output}");
    assert_eq!(
        format(Path::new("a.tsx"), &output, &config, Dialect::Auto).unwrap(),
        output
    );
}

#[test]
fn formats_plain_css_in_js_and_skips_interpolation() {
    let input = r#"const Button = styled.button`
  color:red;
  padding:4px;
`;
const dynamic = css`
  color: ${color};
`;
const text = "css`color:red;`";
"#;
    let output = format(Path::new("a.tsx"), input, &Config::default(), Dialect::Tsx).unwrap();
    assert!(
        output.contains("  color: red;\n  padding: 4px;"),
        "{output}"
    );
    assert!(output.contains("color: ${color};"), "{output}");
    assert!(output.contains(r#""css`color:red;`""#), "{output}");
    let object = "const v = { css: `a{color:red}` };\n";
    assert_eq!(
        format(Path::new("a.tsx"), object, &Config::default(), Dialect::Tsx).unwrap(),
        object
    );
    assert_eq!(
        format(
            Path::new("a.tsx"),
            &output,
            &Config::default(),
            Dialect::Tsx
        )
        .unwrap(),
        output
    );
}

#[test]
fn formats_styled_call_and_global_style_templates() {
    let input = r#"const Card = styled(Button)`
  display:grid;
  gap:1rem;
`;
const Global = createGlobalStyle`
  body{margin:0}
`;
"#;
    let output = format(Path::new("a.tsx"), input, &Config::default(), Dialect::Tsx).unwrap();
    assert!(output.contains("display: grid;\n  gap: 1rem;"), "{output}");
    assert!(output.contains("body {\n    margin: 0;\n  }"), "{output}");
}
