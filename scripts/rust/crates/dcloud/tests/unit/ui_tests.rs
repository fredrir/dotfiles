use super::*;
use serde_json::json;

#[test]
fn human_catalog_is_a_table_without_embedded_raw_records() {
    let value = json!({"items":[{"kind":"archive","id":"archive-id","host":"macie","job":"Documents","destination":"drive","manifest":{"secret_metadata":"not displayed"}}],"errors":[]});
    let rendered = human(&value);
    assert!(rendered.contains("DESTINATION"));
    assert!(rendered.contains("archive-id"));
    assert!(!rendered.contains("secret_metadata"));
    assert!(!rendered.contains('{'));
}

#[test]
fn terminal_controls_and_bidirectional_overrides_are_not_rendered() {
    let value = json!({"error":"bad\u{1b}[31m\nname\u{202e}exe"});
    let rendered = human(&value);
    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains('\u{202e}'));
    assert!(rendered.contains("bad�[31m�name�exe"));
}

#[test]
fn an_empty_catalog_never_opens_or_selects_the_synthetic_root() {
    assert_eq!(select(&[]).unwrap(), None);
}

#[test]
fn virtual_file_tree_does_not_follow_symlinks() {
    let tree = Tree {
        entries: vec![
            ("directory/file".into(), EntryKind::File),
            ("link".into(), EntryKind::Symlink),
        ],
    };
    let root = tree.read_directory(&PathBuf::new()).unwrap();
    assert_eq!(root.entries[0].kind, EntryKind::Directory);
    assert_eq!(root.entries[1].kind, EntryKind::Symlink);
    assert!(root.parent.is_none());
}
