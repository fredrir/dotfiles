use super::*;
use vte::{Parser, Perform};

struct Track(Option<Foreground>);

impl Perform for Track {
    fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, _: char) {
        self.0 = Foreground::set_by(params);
    }
}

fn set_by(sequence: &str) -> Option<Foreground> {
    let mut track = Track(None);
    Parser::new().advance(&mut track, sequence.as_bytes());
    track.0
}

fn after(start: Foreground, sequence: &str) -> Foreground {
    set_by(sequence).unwrap_or(start)
}

const RED: Foreground = Foreground::Basic(31);

#[test]
fn basic_and_bright_colors_set_the_foreground() {
    assert_eq!(after(Foreground::Default, "\x1b[31m"), RED);
    assert_eq!(
        after(Foreground::Default, "\x1b[1;94m"),
        Foreground::Basic(94)
    );
}

#[test]
fn indexed_and_rgb_colors_set_the_foreground() {
    assert_eq!(after(RED, "\x1b[38;5;208m"), Foreground::Indexed(208));
    assert_eq!(after(RED, "\x1b[38;2;1;2;3m"), Foreground::Rgb(1, 2, 3));
}

#[test]
fn colon_separated_colors_set_the_foreground() {
    assert_eq!(after(RED, "\x1b[38:5:208m"), Foreground::Indexed(208));
    assert_eq!(after(RED, "\x1b[38:2::1:2:3m"), Foreground::Rgb(1, 2, 3));
    assert_eq!(after(RED, "\x1b[38:2:1:2:3m"), Foreground::Rgb(1, 2, 3));
}

#[test]
fn resets_return_to_the_default() {
    for reset in ["\x1b[m", "\x1b[0m", "\x1b[39m", "\x1b[1;0m"] {
        assert_eq!(after(RED, reset), Foreground::Default, "{reset:?}");
    }
}

#[test]
fn background_and_underline_colors_leave_the_foreground_alone() {
    assert_eq!(after(RED, "\x1b[48;2;31;32;33m"), RED);
    assert_eq!(after(RED, "\x1b[48;5;31m"), RED);
    assert_eq!(after(RED, "\x1b[58:2::31:32:33m"), RED);
}

#[test]
fn attributes_leave_the_foreground_alone() {
    assert_eq!(after(RED, "\x1b[1;3;4m"), RED);
    assert_eq!(set_by("\x1b[1;48;5;1m"), None);
}

#[test]
fn the_last_color_in_a_sequence_wins() {
    assert_eq!(
        after(Foreground::Default, "\x1b[31;48;5;1;32m"),
        Foreground::Basic(32)
    );
}

#[test]
fn a_truncated_extended_color_keeps_the_previous_one() {
    assert_eq!(after(RED, "\x1b[38;2;1m"), RED);
    assert_eq!(after(RED, "\x1b[38m"), RED);
}

#[test]
fn every_foreground_writes_back_as_sgr() {
    assert_eq!(Foreground::Default.to_string(), "\x1b[39m");
    assert_eq!(RED.to_string(), "\x1b[31m");
    assert_eq!(Foreground::Indexed(208).to_string(), "\x1b[38;5;208m");
    assert_eq!(Foreground::Rgb(1, 2, 3).to_string(), "\x1b[38;2;1;2;3m");
}
