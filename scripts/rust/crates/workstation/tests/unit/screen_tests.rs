use super::*;

#[test]
fn width_counts_what_is_printed_rather_than_what_is_sent() {
    assert_eq!(width("my-app"), 6);
    assert_eq!(width("\x1b[1mmy-app\x1b[0m"), 6);
    assert_eq!(width(""), 0);
    assert_eq!(width("\x1b[0m"), 0);
}

#[test]
fn fit_keeps_short_text_whole() {
    assert_eq!(fit("my-app", 10), "my-app");
    assert_eq!(fit("my-app", 6), "my-app");
}

#[test]
fn fit_marks_the_text_it_had_to_cut() {
    assert_eq!(fit("my-application", 6), "my-ap…");
    assert_eq!(fit("my-app", 1), "…");
    assert_eq!(fit("my-app", 0), "");
}

#[test]
fn fit_measures_characters_rather_than_bytes() {
    assert_eq!(fit("émigré", 6), "émigré");
    assert_eq!(fit("émigré", 3), "ém…");
}
