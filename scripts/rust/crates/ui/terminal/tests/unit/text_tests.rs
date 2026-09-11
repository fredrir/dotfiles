use super::*;

#[test]
fn fitting_counts_terminal_cells_and_hides_controls() {
    assert_eq!(fit("東京abc", 5), "東京…");
    assert_eq!(width("\x1b[31m東京\x1b[0m"), 4);
    assert_eq!(fit("x\x1b[2J", 40), "x\\u{1b}[2J");
    assert_eq!(truncate_front("abcdef", 4), "…def");
    assert_eq!(truncate_back("東京", 0), "");
}

#[test]
fn wrapping_preserves_words_and_bounds_wide_or_unbroken_text() {
    assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    assert_eq!(wrap("東京abc", 4), ["東京", "abc"]);
    assert_eq!(wrap("e\u{301}bc", 2), ["e\u{301}b", "c"]);
    assert_eq!(wrap("abc", 0), [""]);
    for limit in 1..12 {
        for line in wrap("東京abc verylongword\x1b[2J", limit) {
            assert!(width(&line) <= limit, "{limit}: {line:?}");
            assert!(!line.contains('\x1b'));
        }
    }
    assert_eq!(pad_right("東京", 6), "東京  ");
}
