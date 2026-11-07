#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use dotfmt_html::config::LineEndings;
use dotfmt_html::{Config, Dialect, format};
use dotfmt_tailwind::{OrderTable, Sorter};

fn run(input: &str, config: &Config) -> String {
    let output = format(Path::new("x.html"), input, config, Dialect::Html).unwrap();
    assert_eq!(
        format(Path::new("x.html"), &output, config, Dialect::Html).unwrap(),
        output,
        "formatting must settle after one pass for {input:?}"
    );
    output
}

fn tailwind() -> Config {
    let table = OrderTable::from_pairs([
        ("p-4".to_owned(), Some(10)),
        ("flex".to_owned(), Some(5)),
        ("m-4".to_owned(), Some(1)),
        ("block".to_owned(), Some(2)),
    ]);
    Config {
        tailwind: dotfmt_tailwind::Config {
            mode: dotfmt_tailwind::Mode::On,
            ..dotfmt_tailwind::Config::default()
        },
        tailwind_sorter: Some(Sorter::new(
            Arc::new(table),
            dotfmt_tailwind::Config::default(),
        )),
        ..Config::default()
    }
}

#[test]
fn preserves_a_bom() {
    assert_eq!(
        run("\u{feff}<div> x </div>\n", &Config::default()),
        "\u{feff}<div>x</div>\n"
    );
    assert_eq!(run("\u{feff}", &Config::default()), "\u{feff}");
}

#[test]
fn normalizes_crlf_and_can_write_crlf_back() {
    assert_eq!(
        run("<div>\r\n<span>x</span>\r\n</div>", &Config::default()),
        "<div>\n  <span>x</span>\n</div>\n"
    );
    let config = Config {
        line_endings: LineEndings::Windows,
        ..Config::default()
    };
    assert_eq!(
        run("<div>\n<span>x</span>\n</div>", &config),
        "<div>\r\n  <span>x</span>\r\n</div>\r\n"
    );
    let config = Config {
        line_endings: LineEndings::Windows,
        final_newline: false,
        ..Config::default()
    };
    assert_eq!(
        run("<div>\n<span>x</span>\n</div>", &config),
        "<div>\r\n  <span>x</span>\r\n</div>"
    );
}

#[test]
fn final_newline_is_configurable_and_empty_input_stays_empty() {
    assert_eq!(run("", &Config::default()), "");
    assert_eq!(run("\n\n", &Config::default()), "");
    assert_eq!(run(" \n\t\n", &Config::default()), "");
    assert_eq!(run("<div>x</div>\n", &Config::default()), "<div>x</div>\n");
    let config = Config {
        final_newline: false,
        ..Config::default()
    };
    assert_eq!(run("<div>x</div>\n", &config), "<div>x</div>");
}

#[test]
fn preserves_comments_entities_unicode_and_raw_text() {
    let input =
        "<div title=\"a &amp; b\">caf\u{e9} &lt;tag&gt;<!-- <span class=\"p-4 flex\"> --></div>";
    assert_eq!(run(input, &Config::default()), format!("{input}\n"));

    let script = "<script>const template = '<div class=\"p-4 flex\">';</script>";
    assert_eq!(
        run(script, &tailwind()),
        "<script>\n  const template = '<div class=\"p-4 flex\">';\n</script>\n"
    );

    let style = "<style>.x::before { content: '<div class=\"p-4 flex\">'; }</style>";
    assert!(run(style, &tailwind()).contains("p-4 flex"));

    let pre = "<pre>  <div class=\"p-4 flex\">  </div>\n</pre>";
    assert_eq!(run(pre, &tailwind()), format!("{pre}\n"));
}

#[test]
fn prettier_ignore_file_leaves_the_whole_document_alone() {
    let input = "<!-- prettier-ignore -->\n<div    class=\"p-4 flex\"><span   >x</span></div>\n";
    assert_eq!(run(input, &tailwind()), input);
}

#[test]
fn prettier_ignore_element_leaves_only_the_next_element_alone() {
    let input = "<div>\n  <!-- prettier-ignore -->\n  <span    class=\"p-4 flex\"   >x</span>\n  <p   class=\"block m-4\">y</p>\n</div>";
    let output = run(input, &tailwind());
    assert!(
        output.contains("  <span    class=\"p-4 flex\"   >x</span>"),
        "{output}"
    );
    assert!(output.contains("<p class=\"m-4 block\">y</p>"), "{output}");
}

#[test]
fn empty_elements_respect_closing_tag_placement() {
    assert_eq!(run("<div></div>", &Config::default()), "<div></div>\n");

    let config = Config {
        closing_tag_line_break_for_empty: markup_fmt::config::ClosingTagLineBreakForEmpty::Always,
        ..Config::default()
    };
    assert_eq!(run("<div></div>", &config), "<div>\n</div>\n");

    let config = Config {
        closing_tag_line_break_for_empty: markup_fmt::config::ClosingTagLineBreakForEmpty::Never,
        ..Config::default()
    };
    assert_eq!(run("<div></div>", &config), "<div></div>\n");
}

#[test]
fn markup_options_are_observable_in_output() {
    let config = Config {
        quotes: markup_fmt::config::Quotes::Single,
        max_attrs_per_line: Some(1),
        ..Config::default()
    };
    let output = run(
        "<div class=\"p-4 flex\" data-long=\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"></div>",
        &config,
    );
    assert!(output.contains("class='p-4 flex'"), "{output}");
    assert!(output.contains("data-long='"), "{output}");

    let tabs = Config {
        indent_type: dotfmt_html::config::IndentType::Tabs,
        ..Config::default()
    };
    assert_eq!(
        run("<div>\n<span>x</span>\n</div>", &tabs),
        "<div>\n\t<span>x</span>\n</div>\n"
    );
}

/// Embedded snippets the formatter declines are preserved byte-for-byte. markup_fmt
/// re-indents raw snippets itself, which used to grow indentation on every pass when a
/// snippet mixed indented lines with column-zero ones (template literals, brace blocks).
#[test]
fn unformattable_embedded_snippets_keep_their_authored_bytes() {
    for input in [
        "<div><script> const broken = ; </script></div>\n",
        "<script>const tpl = `a\nb`;\n  const broken = ;\n</script>\n",
        "<div><style>.a { color: </style></div>\n",
        "<script type=\"text/x-template\">\n  <div class=\"a\">\n    <span>x</span>\n  </div>\n</script>\n",
        "<script type=\"application/json\">\n  {\"a\":1}\n</script>\n",
        "<script type=\"text/template\"><div   >x</div></script>\n",
    ] {
        assert_eq!(run(input, &Config::default()), input, "{input:?}");
        let mut output = input.to_owned();
        for _ in 0..3 {
            output = format(
                Path::new("x.html"),
                &output,
                &Config::default(),
                Dialect::Html,
            )
            .expect("preserved snippet stays valid");
        }
        assert_eq!(output, input, "indentation must not grow for {input:?}");
    }
}

#[test]
fn valid_embedded_snippets_still_format_and_settle() {
    assert_eq!(
        run(
            "<div><script>const a={b:1}</script></div>",
            &Config::default()
        ),
        "<div>\n  <script>\n    const a = { b: 1 };\n  </script>\n</div>\n"
    );
    assert_eq!(
        run("<script>const tpl = `a\nb`;</script>", &Config::default()),
        "<script>\n  const tpl = `a\nb`;\n</script>\n"
    );
    assert_eq!(
        run(
            "<div><style>.a{color:red}</style></div>",
            &Config::default()
        ),
        "<div>\n  <style>\n    .a {\n      color: red;\n    }\n  </style>\n</div>\n"
    );
}
