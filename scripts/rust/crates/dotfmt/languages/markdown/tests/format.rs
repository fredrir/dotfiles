#![forbid(unsafe_code)]

use dotfmt_markdown::{
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
        "## Title\n\nsome very.\nimportant text\n\n## Title 2\n\nsome more important text"
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
fn trailing_blank_lines_are_trimmed_without_stripping_content_line_spaces() {
    for final_newline in [false, true] {
        let config = Config {
            final_newline,
            ..Config::default()
        };
        for body in ["1. abc\n2. efghij some text", "# Heading", "text  "] {
            let expected = format!("{body}{}", if final_newline { "\n" } else { "" });
            for suffix in ["\n\n\n", "\n \n\t\n", "\n\t  ", "\r\n \r\n\t\r\n"] {
                assert_eq!(formatted(&format!("{body}{suffix}"), &config), expected);
            }
        }
        for input in ["", "\n\n", " \n\t\n", "  \t"] {
            assert_eq!(formatted(input, &config), "");
        }
    }
}

#[test]
fn trailing_blank_lines_can_be_preserved_across_repeated_formatting() {
    for final_newline in [false, true] {
        let config = Config {
            trim_trailing_blank_lines: false,
            final_newline,
            ..Config::default()
        };
        for body in ["1. abc\n2. efghij some text", "# Heading", "text  ", ""] {
            for suffix in ["\n\n", "\n \n\t\n", "\n\t  "] {
                let input = format!("{body}{suffix}");
                let mut expected = input.clone();
                if final_newline && !expected.ends_with('\n') {
                    expected.push('\n');
                }
                assert_eq!(formatted(&input, &config), expected);
            }
        }
        assert_eq!(formatted("", &config), "");
        assert_eq!(
            formatted("text\n", &config),
            if final_newline { "text\n" } else { "text" }
        );
    }
}

#[test]
fn trailing_blank_line_trimming_preserves_blank_lines_inside_code() {
    for trim_trailing_blank_lines in [false, true] {
        let config = Config {
            trim_trailing_blank_lines,
            ..Config::default()
        };
        for input in ["```\ncode\n \n\t\n```", "```\ncode\n \n\t\n"] {
            let output = formatted(input, &config);
            assert_eq!(
                comrak::markdown_to_html(input, &options(&config)),
                comrak::markdown_to_html(&output, &options(&config))
            );
            assert!(output.contains("code\n \n\t\n```"), "{output}");
        }
    }
}

#[test]
fn short_tables_align_and_wide_tables_drop_padding_without_losing_content() {
    let input = "| Name | Value |\n| --- | --- |\n| long name | x |\n| a | long value |";
    assert_eq!(
        formatted(input, &Config::default()),
        "| Name      | Value      |\n| --- | --- |\n| long name | x          |\n| a         | long value |"
    );
    let config = Config {
        width: 25,
        ..Config::default()
    };
    assert_eq!(formatted(input, &config), input);
}

#[test]
fn divider_autosizing_follows_table_layout_when_enabled() {
    let input = "| Name | Value |\n| ----- | ---------- |\n| long name | long value |";
    for table_style in [TableStyle::Auto, TableStyle::Aligned, TableStyle::Compact] {
        for width in [0, 10, 80] {
            for autosize_table in [false, true] {
                let config = Config {
                    table_style,
                    width,
                    autosize_table,
                    ..Config::default()
                };
                let output = formatted(input, &config);
                let aligned = table_style == TableStyle::Aligned
                    || (table_style == TableStyle::Auto && width != 10);
                assert_eq!(
                    output.lines().nth(1).unwrap(),
                    if autosize_table && aligned {
                        "| --------- | ---------- |"
                    } else {
                        "| --- | --- |"
                    },
                    "{config:?}"
                );
            }
        }
    }
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
        .enumerate()
        .filter(|(row, _)| *row != 1)
        .map(|(_, line)| line)
        .map(unicode_width::UnicodeWidthStr::width)
        .collect();
    assert!(widths.iter().all(|width| *width == widths[0]), "{output}");
    assert_eq!(output.lines().nth(1).unwrap(), "| :--- | :---: | ---: |");
    let config = Config {
        autosize_table: true,
        ..Config::default()
    };
    let output = formatted(input, &config);
    assert_eq!(
        output.lines().nth(1).unwrap(),
        "| :--- | :------: | ------: |"
    );
    assert_eq!(
        comrak::markdown_to_html(input, &options(&config)),
        comrak::markdown_to_html(&output, &options(&config))
    );
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
fn normalizes_lists_emphasis_and_links_without_wrapping_prose() {
    let input = "* _one_ and __two__\n* [link][id]\n\n[id]: https://example.com 'Title'\n\nA paragraph with several words that must remain on the same line even beyond the configured table width.";
    let config = Config {
        width: 30,
        ..Config::default()
    };
    let output = formatted(input, &config);
    assert!(
        output.starts_with("- *one* and **two**\n- [link][id]"),
        "{output}"
    );
    assert!(
        output.contains("A paragraph with several words that must remain on the same line even beyond the configured table width."),
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
fn heading_spacing_and_final_newline_are_configurable() {
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

const SHARED_LIST: &str = "## `shared`

- `atuin`
- `direnv`
- `fastfetch`
- `gh`
- `git`
- `hport` — Ignore rules for the ports hport forwards from the peer
- `nvim`
- `obsidian`
- `op-bridge` — op that sends 1Password reads through the op-bridge daemon on macie; the rest goes to the real op
- `rsync`
- `ssh`
- `starship`
- `tools`
- `transcript`
- `ui`
- `vscode`
- `wez-vtabs` — wez-vtabs deploy targets
- `wezterm`
- `yazi`
- `zsh`";

#[test]
fn shared_list_remains_unchanged_in_every_dialect_at_any_table_width() {
    use dotfmt_markdown::dialect::Dialect;
    for dialect in [Dialect::Commonmark, Dialect::Gfm, Dialect::Obsidian] {
        for width in [0, 10, 80, 10000] {
            let config = Config {
                dialect,
                width,
                ..Config::default()
            };
            assert_eq!(formatted(SHARED_LIST, &config), SHARED_LIST);
        }
    }
}

#[test]
fn one_blank_line_does_not_expand_the_whole_list() {
    let input = SHARED_LIST.replace("- `starship`\n- `tools`", "- `starship`\n\n- `tools`");
    assert_eq!(formatted(&input, &Config::default()), SHARED_LIST);
    let wrapped = input.replace("daemon on macie", "daemon on\n  macie");
    let expected = SHARED_LIST.replace("daemon on macie", "daemon on\n  macie");
    assert_eq!(formatted(&wrapped, &Config::default()), expected);
}

#[test]
fn prose_quotes_and_list_continuations_keep_existing_line_breaks() {
    let config = Config {
        width: 10,
        ..Config::default()
    };
    let input = "A setting with a very long value must remain on this exact line.\nThe next line must remain separate.\n\n> A long quotation that must not wrap.\n> This line stays separate.\n\n- A long list item that must not wrap.\n  Existing continuation.";
    assert_eq!(formatted(input, &config), input);
}

#[test]
fn nested_ordered_and_task_lists_are_compact_but_multiple_paragraphs_stay_separate() {
    for (input, expected) in [
        ("1. first\n\n2. second", "1. first\n2. second"),
        ("- [ ] first\n\n- [x] second", "- [ ] first\n- [x] second"),
        (
            "- outer\n  - first\n\n  - second\n\n- next",
            "- outer\n  - first\n  - second\n- next",
        ),
        (
            "- first paragraph\n\n  second paragraph\n\n- next item",
            "- first paragraph\n\n  second paragraph\n\n- next item",
        ),
    ] {
        assert_eq!(formatted(input, &Config::default()), expected);
    }
}
