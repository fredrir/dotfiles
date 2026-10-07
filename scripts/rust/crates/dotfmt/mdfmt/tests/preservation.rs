#![forbid(unsafe_code)]

use mdfmt::{config::Config, dialect::Dialect, format, options};

fn stable(input: &str, dialect: Dialect) -> String {
    let config = Config {
        dialect,
        ..Config::default()
    };
    let output = format(input, &config).unwrap();
    assert_eq!(
        format(&output, &config).unwrap(),
        output,
        "second pass changed {input:?}"
    );
    output
}

#[test]
fn unused_and_used_definitions_keep_their_source_and_location() {
    for dialect in [Dialect::Commonmark, Dialect::Gfm, Dialect::Obsidian] {
        for input in [
            "[future]: https://example.com \"Title\"",
            "Text\n\n[^note]: Important unpublished note",
            "[label][id]\n\n[id]: /url 'Title'\n\n[unused]: /other",
            "Text[^note].\n\n[^note]: First paragraph.\n\n    Second paragraph.",
        ] {
            assert_eq!(stable(input, dialect), input);
        }
    }
}

#[test]
fn ragged_tables_keep_extra_cells_and_fill_missing_cells() {
    let config = Config::default();
    for input in [
        "| a | b |\n| --- | --- |\n| one | two | DO NOT DROP |\n| short |",
        "> a | b\n> --- | ---\n> one | two | DO NOT DROP",
        "- a | b\n  --- | ---\n  one | two | DO NOT DROP",
    ] {
        let output = stable(input, Dialect::Gfm);
        assert!(output.contains("DO NOT DROP"), "{output}");
        assert_eq!(
            comrak::markdown_to_html(input, &options(&config)),
            comrak::markdown_to_html(&output, &options(&config))
        );
    }
}

#[test]
fn metadata_including_empty_blocks_and_yaml_terminators_is_preserved() {
    for dialect in [Dialect::Gfm, Dialect::Obsidian] {
        for metadata in [
            "---\n---",
            "+++\n+++",
            "---\ntitle: A\nvalue: false\n...",
            "---\nvalue: '__literal__'\n---",
        ] {
            let input = format!("{metadata}\n\n# Body\ntext");
            assert_eq!(
                stable(&input, dialect),
                format!("{metadata}\n\n# Body\n\ntext")
            );
        }
    }
}

#[test]
fn obsidian_escapes_tags_and_inline_footnotes_keep_their_meaning() {
    for input in [
        r"Literal \$x$ stays literal.",
        r"Literal \%% secret %% visible",
        r"\^blockid",
        "(#work)",
        "Here ^[inline note].",
        r"\==not highlighted==",
        r"\#tag",
        r"\[[Note]]",
        "#工作/笔记",
    ] {
        assert_eq!(stable(input, Dialect::Obsidian), input);
    }
}

#[test]
fn ordered_markers_and_multiline_obsidian_containers_keep_their_indentation() {
    for input in [
        "9. first\n1. > [!note] Title\n   > Body",
        "9. first\n1. $$\n   x_i = \\alpha\n   $$",
        "9. first\n1. %%\n   hidden\n   %%",
        "9. first\n1. paragraph\n   ^block-id",
    ] {
        assert_eq!(stable(input, Dialect::Obsidian), input);
    }
}

#[test]
fn github_math_is_preserved_even_when_it_looks_like_markdown() {
    for input in [
        r"The equation $x_1 = \alpha$ is useful.",
        "$$\nx_i = \\frac{a}{b}\n$$",
        "$$\n# not a heading\n\n__not emphasis__\n$$",
    ] {
        assert_eq!(stable(input, Dialect::Gfm), input);
    }
}

#[test]
fn html_and_code_keep_their_original_spelling_and_spacing() {
    for input in [
        "<a href=https://example.com><img src=https://example.com/image.svg?x=y></a>",
        "> ```\n> line\n>\n> ```",
        "    indented code\n    more code",
        "A `one\ntwo` code span.",
        "<div>\n__literal__\n# heading\n</div>",
    ] {
        assert_eq!(stable(input, Dialect::Gfm), input);
    }
}

#[test]
fn separate_lists_do_not_merge_or_gain_html_comments() {
    for input in ["- a\n\n* b", "1. a\n\n1) b", "* a\n\n    code"] {
        let output = stable(input, Dialect::Gfm);
        assert_eq!(output, input.replace("* a", "- a"));
        assert!(!output.contains("<!--"));
    }
}

