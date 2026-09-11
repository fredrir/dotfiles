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
