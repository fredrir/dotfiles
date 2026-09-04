use super::*;

#[test]
fn sync_ui_text_never_contains_terminal_controls_or_extra_lines() {
    let sanitized = sanitize_text("safe\nforged\r\u{1b}[31m\u{7} alert\tend");
    assert_eq!(sanitized, "safe forged �[31m� alert end");
    assert!(!sanitized.chars().any(char::is_control));
}
