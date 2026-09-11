use std::io::{self, Write};

use crate::screen::SignalGuard;
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::raw::RawMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Teardown {
    ClearViewport,
    #[default]
    KeepViewport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    ClearViewport,
    CursorToOrigin,
    ShowCursor,
    Flush,
    DisableRawMode,
}

fn steps(teardown: Teardown, raw: bool) -> Vec<Step> {
    let mut steps = Vec::with_capacity(5);
    if teardown == Teardown::ClearViewport {
        steps.push(Step::ClearViewport);
        steps.push(Step::CursorToOrigin);
    }
    steps.push(Step::ShowCursor);
    steps.push(Step::Flush);
    if raw {
        steps.push(Step::DisableRawMode);
    }
    steps
}

pub struct Inline<W: Write> {
    terminal: Terminal<crate::Backend<W>>,
    teardown: Teardown,
    raw: RawMode,
    signals: Option<SignalGuard>,
}

impl<W: Write> Inline<W> {
    pub fn new(writer: W, height: u16, teardown: Teardown) -> io::Result<Self> {
        Self::open(writer, height, teardown, None)
    }

    pub fn with_signals(
        writer: W,
        height: u16,
        teardown: Teardown,
        signals: SignalGuard,
    ) -> io::Result<Self> {
        Self::open(writer, height, teardown, Some(signals))
    }

    fn open(
        writer: W,
        height: u16,
        teardown: Teardown,
        signals: Option<SignalGuard>,
    ) -> io::Result<Self> {
        let raw = RawMode::enable()?;
        let mut terminal = Terminal::with_options(
            crate::Backend::new(crate::SgrWriter::new(writer)),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;
        terminal.hide_cursor()?;
        Ok(Self {
            terminal,
            teardown,
            raw,
            signals,
        })
    }

    pub fn terminal(&mut self) -> &mut Terminal<crate::Backend<W>> {
        &mut self.terminal
    }

    pub fn resize_viewport(&mut self, writer: W, height: u16) -> io::Result<()> {
        let origin = self.terminal.get_frame().area().as_position();
        self.terminal.clear()?;
        self.terminal.set_cursor_position(origin)?;
        self.terminal.backend_mut().flush()?;
        let mut terminal = Terminal::with_options(
            crate::Backend::new(crate::SgrWriter::new(writer)),
            TerminalOptions {
                viewport: Viewport::Inline(height.max(1)),
            },
        )?;
        terminal.hide_cursor()?;
        self.terminal = terminal;
        Ok(())
    }
}

impl<W: Write> Drop for Inline<W> {
    fn drop(&mut self) {
        let origin = self.terminal.get_frame().area().as_position();
        for step in steps(self.teardown, self.raw.is_enabled()) {
            match step {
                Step::ClearViewport => {
                    let _ = self.terminal.clear();
                }
                Step::CursorToOrigin => {
                    let _ = self.terminal.set_cursor_position(origin);
                }
                Step::ShowCursor => {
                    let _ = self.terminal.show_cursor();
                }
                Step::Flush => {
                    let _ = self.terminal.backend_mut().flush();
                }
                Step::DisableRawMode => {
                    let _ = self.raw.disable();
                }
            }
        }
        drop(self.signals.take());
    }
}

#[cfg(test)]
#[path = "../tests/unit/inline_tests.rs"]
mod tests;
