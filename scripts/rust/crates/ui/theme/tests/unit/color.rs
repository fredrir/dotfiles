use super::*;

#[test]
fn color_policy_respects_explicit_modes_and_terminal_signals() {
    assert!(ColorMode::Always.enabled(false));
    assert!(!ColorMode::Never.enabled(true));
    for (terminal, no_color, clicolor, term, expected) in [
        (true, false, None, Some("xterm"), true),
        (false, false, None, Some("xterm"), false),
        (true, true, None, Some("xterm"), false),
        (true, false, Some("0"), Some("xterm"), false),
        (true, false, None, Some("DUMB"), false),
    ] {
        assert_eq!(auto_enabled(terminal, no_color, clicolor, term), expected);
    }
}

#[test]
fn color_depth_uses_terminal_capabilities() {
    assert_eq!(
        ColorDepth::from_signals(Some("truecolor"), Some("xterm-256color")),
        ColorDepth::TrueColor
    );
    assert_eq!(
        ColorDepth::from_signals(None, Some("xterm-direct")),
        ColorDepth::TrueColor
    );
    assert_eq!(
        ColorDepth::from_signals(None, Some("screen-256color")),
        ColorDepth::Ansi256
    );
    assert_eq!(
        ColorDepth::from_signals(None, Some("vt100")),
        ColorDepth::Ansi16
    );
}

#[test]
fn colors_reject_malformed_or_executable_terminal_sequences() {
    assert_eq!(Color::parse("#12AbEf").unwrap(), Color::Rgb(18, 171, 239));
    for source in [
        "", "red", "#fff", "#1234567", "#GG0000", "#é1234", "\x1b[31m",
    ] {
        assert!(Color::parse(source).is_err(), "{source:?}");
    }
}

#[test]
fn indexed_downgrade_keeps_exact_representable_colors() {
    assert_eq!(
        Color::Rgb(255, 0, 0).at_depth(ColorDepth::Ansi256),
        Color::Ansi(196)
    );
    assert_eq!(
        Color::Rgb(255, 0, 0).at_depth(ColorDepth::Ansi16),
        Color::Ansi(9)
    );
    assert_eq!(
        Color::Terminal.at_depth(ColorDepth::Ansi256),
        Color::Terminal
    );
}
