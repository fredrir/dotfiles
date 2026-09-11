use super::*;

#[test]
fn a_fully_open_screen_restores_in_reverse_order() {
    assert_eq!(
        steps(true, true, true),
        [
            Step::ShowCursor,
            Step::DisableMouseCapture,
            Step::LeaveAlternateScreen,
            Step::DisableRawMode,
        ]
    );
}

#[test]
fn an_uncaptured_mouse_is_left_out_of_the_teardown() {
    assert_eq!(
        steps(false, true, true),
        [
            Step::ShowCursor,
            Step::LeaveAlternateScreen,
            Step::DisableRawMode,
        ]
    );
}

#[test]
fn every_undone_step_is_skipped_but_the_cursor_always_comes_back() {
    assert_eq!(steps(false, false, false), [Step::ShowCursor]);
}

#[test]
fn mouse_capture_defaults_to_released() {
    assert!(!MouseCapture::default().captured());
    assert!(MouseCapture::Enabled.captured());
}
