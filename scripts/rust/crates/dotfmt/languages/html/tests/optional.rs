#![forbid(unsafe_code)]

use std::path::Path;

use dotfmt_core::diagnostic::DiagnosticKind;
use dotfmt_html::{Config, Dialect, format};

/// Format `input`, assert the result settles after one pass, and return the
/// nesting with layout whitespace removed so assertions read as element
/// structure.
fn nesting(input: &str) -> String {
    let path = Path::new("optional.html");
    let output = format(path, input, &Config::default(), Dialect::Html)
        .unwrap_or_else(|error| panic!("{input:?} must format: {error}"));
    assert_eq!(
        format(path, &output, &Config::default(), Dialect::Html).unwrap(),
        output,
        "formatting must settle after one pass for {input:?}"
    );
    collapse(&output)
}

fn collapse(html: &str) -> String {
    html.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("> <", "><")
}

fn syntax_error(input: &str) -> dotfmt_core::diagnostic::Diagnostic {
    format(
        Path::new("broken.html"),
        input,
        &Config::default(),
        Dialect::Html,
    )
    .unwrap_err()
}

#[test]
fn repairs_list_table_paragraph_and_head_body_omitted_end_tags() {
    // Reported regressions: valid HTML5 that must not be a false syntax error.
    assert_eq!(
        nesting("<body><ul><li>a<li>b</ul></body>"),
        "<body><ul><li>a</li><li>b</li></ul></body>"
    );
    assert_eq!(
        nesting("<body><table><tr><td>a<td>b</table></body>"),
        "<body><table><tr><td>a</td><td>b</td></tr></table></body>"
    );
    assert_eq!(
        nesting("<body><p>one<div>two</div></body>"),
        "<body><p>one</p><div>two</div></body>"
    );
    // `</head>` must be inserted before `<body>`, never after it.
    assert_eq!(
        nesting("<html><head><title>x</title><body><p>y</body></html>"),
        "<html><head><title>x</title></head><body><p>y</p></body></html>"
    );
}

#[test]
fn repairs_omitted_end_tags_across_content_models() {
    let cases = [
        ("<ul><li>a<li>b</ul>", "<ul><li>a</li><li>b</li></ul>"),
        ("<ol><li>a<li>b</ol>", "<ol><li>a</li><li>b</li></ol>"),
        (
            "<dl><dt>a<dd>b<dt>c</dl>",
            "<dl><dt>a</dt><dd>b</dd><dt>c</dt></dl>",
        ),
        (
            "<table><tr><td>a<td>b</table>",
            "<table><tr><td>a</td><td>b</td></tr></table>",
        ),
        (
            "<table><thead><tr><th>h<tbody><tr><td>a</table>",
            "<table><thead><tr><th>h</th></tr></thead><tbody><tr><td>a</td></tr></tbody></table>",
        ),
        (
            "<table><tbody><tr><td>a<td>b<tr><td>c</table>",
            "<table><tbody><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></tbody></table>",
        ),
        (
            "<table><tr><td><p>a<td>b</table>",
            "<table><tr><td><p>a</p></td><td>b</td></tr></table>",
        ),
        (
            "<table><colgroup><col><tbody><tr><td>x</table>",
            "<table><colgroup><col></colgroup><tbody><tr><td>x</td></tr></tbody></table>",
        ),
        (
            "<select><option>a<option>b</select>",
            "<select><option>a</option><option>b</option></select>",
        ),
        (
            "<select><optgroup><option>a<optgroup><option>b</select>",
            "<select><optgroup><option>a</option></optgroup><optgroup><option>b</option></optgroup></select>",
        ),
        ("<p>one<p>two", "<p>one</p><p>two</p>"),
        ("<p>one<div>two</div>", "<p>one</p><div>two</div>"),
        (
            "<head><title>x</title><body><p>y</body>",
            "<head><title>x</title></head><body><p>y</p></body>",
        ),
        (
            "<html><head><meta charset=\"utf-8\"><title>t</title><body><h1>h</h1></body></html>",
            "<html><head><meta charset=\"utf-8\"><title>t</title></head><body><h1>h</h1></body></html>",
        ),
        (
            "<html><body><ul><li>a<li>b</ul>",
            "<html><body><ul><li>a</li><li>b</li></ul></body></html>",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(nesting(input), expected, "input: {input:?}");
    }
}

#[test]
fn closes_an_open_optional_element_inside_its_parent() {
    // The inserted end tag is the optional one, not the enclosing container.
    assert_eq!(nesting("<div><p>x</div>"), "<div><p>x</p></div>");
    assert_eq!(
        nesting("<body><section><li>x</section></body>"),
        "<body><section><li>x</li></section></body>"
    );
    assert_eq!(
        nesting("<div><dl><dt>a</dl></div>"),
        "<div><dl><dt>a</dt></dl></div>"
    );
}

#[test]
fn keeps_reporting_omitted_tags_that_are_not_optional() {
    for input in [
        "<div><span></div>",
        "<body><div>",
        "<div><p>x",
        "<ul><li>a",
        "<table><tr><td>a",
        "<div></span></div>",
    ] {
        let error = syntax_error(input);
        assert_eq!(error.kind, DiagnosticKind::Syntax, "{input:?}: {error}");
        assert!(
            error.message.contains("expected close tag"),
            "{input:?}: {error}"
        );
    }
}

#[test]
fn keeps_reporting_mismatched_end_tags() {
    let error = syntax_error("<ol><li>a<li>b</ul>");
    assert_eq!(error.kind, DiagnosticKind::Syntax);
    assert!(error.message.contains("expected close tag"), "{error}");
}
