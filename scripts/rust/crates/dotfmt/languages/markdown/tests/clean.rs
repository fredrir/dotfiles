#![forbid(unsafe_code)]

use dotfmt_markdown::{
    clean,
    config::Config,
    dialect::Dialect,
    format,
};

fn cleaned(input: &str, config: &Config) -> String {
    let formatted = format(input, config).unwrap();
    let output = clean(&formatted, config);
    assert_eq!(
        clean(&output, config),
        output,
        "cleaning must settle after one pass"
    );
    output
}

#[test]
fn inline_formatting_is_stripped_to_its_content() {
    let input = "**bold** and *emphasis* and ~~strike~~ and `code`\n";
    assert_eq!(cleaned(input, &Config::default()), "bold and emphasis and strike and code");
}

#[test]
fn links_and_images_keep_their_labels() {
    let input = "see [the docs](https://example.com \"title\") and ![alt](img.png)\n";
    assert_eq!(cleaned(input, &Config::default()), "see the docs and alt");
}

#[test]
fn headings_lose_their_markers() {
    for level in 1..=6 {
        let heading = "#".repeat(level);
        let input = format!("{heading} Title\n\nsome text");
        let expected = "Title\n\nsome text";
        assert_eq!(cleaned(&input, &Config::default()), expected);
    }
    assert_eq!(
        cleaned("Title\n=====\n\ntext", &Config::default()),
        "Title\n\ntext"
    );
}

#[test]
fn list_markers_and_tasks_are_stripped() {
    assert_eq!(cleaned("- one\n- two\n", &Config::default()), "one\n\ntwo");
    assert_eq!(cleaned("1. one\n2. two\n", &Config::default()), "one\n\ntwo");
    assert_eq!(cleaned("3. three\n", &Config::default()), "three");
    assert_eq!(
        cleaned("- [x] done\n- [ ] todo\n", &Config::default()),
        "done\n\ntodo"
    );
    assert_eq!(
        cleaned("- parent\n  - child\n", &Config::default()),
        "parent\n\nchild"
    );
}

#[test]
fn code_and_html_keep_their_literal_text() {
    let input = "```rust\nfn main() {}\n```\n\n<div>raw</div>\n";
    assert_eq!(
        cleaned(input, &Config::default()),
        "fn main() {}\n\n<div>raw</div>"
    );
}

#[test]
fn tables_become_cell_rows_separated_by_blank_lines() {
    let input = "| Name | Value |\n| --- | --- |\n| a | b |\n";
    assert_eq!(
        cleaned(input, &Config::default()),
        "Name  Value\n\na  b"
    );
}

#[test]
fn quotes_and_thematic_breaks_lose_their_markers() {
    assert_eq!(
        cleaned("> quoted\n> text\n", &Config::default()),
        "quoted text"
    );
    assert_eq!(
        cleaned("text\n\n---\n\nmore\n", &Config::default()),
        "text\n\nmore"
    );
}

#[test]
fn alerts_keep_their_body_without_the_marker() {
    assert_eq!(
        cleaned("> [!NOTE]\n> body text\n", &Config::default()),
        "body text"
    );
}

#[test]
fn frontmatter_is_preserved() {
    let input = "---\na: b\n---\n# Title\n";
    let expected = "---\na: b\n---\nTitle";
    assert_eq!(cleaned(input, &Config::default()), expected);
    let commonmark = Config {
        dialect: Dialect::Commonmark,
        ..Config::default()
    };
    // Without frontmatter support the delimiters and body parse as content.
    assert_eq!(cleaned(input, &commonmark), "a: b\n\nTitle");
}

#[test]
fn obsidian_wikilinks_and_math_keep_their_source() {
    let config = Config {
        dialect: Dialect::Obsidian,
        ..Config::default()
    };
    assert_eq!(cleaned("[[note]] and $x + 1$\n", &config), "[[note]] and $x + 1$");
}

#[test]
fn final_newline_and_blank_input_follow_configuration() {
    for final_newline in [false, true] {
        let config = Config {
            final_newline,
            ..Config::default()
        };
        let suffix = if final_newline { "\n" } else { "" };
        assert_eq!(
            cleaned("**bold**", &config),
            format!("bold{suffix}")
        );
        assert_eq!(cleaned("", &config), "");
        assert_eq!(cleaned("\n\n\n", &config), "");
    }
}
