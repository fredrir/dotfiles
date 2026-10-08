use super::*;

#[test]
fn the_sequence_is_base64_for_the_clipboard_selection() {
    assert_eq!(sequence("hello"), "\x1b]52;c;aGVsbG8=\x1b\\");
}

#[test]
fn an_empty_copy_still_sets_the_clipboard() {
    assert_eq!(sequence(""), "\x1b]52;c;\x1b\\");
}

#[test]
fn the_payload_has_no_line_breaks() {
    let long = "x".repeat(4096);
    assert!(!sequence(&long).contains('\n'));
}
