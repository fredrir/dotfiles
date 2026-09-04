use super::*;

#[test]
fn a_record_is_read_as_its_mode_says() {
    let kinds: Vec<Kind> = [
        "040000 tree abc\tfolder_8",
        "100644 blob abc\tREADME.md",
        "100755 blob abc\tsetup.sh",
        "120000 blob abc\tlink",
        "160000 commit abc\tvendor",
    ]
    .into_iter()
    .filter_map(entry)
    .map(|entry| entry.kind)
    .collect();
    assert!(matches!(
        kinds.as_slice(),
        [
            Kind::Directory,
            Kind::File { executable: false },
            Kind::File { executable: true },
            Kind::Link,
            Kind::Directory,
        ]
    ));
}

#[test]
fn a_name_is_whatever_follows_the_tab() {
    let named = entry("100644 blob abc\ta name with spaces.md").expect("the record is read");
    assert_eq!(named.name, "a name with spaces.md");
    assert!(entry("").is_none());
}

#[test]
fn an_entry_of_the_root_needs_no_separator() {
    assert_eq!(inside("HEAD:", "link"), "HEAD:link");
    assert_eq!(inside("HEAD:config", "link"), "HEAD:config/link");
}
