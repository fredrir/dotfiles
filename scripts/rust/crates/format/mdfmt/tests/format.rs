#![forbid(unsafe_code)]

use mdfmt::{
    config::{Config, TableStyle},
    format, options,
};

fn formatted(input: &str, config: &Config) -> String {
    let output = format(input, config).unwrap();
    assert_eq!(
        format(&output, config).unwrap(),
        output,
        "formatting must settle after one pass"
    );
    output
}

#[test]
fn headings_have_one_blank_line_and_the_file_has_no_final_newline() {
    let input =
        "## Title\nsome very.\nimportant text\n\n\n## Title 2\n\n\nsome more important text\n\n";
    assert_eq!(
        formatted(input, &Config::default()),
        "## Title\n\nsome very. important text\n\n## Title 2\n\nsome more important text"
    );
    for level in 1..=6 {
        let heading = "#".repeat(level);
        assert_eq!(
            formatted(&format!("{heading} Title\ntext"), &Config::default()),
            format!("{heading} Title\n\ntext")
        );
    }
}

#[test]
fn short_tables_align_and_wide_tables_drop_padding_without_losing_content() {
    let input = "| Name | Value |\n| --- | --- |\n| long name | x |\n| a | long value |";
    assert_eq!(
        formatted(input, &Config::default()),
        "| Name      | Value      |\n| --------- | ---------- |\n| long name | x          |\n| a         | long value |"
    );
    let config = Config {
        width: 25,
        ..Config::default()
    };
    assert_eq!(formatted(input, &config), input);
}

#[test]
fn table_style_can_force_alignment_or_compaction() {
    let input = "a | b\n--- | ---\nlong name | value";
    let compact = Config {
        table_style: TableStyle::Compact,
        ..Config::default()
    };
    assert_eq!(
        formatted(input, &compact),
        "| a | b |\n| --- | --- |\n| long name | value |"
    );
    let aligned = Config {
        width: 10,
        table_style: TableStyle::Aligned,
        ..Config::default()
    };
    assert!(formatted(input, &aligned).starts_with("| a         | b     |"));
}

#[test]
fn table_alignment_unicode_and_escaped_pipes_survive() {
    let input = "| 左 | middle | right |\n| :--- | :---: | ---: |\n| 字 | `x\\|y` | a\\|b |\n| é | **bold** | ![a](x) |";
    let output = formatted(input, &Config::default());
    assert!(output.contains(":---"));
    assert!(output.contains("`x\\|y`"));
    assert!(output.contains("a\\|b"));
    assert_eq!(
        comrak::markdown_to_html(input, &options(&Config::default())),
        comrak::markdown_to_html(&output, &options(&Config::default()))
    );
    let widths: Vec<_> = output
        .lines()
        .map(unicode_width::UnicodeWidthStr::width)
        .collect();
    assert!(widths.iter().all(|width| *width == widths[0]), "{output}");
}

#[test]
fn code_html_and_frontmatter_are_not_treated_as_tables_or_headings() {
    for input in [
        "```md\n# heading\n\n\n| a | b |\n| --- | --- |\n| long | x |\n```",
        "<div>\n# heading\n| a | b |\n| --- | --- |\n</div>",
        "---\ntitle: Hello\narray: [a, b]\n---\n\n# Title\n\nText",
        "+++\ntitle = 'Hello'\n+++\n\n# Title\n\nText",
    ] {
        assert_eq!(formatted(input, &Config::default()), input);
    }
}

#[test]
fn nested_tables_and_headings_keep_their_container_prefixes() {
    let input = "> ## Title\n>\n> a | b\n> --- | ---\n> long | x\n\n- outer\n  - inner\n\n    a | b\n    --- | ---\n    x | longer";
    let output = formatted(input, &Config::default());
    assert!(output.contains("> ## Title\n>\n> |"), "{output}");
    assert_eq!(
        comrak::markdown_to_html(input, &options(&Config::default())),
        comrak::markdown_to_html(&output, &options(&Config::default()))
    );
}

#[test]
fn normalizes_lists_emphasis_links_and_wraps_prose() {
    let input = "* _one_ and __two__\n* [link][id]\n\n[id]: https://example.com 'Title'\n\nA paragraph with several words that should wrap at the configured width.";
    let config = Config {
        width: 30,
        ..Config::default()
    };
    let output = formatted(input, &config);
    assert!(
        output.starts_with("- *one* and **two**\n- [link](https://example.com \"Title\")"),
        "{output}"
    );
    assert!(
        output.contains("A paragraph with several words\nthat should wrap"),
        "{output}"
    );
}

#[test]
fn hard_breaks_tasks_footnotes_and_literal_markdown_keep_their_meaning() {
    let config = Config::default();
    for input in [
        "first  \nsecond\n\n- [ ] todo\n- [x] done",
        "Hello[^note].\n\n[^note]: a footnote",
        "Escaped \\*text\\* and \\[link\\]. ~~old~~ <https://example.com>",
        "3. three\n4. four\n\n    code",
        "| a | b |\n| --- | --- |\n| \\\\ | `a\\|b` |",
    ] {
        let output = formatted(input, &config);
        assert_eq!(
            comrak::markdown_to_html(input, &options(&config)),
            comrak::markdown_to_html(&output, &options(&config)),
            "{input} -> {output}"
        );
    }
}

#[test]
fn spacing_wrapping_and_final_newline_are_configurable() {
    let config = Config {
        width: 0,
        heading_blank_lines: 2,
        final_newline: true,
        ..Config::default()
    };
    assert_eq!(
        formatted("# Title\n\nfirst\nsecond\n\n", &config),
        "# Title\n\n\nfirst\nsecond\n"
    );
    assert_eq!(formatted("\n\n", &config), "");
    let config = Config {
        heading_blank_lines: 0,
        ..Config::default()
    };
    assert_eq!(formatted("# Title\n\ntext", &config), "# Title\ntext");
}

#[test]
fn headings_and_tables_at_container_boundaries_settle() {
    for input in [
        "- # Heading\n  body\n- second item",
        "- # Heading\n- second item",
        "> # Heading\n\nOutside",
        "> # Heading\n>\n>\n> Body",
        "# Last heading\n\n",
        "- # Heading\n\n  | a | b |\n  | --- | --- |\n  | long | cell |\n- other",
        "- | a | b |\n  | --- | --- |\n  | long | cell |",
        "> - # Heading\n>   body\n> - next",
        "| a | b |\n| --- | --- |\n| [long label](https://example.com/a/really/long/path) | **a long value** |",
    ] {
        formatted(input, &Config::default());
    }
}
