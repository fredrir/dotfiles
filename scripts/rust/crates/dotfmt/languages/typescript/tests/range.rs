#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_typescript::dialect::Dialect;
use dotfmt_typescript::expand_range;

fn expanded(path: &str, input: &str, needle: &str) -> String {
    let start = input.find(needle).unwrap();
    let end = start + needle.len();
    let (start, end) = expand_range(Path::new(path), input, start, end, Dialect::Auto).unwrap();
    input[start..end].to_owned()
}

fn highlighted(input: &str, start: usize, end: usize) -> String {
    input[start..end].to_owned()
}

#[test]
fn expands_jsx_text_and_attribute_ranges_to_the_jsx_element() {
    let input = r#"const view = (
  <section className="hero">
    Hello <span data-kind="text">world</span>
  </section>
);
"#;
    assert_eq!(
        expanded("App.tsx", input, "world"),
        "<span data-kind=\"text\">world</span>"
    );
    assert_eq!(
        expanded("App.tsx", input, "hero"),
        "<section className=\"hero\">\n    Hello <span data-kind=\"text\">world</span>\n  </section>"
    );
    assert_eq!(
        expanded("App.tsx", input, "data-kind"),
        "<span data-kind=\"text\">world</span>"
    );

    let fragment = "const view = <><span>first</span>second</>;\n";
    assert_eq!(
        expanded("App.tsx", fragment, "second"),
        "<><span>first</span>second</>"
    );
}

#[test]
fn expands_ranges_inside_multiline_objects_and_arrays() {
    let input = r#"const theme = {
  colors: {
    primary: "red",
    secondary: "blue",
  },
  spacing: 1,
};
"#;
    assert_eq!(
        expanded("theme.ts", input, "primary"),
        "{\n    primary: \"red\",\n    secondary: \"blue\",\n  }"
    );
    assert_eq!(
        expanded("theme.ts", input, "spacing"),
        "{\n  colors: {\n    primary: \"red\",\n    secondary: \"blue\",\n  },\n  spacing: 1,\n}"
    );

    let array = "const values = [\n  1,\n  2,\n];\n";
    assert_eq!(expanded("values.ts", array, "1"), "[\n  1,\n  2,\n]");
}

#[test]
fn expands_function_body_and_return_ranges_to_statements() {
    let input = "function add(a: number, b: number) {\n  return a + b;\n}\n";
    let body_start = input.find('{').unwrap();
    let body_end = input.rfind("}\n").unwrap() + 1;
    let (start, end) = expand_range(
        Path::new("add.ts"),
        input,
        body_start,
        body_end,
        Dialect::Auto,
    )
    .unwrap();
    assert_eq!(
        highlighted(input, start, end),
        "function add(a: number, b: number) {\n  return a + b;\n}"
    );
    assert_eq!(expanded("add.ts", input, "a + b"), "return a + b;");
}

#[test]
fn expands_type_alias_and_interface_ranges_to_their_declarations() {
    let input = r#"type User = {
  name: string;
  age: number;
};
interface Props {
  value: number;
}
"#;
    assert_eq!(
        expanded("types.ts", input, "string"),
        "type User = {\n  name: string;\n  age: number;\n};"
    );
    let value = input.find("value: number").unwrap();
    let number = value + "value: ".len();
    let (start, end) = expand_range(
        Path::new("types.ts"),
        input,
        number,
        number + "number".len(),
        Dialect::Auto,
    )
    .unwrap();
    assert_eq!(
        highlighted(input, start, end),
        "interface Props {\n  value: number;\n}"
    );
}

#[test]
fn zero_width_ranges_expand_to_the_enclosing_statement_or_jsx_node() {
    let object = "const value = {\n  a: 1,\n};\n";
    let cursor = object.find("a: 1").unwrap();
    let (start, end) =
        expand_range(Path::new("value.ts"), object, cursor, cursor, Dialect::Auto).unwrap();
    assert_eq!(
        highlighted(object, start, end),
        "const value = {\n  a: 1,\n};"
    );

    let jsx = "const view = <div><span>{value}</span></div>;\n";
    let cursor = jsx.find("value").unwrap();
    let (start, end) =
        expand_range(Path::new("view.tsx"), jsx, cursor, cursor, Dialect::Auto).unwrap();
    assert_eq!(highlighted(jsx, start, end), "<span>{value}</span>");
}

#[test]
fn mid_character_ranges_are_clamped_to_char_boundaries() {
    let input = "const text = \"a😀b\";\n";
    let emoji = input.find('😀').unwrap();
    let (start, end) = expand_range(
        Path::new("text.ts"),
        input,
        emoji + 1,
        emoji + 2,
        Dialect::Auto,
    )
    .unwrap();
    assert_eq!(highlighted(input, start, end), "const text = \"a😀b\";");
}

#[test]
fn parse_errors_and_reversed_ranges_return_none() {
    assert_eq!(
        expand_range(Path::new("broken.ts"), "const x: = 1;", 0, 1, Dialect::Auto),
        None
    );
    assert_eq!(
        expand_range(Path::new("a.ts"), "const x = 1;", 5, 2, Dialect::Auto),
        None
    );
}
