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

const IDEMPOTENT_CHAIN: &str = r#"type User = { id: string; name: string; createdAt: string };
declare const users: { items: User[] };
export const D = users.items.slice(0, 5).map((user) => ({ id: user.id, title: `${user.name} joined`, at: user.createdAt }));
export const E = <List entries={users.items.slice(0, 5).map((user) => ({ id: user.id, title: `${user.name} joined`, at: user.createdAt }))} />;
"#;

#[test]
fn tsx_member_chain_repro_is_idempotent() {
    let first = formatted("idempotent-chain.tsx", IDEMPOTENT_CHAIN);
    let second = formatted("idempotent-chain.tsx", &first);
    assert_eq!(
        first, second,
        "format(format(input)) must equal format(input)"
    );
}

#[test]
fn tsx_member_chain_repro_is_stable_on_the_third_pass() {
    let first = formatted("idempotent-chain.tsx", IDEMPOTENT_CHAIN);
    let second = formatted("idempotent-chain.tsx", &first);
    let third = formatted("idempotent-chain.tsx", &second);
    assert_eq!(
        second, third,
        "the settled member-chain layout must not drift"
    );
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
fn formats_plain_css_in_js_and_preserves_interpolation() {
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

#[test]
fn formats_css_in_js_interpolations_and_restores_them_exactly() {
    let input = r#"const Button = styled.div`
  color:${color};
  background:${active ? colors.primary : colors.secondary};
  border-color:${{ tone: theme.border }.tone};
  padding:${spacing}px;
`;
const Global = createGlobalStyle`
  body{
    color:${text};
    background:${background};
    margin:0;
  }
`;
"#;
    let output = format(Path::new("a.tsx"), input, &Config::default(), Dialect::Tsx).unwrap();
    for interpolation in [
        "${color}",
        "${active ? colors.primary : colors.secondary}",
        "${{ tone: theme.border }.tone}",
        "${spacing}px",
        "${text}",
        "${background}",
    ] {
        assert!(
            output.matches(interpolation).count() == 1,
            "{interpolation}: {output}"
        );
    }
    assert!(!output.contains("DOTFMT_EXPR"), "{output}");
    assert!(
        output.contains("color: ${color};\n  background:"),
        "{output}"
    );
    assert!(
        output.contains("background: ${background};\n    margin: 0;"),
        "{output}"
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
fn interpolation_formatting_failure_keeps_the_template_unchanged() {
    let input = r#"const broken = css`
  color ${value} red;
`;
"#;
    let output = format(Path::new("a.tsx"), input, &Config::default(), Dialect::Tsx).unwrap();
    assert!(output.contains("color ${value} red;"), "{output}");
    assert!(!output.contains("DOTFMT_EXPR"), "{output}");
}

#[test]
fn non_tagged_object_properties_are_not_formatted_as_css() {
    let input = "const styles = { css: `color:red` };\n";
    let output = format(Path::new("a.tsx"), input, &Config::default(), Dialect::Tsx).unwrap();
    assert!(output.contains("`color:red`"), "{output}");
}

#[test]
fn pathological_nesting_is_a_diagnostic_not_a_stack_overflow() {
    let input = format!("const x = {}1{};\n", "(".repeat(9000), ")".repeat(9000));
    let error = format(Path::new("a.ts"), &input, &Config::default(), Dialect::Ts)
        .expect_err("nesting guard");
    assert!(error.message.contains("nesting"), "{error:?}");
}

fn tailwind_merge_config() -> Config {
    let table = dotfmt_tailwind::OrderTable::from_pairs([
        ("m-4".to_owned(), Some(1)),
        ("m-2".to_owned(), Some(2)),
        ("flex".to_owned(), Some(5)),
        ("p-4".to_owned(), Some(10)),
        ("p-2".to_owned(), Some(11)),
    ]);
    let tailwind = dotfmt_tailwind::Config {
        mode: dotfmt_tailwind::Mode::On,
        functions: vec!["cn".to_owned(), "clsx".to_owned()],
        merge: true,
        dedupe: true,
        ..dotfmt_tailwind::Config::default()
    };
    Config {
        tailwind: tailwind.clone(),
        tailwind_sorter: Some(dotfmt_tailwind::Sorter::new(
            std::sync::Arc::new(table),
            tailwind,
        )),
        ..Config::default()
    }
}

#[test]
fn tailwind_merge_formats_class_lists_and_settles() {
    let config = tailwind_merge_config();
    for (path, input, expected) in [
        (
            "merge-single.tsx",
            "export const A=()=> <div className=\"p-2 p-4\"/>\n",
            "export const A = () => <div className=\"p-4\" />;\n",
        ),
        (
            "merge-multiple.tsx",
            "export const B=()=> <div className=\"p-2 p-4 m-2 m-4 flex\"/>\n",
            "export const B = () => <div className=\"m-4 flex p-4\" />;\n",
        ),
        (
            "merge-duplicates.tsx",
            "export const C=()=> <div className=\"flex flex p-4\"/>\n",
            "export const C = () => <div className=\"flex p-4\" />;\n",
        ),
        (
            "merge-variants.tsx",
            "export const D=()=> <div className=\"md:p-2 p-2 p-4 hover:p-2\"/>\n",
            "export const D = () => <div className=\"md:p-2 hover:p-2 p-4\" />;\n",
        ),
        (
            "merge-cn.ts",
            "const x=cn(\"p-2 p-4\");\n",
            "const x = cn(\"p-4\");\n",
        ),
        (
            "merge-clsx.ts",
            "const y=clsx(\"p-2 p-4 flex flex\");\n",
            "const y = clsx(\"flex p-4\");\n",
        ),
    ] {
        let output = format(Path::new(path), input, &config, Dialect::Auto)
            .unwrap_or_else(|error| panic!("{path}: {error:?}"));
        assert_eq!(output, expected, "{path}");
        assert_eq!(
            format(Path::new(path), &output, &config, Dialect::Auto).unwrap(),
            output,
            "{path} must settle after one pass"
        );
    }
}
