use super::*;

#[test]
fn tags_hash_each_input_that_exists() {
    let both = tags(Some("a"), Some("b"));
    assert_eq!(both.len(), 2);
    assert!(both[0].starts_with("bios:") && both[0].len() == 13);
    assert!(both[1].starts_with("lact:"));
    assert_eq!(tags(None, None), Vec::<String>::new());
    assert_eq!(tags(Some("a"), None)[0], both[0]);
}
