use super::*;

#[test]
fn a_guard_that_never_enabled_raw_mode_reports_it() {
    let raw = RawMode { enabled: false };
    assert!(!raw.is_enabled());
}

#[test]
fn disabling_twice_leaves_the_terminal_alone_the_second_time() {
    let mut raw = RawMode { enabled: false };
    assert!(raw.disable().is_ok());
    assert!(raw.disable().is_ok());
    assert!(!raw.is_enabled());
}
