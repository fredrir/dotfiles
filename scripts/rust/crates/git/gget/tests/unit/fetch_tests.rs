use super::*;

#[test]
fn a_pattern_is_anchored_at_the_root() {
    assert_eq!(pattern("folder_8/folder_9"), "/folder_8/folder_9");
    assert_eq!(pattern("README.md"), "/README.md");
}

#[test]
fn a_pattern_spells_out_what_gitignore_would_read() {
    assert_eq!(pattern("src/[id]/*.rs"), "/src/\\[id\\]/\\*.rs");
}
