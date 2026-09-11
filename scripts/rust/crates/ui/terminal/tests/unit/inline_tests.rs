use super::*;

#[test]
fn clearing_rewinds_the_viewport_before_showing_the_cursor() {
    assert_eq!(
        steps(Teardown::ClearViewport, true),
        [
            Step::ClearViewport,
            Step::CursorToOrigin,
            Step::ShowCursor,
            Step::Flush,
            Step::DisableRawMode,
        ]
    );
}

#[test]
fn keeping_the_viewport_leaves_the_last_frame_on_screen() {
    assert_eq!(
        steps(Teardown::KeepViewport, true),
        [Step::ShowCursor, Step::Flush, Step::DisableRawMode]
    );
}

#[test]
fn raw_mode_that_was_never_entered_is_not_left() {
    assert_eq!(
        steps(Teardown::KeepViewport, false),
        [Step::ShowCursor, Step::Flush]
    );
}

#[test]
fn the_default_teardown_keeps_the_viewport() {
    assert_eq!(Teardown::default(), Teardown::KeepViewport);
}
