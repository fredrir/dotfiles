use std::io;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Backspace,
    Tab,
    Char(char),
    Interrupt,
    Kill,
    WordBack,
    Home,
    End,
    PageUp,
    PageDown,
    Unknown,
}

pub trait Terminal {
    type Error;

    fn size(&self) -> Size;
    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error>;
    fn read_key(&mut self) -> Result<Key, Self::Error>;
    fn clear(&mut self) -> Result<(), Self::Error>;
}

pub struct SystemTerminal {
    screen: workstation::Screen,
}

impl SystemTerminal {
    pub fn open() -> io::Result<Option<Self>> {
        workstation::Screen::open().map(|screen| screen.map(|screen| Self { screen }))
    }
}

impl Terminal for SystemTerminal {
    type Error = io::Error;

    fn size(&self) -> Size {
        let (width, height) = self.screen.size().unwrap_or_else(|| {
            (
                workstation::terminal_width().unwrap_or(80),
                workstation::terminal_height().unwrap_or(24),
            )
        });
        Size { width, height }
    }

    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error> {
        self.screen.draw(lines)
    }

    fn read_key(&mut self) -> Result<Key, Self::Error> {
        Ok(match self.screen.key()? {
            workstation::Key::Up => Key::Up,
            workstation::Key::Down => Key::Down,
            workstation::Key::Left => Key::Left,
            workstation::Key::Right => Key::Right,
            workstation::Key::Enter => Key::Enter,
            workstation::Key::Escape => Key::Escape,
            workstation::Key::Backspace => Key::Backspace,
            workstation::Key::Tab => Key::Tab,
            workstation::Key::Char(character) => Key::Char(character),
            workstation::Key::Interrupt => Key::Interrupt,
            workstation::Key::Kill => Key::Kill,
            workstation::Key::WordBack => Key::WordBack,
            workstation::Key::Home => Key::Home,
            workstation::Key::End => Key::End,
            workstation::Key::PageUp => Key::PageUp,
            workstation::Key::PageDown => Key::PageDown,
            workstation::Key::Unknown => Key::Unknown,
        })
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.screen.clear()
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
mod tests {
    use super::*;

    #[test]
    fn scripted_terminal_replays_keys_and_records_frames() {
        let mut terminal = ScriptedTerminal::new(
            Size {
                width: 31,
                height: 9,
            },
            [Key::Down, Key::Enter],
        );

        terminal.draw(&["first".into(), "second".into()]).unwrap();

        assert_eq!(
            terminal.size(),
            Size {
                width: 31,
                height: 9
            }
        );
        assert_eq!(terminal.read_key().unwrap(), Key::Down);
        assert_eq!(terminal.read_key().unwrap(), Key::Enter);
        assert_eq!(terminal.read_key().unwrap(), Key::Interrupt);
        assert_eq!(terminal.frames, vec![vec!["first", "second"]]);
    }

    #[test]
    fn scripted_terminal_records_every_clear() {
        let mut terminal = ScriptedTerminal::new(Size::default(), []);

        terminal.clear().unwrap();
        terminal.clear().unwrap();

        assert_eq!(terminal.clears, 2);
    }
}
