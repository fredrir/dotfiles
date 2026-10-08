use crate::{block, config::Config};

fn formatted(input: &str, config: &Config) -> String {
    let output = block::format(input, config).unwrap();
    assert_eq!(
        block::signature(input).unwrap(),
        block::signature(&output).unwrap(),
        "layout changed the entries in {input:?}"
    );
    assert_eq!(block::format(&output, config).unwrap(), output);
    output
}

fn line_for<'a>(output: &'a str, key: &str) -> &'a str {
    output
        .lines()
        .find(|line| {
            line.trim_start()
                .strip_prefix(key)
                .is_some_and(|tail| tail.starts_with(char::is_whitespace))
        })
        .unwrap_or_else(|| panic!("missing {key:?} in {output:?}"))
}

fn marker_column(output: &str, key: &str) -> usize {
    let line = line_for(output, key);
    let marker = line.rfind('#').unwrap();
    line[..marker].chars().count()
}

#[test]
fn trailing_comments_align_after_rendered_keys_and_values() {
    let config = Config {
        align_max: 4,
        ..Config::default()
    };
    let input = "group {\na = short # first\nbb = a much longer value # second\n# a standalone comment\na_key_over_the_cap = x # third\n}\n";
    let output = formatted(input, &config);
    let columns: Vec<_> = ["a", "bb", "a_key_over_the_cap"]
        .into_iter()
        .map(|key| marker_column(&output, key))
        .collect();
    assert!(
        columns.iter().all(|column| *column == columns[0]),
        "{output}"
    );
    assert_eq!(
        line_for(&output, "a").find('='),
        line_for(&output, "bb").find('=')
    );
    assert!(line_for(&output, "a_key_over_the_cap").find('=') > line_for(&output, "a").find('='));
    assert!(output.contains("a much longer value"));
    assert!(
        output
            .lines()
            .any(|line| line.trim() == "# a standalone comment")
    );
}

#[test]
fn comment_columns_restart_at_blank_lines_and_nested_block_boundaries() {
    let input = "outer {\na = a long first-group value # first\nbb = x # second\n\nc = x # third\ndd = y # fourth\nnested {\ne = an even longer nested-group value # fifth\nff = z # sixth\n}\ng = x # seventh\nhh = y # eighth\n}\n";
    let output = formatted(input, &Config::default());
    for [one, two] in [["a", "bb"], ["c", "dd"], ["e", "ff"], ["g", "hh"]] {
        assert_eq!(
            marker_column(&output, one),
            marker_column(&output, two),
            "{output}"
        );
    }
    assert!(
        marker_column(&output, "a") > marker_column(&output, "c"),
        "{output}"
    );
    assert!(
        marker_column(&output, "e") > marker_column(&output, "a"),
        "{output}"
    );
    assert_eq!(
        marker_column(&output, "c"),
        marker_column(&output, "g"),
        "{output}"
    );
}

#[test]
fn quoted_escaped_and_unseparated_hashes_remain_value_data() {
    let input = "group {\ndouble = \"value # inside\" # first\nsingle = 'another # inside' # second\nescaped = before \\# literal # third\npart = firmware#123 # fourth\nquoted_only = \"  untouched # bytes  \"\nliteral_only = part#number\nhash =#literal\n}\n";
    let output = formatted(input, &Config::default());
    for value in [
        "\"value # inside\"",
        "'another # inside'",
        "before \\# literal",
        "firmware#123",
        "\"  untouched # bytes  \"",
        "part#number",
    ] {
        assert!(output.contains(value), "{value:?}: {output}");
    }
    let column = marker_column(&output, "double");
    for key in ["single", "escaped", "part"] {
        assert_eq!(marker_column(&output, key), column, "{output}");
    }
    assert!(line_for(&output, "quoted_only").ends_with("\"  untouched # bytes  \""));
    assert!(line_for(&output, "literal_only").ends_with("part#number"));
    assert!(line_for(&output, "hash").ends_with("=#literal"));
}

#[test]
fn bare_pattern_data_keeps_literal_hashes_and_escaped_trailing_spaces() {
    let input =
        "excluded_files {\nfile#name\nfile=name # literal pattern text\n\\#leading\nspace\\ \n}\n";
    let output = formatted(input, &Config::default());
    for pattern in [
        "file#name",
        "file=name # literal pattern text",
        "\\#leading",
        "space\\ ",
    ] {
        assert!(
            output.lines().any(|line| line.trim_start() == pattern),
            "{pattern:?}: {output:?}"
        );
    }
}

#[test]
fn disabling_alignment_does_not_pad_comment_columns() {
    let config = Config {
        align: false,
        ..Config::default()
    };
    let output = formatted(
        "group {\na = 1     # first\nlonger = many words # second\n}\n",
        &config,
    );
    assert_eq!(
        output,
        "group {\n  a = 1 # first\n  longer = many words # second\n}"
    );
    assert_ne!(
        marker_column(&output, "a"),
        marker_column(&output, "longer")
    );
}

