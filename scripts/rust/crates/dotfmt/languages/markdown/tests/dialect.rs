#![forbid(unsafe_code)]

use dotfmt_markdown::{config::Config, dialect::Dialect, format};

fn obsidian(input: &str) -> String {
    let config = Config {
        dialect: Dialect::Obsidian,
        ..Config::default()
    };
    let result = format(input, &config).unwrap();
    assert_eq!(
        format(&result, &config).unwrap(),
        result,
        "unstable output for {input:?}"
    );
    result
}

#[test]
fn commonmark_and_gfm_interpret_extension_syntax_differently() {
    let input = "| a | b |\n| --- | --- |\n| longest | x |\n\n~~old~~\n\n- [x] done";
    let commonmark = format(
        input,
        &Config {
            dialect: Dialect::Commonmark,
            ..Config::default()
        },
    )
    .unwrap();
    let gfm = format(
        input,
        &Config {
            dialect: Dialect::Gfm,
            ..Config::default()
        },
    )
    .unwrap();
    assert_ne!(commonmark, gfm);
    assert!(gfm.contains("| a       | b   |"), "{gfm}");
    assert!(gfm.contains("~~old~~"));
    assert!(gfm.contains("- [x] done"));
    assert!(!commonmark.contains("| a       | b   |"));
}

#[test]
fn github_alerts_keep_their_marker_separate_from_the_body() {
    let config = Config {
        dialect: Dialect::Gfm,
        ..Config::default()
    };
    let output = format("> [!NOTE]\n> Be careful.", &config).unwrap();
    assert!(output.starts_with("> [!NOTE]\n> "), "{output}");
    assert_eq!(format(&output, &config).unwrap(), output);
}

#[test]
fn wiki_links_and_embeds_keep_aliases_fragments_and_dimensions() {
    for input in [
        "[[My note]]",
        "[[Note#Heading|Alias]]",
        "[[Note#^block-id]]",
        "![[image.png|300x200]]",
        "![[document.pdf#page=3]]",
        "![[Note#Heading]]",
        "😃 [[Note with spaces|Unicode 标签]]",
    ] {
        assert_eq!(obsidian(input), input);
    }
    assert_eq!(
        obsidian("# Heading\n__Strong__ and [[My note]]."),
        "# Heading\n\n**Strong** and [[My note]]."
    );
}

#[test]
fn callouts_keep_custom_types_folding_and_nested_content_verbatim() {
    for input in [
        "> [!custom]- Custom title\n> Body with [[Note]].\n>\n> ## Heading\n> body",
        "> [!tip]+ Title\n> A paragraph\n>\n> > [!question]- Nested\n> > Text",
        "- > [!note]- Title\n  > Body",
    ] {
        assert_eq!(obsidian(input), input);
    }
}

#[test]
fn comments_and_math_keep_literal_contents() {
    for input in [
        "Text %% **private** [[Note]] %% end.",
        "%%\n# Hidden\n\n| a | b |\n| --- | --- |\n%%",
        "Text %% start\n# hidden\n%% end.",
        "$x_1 + \\alpha$",
        "$$\nx^2 + y^2\n$$",
        "> $$\n> x^2\n> $$",
        "Escaped \\[[literal]] and `%%not a comment%% $x$`",
    ] {
        let result = obsidian(input);
        // Escaping normal Markdown may be canonicalized, but protected syntax must stay intact.
        if !input.starts_with("Escaped") {
            assert_eq!(result, input);
        }
    }
    assert_eq!(obsidian("%% unfinished comment\n"), "%% unfinished comment");
}

#[test]
fn highlights_tags_and_block_ids_survive() {
    for input in [
        "==important== #tag/nested",
        "#tag",
        "Text ^block-id",
        "A paragraph\n^block-id",
        "- list item\n\n^list-id",
        "| a | b |\n| --- | --- |\n| x | y |\n\n^table-id",
    ] {
        let output = obsidian(input);
        if input.contains("^list-id") {
            assert!(output.contains("\n\n^list-id"));
        } else if input.contains("^table-id") {
            assert!(output.contains("\n\n^table-id"));
        } else {
            assert_eq!(output, input);
        }
    }
}

#[test]
fn wiki_cells_measure_source_width_and_preserve_escaped_pipes() {
    let input = "| Name | Link |\n| --- | --- |\n| x | [[Note\\|Alias]] |";
    let output = obsidian(input);
    assert!(output.contains("[[Note\\|Alias]]"));
    let widths: Vec<_> = output
        .lines()
        .enumerate()
        .filter(|(row, _)| *row != 1)
        .map(|(_, line)| line)
        .map(unicode_width::UnicodeWidthStr::width)
        .collect();
    assert!(widths.iter().all(|w| *w == widths[0]), "{output}");
}

#[test]
fn short_tags_and_math_cells_align_using_their_original_width() {
    let input = format!(
        "| Tag | Math |\n| --- | --- |\n{}",
        "| #a | $x$ |\n".repeat(15)
    );
    let output = obsidian(&input);
    let widths: Vec<_> = output
        .lines()
        .enumerate()
        .filter(|(row, _)| *row != 1)
        .map(|(_, line)| line)
        .map(unicode_width::UnicodeWidthStr::width)
        .collect();
    assert!(widths.iter().all(|w| *w == widths[0]), "{output}");
}

#[test]
fn protected_syntax_does_not_change_code_frontmatter_or_urls() {
    for input in [
        "```md\n[[link]] %%secret%% $x$ ==bold==\n```",
        "`[[Note]]` and `$x$`",
        "[label](https://example.com/%%literal%%)",
        "---\nname: '[[literal]] %%literal%% $x$'\n---\n\nBody",
        "+++\nname = '[[literal]] %%literal%%'\n+++\n\nBody",
        "\u{e000} [[Note]] \u{e001}",
    ] {
        assert_eq!(obsidian(input), input);
    }
}

#[test]
fn reference_definition_without_a_body_does_not_panic() {
    assert_eq!(obsidian("[foo]: /url\n"), "[foo]: /url");
}
