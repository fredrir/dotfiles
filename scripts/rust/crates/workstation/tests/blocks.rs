use workstation::blocks::{self, Comments};

#[test]
fn records_keep_source_lines_and_comment_modes_preserve_part_numbers() {
    let source = "# header\narchie {\n  GPU = board #42\n\n  role = hyprland\n}\n";
    let entries = blocks::parse_with_comments(source, Comments::Lines).unwrap();
    assert_eq!(entries[0].block, "archie");
    assert!(entries[0].opens);
    assert_eq!(entries[1].number, 3);
    assert_eq!(entries[1].split(), ("GPU", "board #42"));
    assert_eq!(entries[2].number, 5);
    assert_eq!(blocks::parse(source).unwrap()[1].split(), ("GPU", "board"));
    assert_eq!(
        blocks::parse_with_comments("a {\n value = #42\n}", Comments::None).unwrap()[1].split(),
        ("value", "#42")
    );
}

#[test]
fn malformed_blocks_fail_before_consumers_receive_entries() {
    for (source, expected) in [
        ("}", "line 1: unexpected }"),
        ("a {\nb {", "line 2: nested block"),
        ("value = 1", "line 1: entry outside a block"),
        ("a {\nvalue = 1", "missing } for a"),
        ("{\n}", "line 1: block name missing"),
    ] {
        assert_eq!(blocks::parse(source).unwrap_err(), expected);
    }
}

#[test]
fn missing_file_is_empty_but_invalid_content_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("hosts.dotfile");
    assert!(blocks::read(&path).unwrap().is_empty());
    std::fs::write(&path, "outside").unwrap();
    let error = blocks::read(&path).unwrap_err();
    assert!(error.contains("hosts.dotfile"));
    assert!(error.contains("line 1"));
}

#[test]
fn compact_empty_blocks_match_multiline_blocks_and_keep_source_lines() {
    for source in ["packages {}\n", " packages \t{} \n"] {
        assert_eq!(
            blocks::parse(source).unwrap(),
            blocks::parse("packages {\n}\n").unwrap()
        );
    }
    let entries =
        blocks::parse("# header\nfirst {}\n\nsecond {\n value = 1\n}\nthird {}\n").unwrap();
    let records: Vec<_> = entries
        .iter()
        .map(|entry| {
            (
                entry.block.as_str(),
                entry.number,
                entry.text.as_str(),
                entry.opens,
            )
        })
        .collect();
    assert_eq!(
        records,
        [
            ("first", 2, "", true),
            ("second", 4, "", true),
            ("second", 5, "value = 1", false),
            ("third", 7, "", true),
        ]
    );
}

#[test]
fn compact_blocks_follow_the_existing_comment_policy() {
    let source = "# header\npackages {} # retained in line-only mode\n";
    let entries = blocks::parse_with_comments(source, Comments::Inline).unwrap();
    assert_eq!(entries[0].block, "packages");
    assert_eq!(entries[0].number, 2);
    assert!(entries[0].opens);
    assert_eq!(
        blocks::parse_with_comments(source, Comments::Lines).unwrap_err(),
        "line 2: entry outside a block"
    );
    assert_eq!(
        blocks::parse_with_comments(source, Comments::None).unwrap_err(),
        "line 1: entry outside a block"
    );
    for mode in [Comments::Inline, Comments::Lines, Comments::None] {
        assert_eq!(
            blocks::parse_with_comments("packages {}", mode).unwrap(),
            blocks::parse_with_comments("packages {\n}", mode).unwrap()
        );
    }
}

#[test]
fn literal_brace_entries_and_quoted_values_keep_their_existing_meaning() {
    let input = "packages {\nname{}\nliteral {}\nlib{foo,bar}\nquoted = \"literal {} #42\"\n}\n";
    for mode in [Comments::Lines, Comments::None] {
        let entries = blocks::parse_with_comments(input, mode).unwrap();
        assert_eq!(entries.len(), 5);
        assert!(
            entries[1..]
                .iter()
                .all(|entry| !entry.opens && entry.block == "packages")
        );
        assert_eq!(entries[1].text, "name{}");
        assert_eq!(entries[2].text, "literal {}");
        assert_eq!(entries[3].text, "lib{foo,bar}");
        assert_eq!(entries[4].split(), ("quoted", "\"literal {} #42\""));
    }
}

#[test]
fn compact_empty_blocks_do_not_open_a_scope_or_enable_nested_block_syntax() {
    for (source, expected) in [
        ("packages{} ", "line 1: entry outside a block"),
        ("{}", "line 1: block name missing"),
        ("packages {}\n}", "line 2: unexpected }"),
        ("packages {}\nvalue = 1", "line 2: entry outside a block"),
        ("outer {\ninner {\n}\n}", "line 2: nested block"),
    ] {
        assert_eq!(blocks::parse(source).unwrap_err(), expected);
    }
}
