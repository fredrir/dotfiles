use super::*;

#[test]
fn explicit_color_modes_override_terminal_detection() {
    assert!(ColorMode::Always.enabled(false));
    assert!(!ColorMode::Never.enabled(true));
}
