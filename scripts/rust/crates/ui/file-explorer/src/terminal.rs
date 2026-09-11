use std::io;

pub use ui_terminal::Key;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub width: usize,
    pub height: usize,
}

pub trait Terminal {
    type Error;

    fn size(&self) -> Size;
    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error>;
    fn read_key(&mut self) -> Result<Key, Self::Error>;
    fn clear(&mut self) -> Result<(), Self::Error>;
    fn read_event(&mut self) -> Result<ui_terminal::Event, Self::Error> {
        self.read_key().map(ui_terminal::Event::Key)
    }
    fn poll_event(
        &mut self,
        _timeout: std::time::Duration,
    ) -> Result<Option<ui_terminal::Event>, Self::Error> {
        self.read_event().map(Some)
    }
}

pub struct SystemTerminal {
    screen: ui_terminal::Screen,
}

impl SystemTerminal {
    pub fn open() -> io::Result<Option<Self>> {
        ui_terminal::Screen::open().map(|screen| screen.map(|screen| Self { screen }))
    }
}

impl Terminal for SystemTerminal {
    type Error = io::Error;

    fn size(&self) -> Size {
        let (width, height) = self.screen.size().unwrap_or_else(|| {
            (
                ui_terminal::terminal_width().unwrap_or(80),
                ui_terminal::terminal_height().unwrap_or(24),
            )
        });
        Size { width, height }
    }

    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error> {
        self.screen.draw(lines)
    }

    fn read_key(&mut self) -> Result<Key, Self::Error> {
        self.screen.key()
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.screen.clear()
    }
    fn read_event(&mut self) -> Result<ui_terminal::Event, Self::Error> {
        self.screen.event()
    }
    fn poll_event(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Option<ui_terminal::Event>, Self::Error> {
        self.screen.poll_event(timeout)
    }
}

pub struct ScriptedTerminal {
    pub size: Size,
    pub keys: std::collections::VecDeque<Key>,
    pub frames: Vec<Vec<String>>,
    pub clears: usize,
}

impl ScriptedTerminal {
    pub fn new(size: Size, keys: impl IntoIterator<Item = Key>) -> Self {
        Self {
            size,
            keys: keys.into_iter().collect(),
            frames: Vec::new(),
            clears: 0,
        }
    }
}

impl Terminal for ScriptedTerminal {
    type Error = io::Error;

    fn size(&self) -> Size {
        self.size
    }

    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error> {
        self.frames.push(lines.to_vec());
        Ok(())
    }

    fn read_key(&mut self) -> Result<Key, Self::Error> {
        Ok(self.keys.pop_front().unwrap_or(Key::Interrupt))
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.clears += 1;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/terminal_tests.rs"]
mod tests;
