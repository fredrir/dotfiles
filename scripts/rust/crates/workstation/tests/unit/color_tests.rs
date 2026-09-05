use super::*;

#[test]
fn always_and_never_ignore_the_terminal_and_the_environment() {
    assert!(ColorMode::Always.enabled(false));
    assert!(ColorMode::Always.enabled(true));
    assert!(!ColorMode::Never.enabled(false));
    assert!(!ColorMode::Never.enabled(true));
}

#[test]
fn auto_needs_a_terminal_before_it_needs_anything_else() {
    assert!(!ColorMode::Auto.enabled(false));
    assert!(!auto_enabled(false, false, None, None));
    assert!(auto_enabled(true, false, None, None));
}

#[test]
fn auto_gives_way_to_every_signal_that_asks_for_no_colour() {
    assert!(!auto_enabled(true, true, None, None));
    assert!(!auto_enabled(true, false, Some("0"), None));
    assert!(!auto_enabled(true, false, None, Some("dumb")));
    assert!(!auto_enabled(true, false, None, Some("DUMB")));
    assert!(!auto_enabled(true, false, None, Some("Dumb")));
}

#[test]
fn auto_keeps_colour_for_the_neighbouring_values_of_those_signals() {
    assert!(auto_enabled(true, false, Some("1"), Some("xterm-256color")));
    assert!(auto_enabled(true, false, Some(""), Some("dumber")));
}

#[test]
fn auto_is_the_mode_a_missing_flag_falls_back_to() {
    assert_eq!(ColorMode::default(), ColorMode::Auto);
}
