use super::*;

#[test]
fn capture_aligns_unicode_strips_ansi_and_hides_local_ip() {
    let rendered = render(
        "\n\x1b[31m Shell \x1b[20Gwrong\x1b[0m\nテ Terminal \x1b[20Gwrong\n  Local IP \x1b[20G192.0.2.1\n e\u{301} \x1b[20Gvalue\n\n",
        "zsh 5.9",
        "kitty",
    );
    assert!(rendered.contains("zsh 5.9"));
    assert!(rendered.contains("kitty"));
    assert!(!rendered.contains("wrong"));
    assert!(!rendered.contains("192.0.2.1"));
    assert!(!rendered.contains('\x1b'));
    assert_eq!(visible_width("テe\u{301}"), 5);
    assert!(!rendered.starts_with('\n'));
    assert!(!rendered.ends_with('\n'));
}