#[test]
fn heading_spacing_does_not_expand_every_item_in_a_list() {
    assert_eq!(
        stable("- # Heading\n  body\n- next", Dialect::Gfm),
        "- # Heading\n\n  body\n- next"
    );
}

#[test]
fn inline_normalization_preserves_structure_in_ambiguous_contexts() {
    let config = Config::default();
    for input in [
        "_one_*two*",
        "__foo, __bar__, baz__",
        "____foo__ bar__",
        "_one_ and __two__",
        "é _one_ and [link](/url 'title')",
        "\t_code_",
        "first  \nsecond",
        "[link](/url 'a\"b')",
    ] {
        let output = stable(input, Dialect::Gfm);
        assert_eq!(
            comrak::markdown_to_html(input, &options(&config)),
            comrak::markdown_to_html(&output, &options(&config)),
            "{input} -> {output}"
        );
    }
    assert_eq!(
        stable("[link](/url 'title')", Dialect::Gfm),
        "[link](/url \"title\")"
    );
}

#[test]
fn preservation_does_not_disable_formatting_elsewhere_in_a_document() {
    let input = "---\n---\n\n# Title\n* __bold__ and [reference][id]\n\n[id]: /url\n\n[^unused]: keep this\n\nLiteral \\%% visible \\%%\n\n| a | b |\n| --- | --- |\n| long | x |";
    let output = stable(input, Dialect::Obsidian);
    assert!(
        output.starts_with("---\n---\n\n# Title\n\n- **bold** and [reference][id]"),
        "{output}"
    );
    assert!(output.contains("[id]: /url\n\n[^unused]: keep this"));
    assert!(output.contains("Literal \\%% visible \\%%"));
    // The escaped comment does not terminate the visible text or swallow the table.
    assert!(output.contains("| a    | b   |"), "{output}");
}

#[test]
fn inline_normalization_is_independent_of_the_original_final_newline() {
    for input in [
        "_text_\n\n<div>html</div>",
        "<table><tr><td>\n<pre>\n**Hello**,\n\n_world_.\n</pre>\n</td></tr></table>",
    ] {
        assert_eq!(
            stable(input, Dialect::Gfm),
            stable(&format!("{input}\n"), Dialect::Gfm)
        );
    }
}

#[test]
fn removing_the_final_newline_does_not_remove_code_content() {
    let config = Config::default();
    for input in [
        "`````\n\n```\naaa\n",
        "```\naaa\n    ```\n",
        "~~~~~~\naaa\n~~~ ~~\n",
        "> ```\n> code\n",
        "- ```\n  code\n",
    ] {
        let output = stable(input, Dialect::Gfm);
        assert!(!output.ends_with('\n'));
        assert_eq!(
            comrak::markdown_to_html(input, &options(&config)),
            comrak::markdown_to_html(&output, &options(&config)),
            "{input} -> {output}"
        );
    }
}

#[test]
fn table_escaping_and_container_indentation_preserve_rendered_cells() {
    let config = Config::default();
    let cells = [
        r"\\",
        r"\\\|",
        r"a\|b",
        r"`x\|y`",
        "_italic_",
        "é",
        "",
        "[x](/url)",
    ];
    for (first, rest) in [
        ("", ""),
        ("> ", "> "),
        ("- ", "  "),
        ("> - ", ">   "),
        ("   ", "   "),
    ] {
        for cell in cells {
            for outer_pipe in [true, false] {
                for count in 1..=3 {
                    let wrap = |text: String| {
                        if outer_pipe {
                            format!("|{text}|")
                        } else {
                            text
                        }
                    };
                    let header = wrap(vec!["header"; count].join(" | "));
                    let delimiter = wrap(vec![":---:"; count].join(" | "));
                    let row = wrap(vec![cell; count].join(" | "));
                    let input = format!("{first}{header}\n{rest}{delimiter}\n{rest}{row}");
                    let output = stable(&input, Dialect::Gfm);
                    assert_eq!(
                        comrak::markdown_to_html(&input, &options(&config)),
                        comrak::markdown_to_html(&output, &options(&config)),
                        "{input} -> {output}"
                    );
                }
            }
        }
    }
    let input = "- header | header\n  --- | ---\n\n    leading\n   :---:\n  `x\\|y`";
    let output = stable(input, Dialect::Gfm);
    assert_eq!(
        comrak::markdown_to_html(input, &options(&config)),
        comrak::markdown_to_html(&output, &options(&config)),
        "{input} -> {output}"
    );
}
