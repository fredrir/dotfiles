use super::*;

const SAMPLE: &str = "[2026/09/11 15:50:20]\r\nAi Overclock Tuner [EXPO I]\r\nEXPO [DDR5-6000 30-36-36-76-1.40V-1.40V]\r\nCore Performance Boost [Enabled]\r\nCore Performance Boost [Auto]\r\n\r\n";

fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    bytes
}

#[test]
fn decodes_utf16_with_bom_and_plain_utf8() {
    assert_eq!(decode(&utf16le("a [b]")).unwrap(), "a [b]");
    assert_eq!(decode(b"\xEF\xBB\xBFa [b]").unwrap(), "a [b]");
    assert_eq!(decode(b"a [b]").unwrap(), "a [b]");
    assert!(decode(&[0xFF, 0xFE, 0x41]).is_err());
}

#[test]
fn normalize_strips_carriage_returns_and_trailing_blanks() {
    let text = normalize(SAMPLE);
    assert!(!text.contains('\r'));
    assert!(text.ends_with("Core Performance Boost [Auto]\n"));
    assert_eq!(text.lines().count(), 5);
}

#[test]
fn parse_keeps_header_order_and_duplicates() {
    let export = parse(&normalize(SAMPLE));
    assert_eq!(export.header.as_deref(), Some("[2026/09/11 15:50:20]"));
    assert_eq!(export.header_date().as_deref(), Some("20260911"));
    assert_eq!(export.settings.len(), 4);
    assert_eq!(
        export.value("EXPO"),
        Some("DDR5-6000 30-36-36-76-1.40V-1.40V")
    );
    let boosts = export.occurrences("Core Performance Boost");
    assert_eq!(boosts.len(), 2);
    assert_eq!(boosts[1].line, 5);
    assert_eq!(boosts[1].value, "Auto");
}

#[test]
fn hashes_and_names_are_stable() {
    assert_eq!(sha8("x").len(), 8);
    assert_eq!(sha8("x"), sha8("x"));
    assert_ne!(sha8("x"), sha8("y"));
    assert_eq!(
        file_name("archie", "1681", "20260911"),
        "archie-1681-20260911.txt"
    );
}

#[test]
fn exports_list_by_date_then_version() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "archie-1700-20260801.txt",
        "archie-1681-20260911.txt",
        "macie-1681-20261001.txt",
        "notes.txt",
    ] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }
    let names = list(dir.path(), "archie")
        .unwrap()
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["archie-1700-20260801.txt", "archie-1681-20260911.txt"]
    );
    assert!(
        latest(dir.path(), "archie")
            .unwrap()
            .unwrap()
            .ends_with("archie-1681-20260911.txt")
    );
    assert!(
        list(&dir.path().join("missing"), "archie")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn changed_tracks_occurrences_and_removals() {
    let before = parse("A [1]\nB [x]\nB [y]\nGone [1]\n");
    let after = parse("A [2]\nB [x]\nB [z]\nNew [1]\n");
    let changes = changed(&before, &after);
    assert_eq!(
        changes,
        vec![
            Change {
                name: "A".into(),
                occurrence: 1,
                from: Some("1".into()),
                to: Some("2".into())
            },
            Change {
                name: "B".into(),
                occurrence: 2,
                from: Some("y".into()),
                to: Some("z".into())
            },
            Change {
                name: "New".into(),
                occurrence: 1,
                from: None,
                to: Some("1".into())
            },
            Change {
                name: "Gone".into(),
                occurrence: 1,
                from: Some("1".into()),
                to: None
            },
        ]
    );
}
