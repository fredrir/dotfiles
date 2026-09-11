use super::*;

#[test]
fn blocks_preserve_surroundings_and_reject_ambiguous_markers() {
    let text = "before\n<!-- cli:flags:start -->\nold\n<!-- cli:flags:end -->\nafter";
    assert_eq!(
        replace_block(text, "cli:flags", "new").unwrap(),
        text.replace("old", "new")
    );
    assert!(replace_block(&format!("{text}\n{text}"), "cli:flags", "new").is_err());
    assert!(replace_block("<!-- x:end --><!-- x:start -->", "x", "new").is_err());
    assert!(replace_block("authored page", "x", "new").is_err());
}
