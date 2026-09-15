#![deny(unsafe_code)]

#[cfg(feature = "ratatui")]
mod alternate;
#[cfg(feature = "ratatui")]
mod inline;
#[cfg(feature = "crossterm")]
mod input;
#[cfg(feature = "crossterm")]
mod raw;
#[cfg(feature = "ratatui")]
mod sgr;
#[cfg(feature = "ratatui")]
mod style;
#[cfg(feature = "ratatui")]
pub use sgr::SgrWriter;
#[cfg(feature = "ratatui")]
pub type Backend<W> = ratatui::backend::CrosstermBackend<SgrWriter<W>>;
mod policy;
pub mod screen;
mod surface;
pub mod text;
pub use policy::{UiPolicy, capable, environment_flag_enabled, reduced_motion_requested};
pub use surface::{ScriptedSurface, Surface};

#[cfg(feature = "ratatui")]
pub use alternate::{Alternate, MouseCapture};
#[cfg(feature = "ratatui")]
pub use inline::{Inline, Teardown};
#[cfg(feature = "crossterm")]
pub use input::{Input, Waited};
#[cfg(feature = "crossterm")]
pub use raw::RawSession;
pub use screen::{
    Event, Key, Screen, SignalGuard, SignalOptions, termination_requested, termination_signal,
};
#[cfg(feature = "ratatui")]
pub use style::ui_style;

pub fn terminal_width() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .or_else(|| terminal_size().map(|(width, _)| width))
}

pub fn terminal_height() -> Option<usize> {
    std::env::var("LINES")
        .ok()
        .and_then(|value| value.parse().ok())
        .or_else(|| terminal_size().map(|(_, height)| height))
}

#[cfg(unix)]
fn terminal_size() -> Option<(usize, usize)> {
    let size = rustix::termios::tcgetwinsize(std::io::stdout()).ok()?;
    (size.ws_col > 0).then_some((size.ws_col as usize, size.ws_row as usize))
}

#[cfg(not(unix))]
fn terminal_size() -> Option<(usize, usize)> {
    None
}