#[test]
fn empty_blocks_compact_and_keep_closing_comments_at_the_same_block() {
    let input = "json {\n}\nlua {\n} # Lua stays enabled\nouter {\nchild{\n}\n}\n";
    let output = formatted(input, &Config::default());
    assert_eq!(
        output,
        "json {}\nlua {} # Lua stays enabled\nouter {\n  child {}\n}"
    );
    let anonymous = formatted("{\n}\n", &Config::default());
    assert_eq!(anonymous, "{}");
}

#[test]
fn header_comments_comment_only_blocks_and_entries_prevent_compaction() {
    let input = "header { # keep on the header\n}\ncommented {\n# keep inside\n}\nentry {\na = 1\n}\nbare {\npackage\n}\n";
    let output = formatted(input, &Config::default());
    assert!(
        output.contains("header { # keep on the header\n}"),
        "{output}"
    );
    assert!(
        output.contains("commented {\n  # keep inside\n}"),
        "{output}"
    );
    for name in ["entry", "bare"] {
        assert!(output.contains(&format!("{name} {{\n")), "{output}");
        assert!(!output.contains(&format!("{name} {{}}")), "{output}");
    }
    assert!(output.lines().any(|line| line.trim() == "package"));
}

#[test]
fn adjacent_empty_blocks_align_their_closing_comments_after_compaction() {
    let input = "json {\n} # JSON\nmarkdown {\n} # Markdown\n\nlua {\n} # Lua\n";
    let output = formatted(input, &Config::default());
    assert_eq!(
        marker_column(&output, "json"),
        marker_column(&output, "markdown"),
        "{output}"
    );
    assert!(
        marker_column(&output, "lua") < marker_column(&output, "markdown"),
        "{output}"
    );
    for name in ["json", "markdown", "lua"] {
        assert!(line_for(&output, name).contains("{}"), "{output}");
    }
}

#[test]
fn empty_values_tabs_and_escaped_separators_keep_comment_boundaries() {
    let input = "group {\nempty = # empty value\nblank =\ntab = value\t# tab separator\nescaped = value\\ #literal # real comment\n}\n";
    let output = formatted(input, &Config::default());
    let column = marker_column(&output, "empty");
    for key in ["tab", "escaped"] {
        assert_eq!(marker_column(&output, key), column, "{output}");
    }
    let empty = line_for(&output, "empty");
    assert!(
        empty[empty.find('=').unwrap() + 1..empty.find('#').unwrap()]
            .trim()
            .is_empty()
    );
    assert!(line_for(&output, "blank").ends_with('='));
    assert!(line_for(&output, "escaped").contains("value\\ #literal"));
    let unaligned = formatted(
        input,
        &Config {
            align: false,
            ..Config::default()
        },
    );
    assert!(line_for(&unaligned, "empty").ends_with("= # empty value"));
    assert!(line_for(&unaligned, "tab").ends_with("= value # tab separator"));
    assert!(line_for(&unaligned, "escaped").ends_with("= value\\ #literal # real comment"));
}

#[test]
fn unicode_keys_and_values_align_by_columns_instead_of_utf8_byte_offsets() {
    let output = formatted(
        "group {\né = café # first\nlonger = tea # second\n}\n",
        &Config::default(),
    );
    assert_eq!(
        marker_column(&output, "é"),
        marker_column(&output, "longer"),
        "{output}"
    );
    let accented = line_for(&output, "é");
    let ascii = line_for(&output, "longer");
    assert_ne!(accented.find('#'), ascii.find('#'));
    assert_eq!(
        accented[..accented.find('=').unwrap()].chars().count(),
        ascii[..ascii.find('=').unwrap()].chars().count()
    );
    assert!(accented.contains("café"));
}

#[test]
fn whitespace_prefixed_part_numbers_and_tags_preserve_literal_value_bytes() {
    let input = "group {\nGPU = board #42\ntag = firmware  #release\nlonger = an unrelated longer value # annotation\n}\n";
    for align in [false, true] {
        let output = formatted(
            input,
            &Config {
                align,
                ..Config::default()
            },
        );
        for (key, expected) in [("GPU", "board #42"), ("tag", "firmware  #release")] {
            let (_, value) = line_for(&output, key).split_once('=').unwrap();
            assert_eq!(value.trim_start(), expected, "{output}");
        }
    }
}

#[test]
fn part_numbers_before_real_comments_remain_data_and_empty_comments_align() {
    let input = "group {\nGPU = board #42 # model\ntag = firmware #release # channel\nlonger = an unrelated longer value #\n}\n";
    let output = formatted(input, &Config::default());
    assert!(line_for(&output, "GPU").contains("board #42"), "{output}");
    assert!(
        line_for(&output, "tag").contains("firmware #release"),
        "{output}"
    );
    let column = marker_column(&output, "GPU");
    for key in ["tag", "longer"] {
        assert_eq!(marker_column(&output, key), column, "{output}");
    }
}
