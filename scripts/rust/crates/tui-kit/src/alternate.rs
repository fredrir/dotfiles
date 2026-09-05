use std::io::{self, Stdout, Write};

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::raw::RawMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseCapture {
    Enabled,
    #[default]
    Disabled,
}

impl MouseCapture {
    fn captured(self) -> bool {
        self == MouseCapture::Enabled
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    ShowCursor,
    DisableMouseCapture,
    LeaveAlternateScreen,
    DisableRawMode,
}

fn steps(mouse: bool, alternate: bool, raw: bool) -> Vec<Step> {
    let mut steps = vec![Step::ShowCursor];
    if mouse {
        steps.push(Step::DisableMouseCapture);
    }
    if alternate {
        steps.push(Step::LeaveAlternateScreen);
    }
    if raw {
        steps.push(Step::DisableRawMode);
    }
    steps
}

pub struct Alternate {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    raw: RawMode,
    alternate: bool,
    mouse: bool,
}

impl Alternate {
    pub fn new(mouse: MouseCapture) -> io::Result<Self> {
        let mouse = mouse.captured();
        let raw = RawMode::enable()?;
        let mut stdout = io::stdout();
        if let Err(error) = enter(&mut stdout, mouse) {
            leave(&mut stdout, mouse);
            return Err(error);
        }
        let mut terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                leave(&mut io::stdout(), mouse);
                return Err(error);
            }
        };
        if let Err(error) = terminal.hide_cursor() {
            let _ = terminal.show_cursor();
            leave(terminal.backend_mut(), mouse);
            return Err(error);
        }
        Ok(Self {
            terminal,
            raw,
            alternate: true,
            mouse,
        })
    }

    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

fn enter(writer: &mut impl Write, mouse: bool) -> io::Result<()> {
    if mouse {
        execute!(writer, EnterAlternateScreen, EnableMouseCapture)
    } else {
        execute!(writer, EnterAlternateScreen)
    }
}

fn leave(writer: &mut impl Write, mouse: bool) {
    if mouse {
        let _ = execute!(writer, DisableMouseCapture);
    }
    let _ = execute!(writer, LeaveAlternateScreen);
}

impl Drop for Alternate {
    fn drop(&mut self) {
        for step in steps(self.mouse, self.alternate, self.raw.is_enabled()) {
            match step {
                Step::ShowCursor => {
                    let _ = self.terminal.show_cursor();
                }
                Step::DisableMouseCapture => {
                    let _ = execute!(self.terminal.backend_mut(), DisableMouseCapture);
                    self.mouse = false;
                }
                Step::LeaveAlternateScreen => {
                    let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
                    self.alternate = false;
                }
                Step::DisableRawMode => {
                    let _ = self.raw.disable();
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/alternate_tests.rs"]
mod tests;
