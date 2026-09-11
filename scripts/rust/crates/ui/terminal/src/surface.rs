use crate::{Event, Key, Screen};
use std::collections::VecDeque;
use std::io;
use std::time::Duration;

pub trait Surface {
    type Error;
    fn size(&self) -> (usize, usize);
    fn draw(&mut self, lines: &[String]) -> Result<(), Self::Error>;
    fn event(&mut self) -> Result<Event, Self::Error>;
    fn clear(&mut self) -> Result<(), Self::Error>;
    fn poll_event(&mut self, _timeout: Duration) -> Result<Option<Event>, Self::Error> {
        self.event().map(Some)
    }
}

impl Surface for Screen {
    type Error = io::Error;
    fn size(&self) -> (usize, usize) {
        Screen::size(self).unwrap_or_else(|| {
            (
                crate::terminal_width().unwrap_or(80),
                crate::terminal_height().unwrap_or(24),
            )
        })
    }
    fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        Screen::draw(self, lines)
    }
    fn event(&mut self) -> io::Result<Event> {
        Screen::event(self)
    }
    fn clear(&mut self) -> io::Result<()> {
        Screen::clear(self)
    }
    fn poll_event(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        Screen::poll_event(self, timeout)
    }
}

pub struct ScriptedSurface {
    pub size: (usize, usize),
    pub events: VecDeque<Event>,
    pub frames: Vec<Vec<String>>,
    pub clears: usize,
}

impl ScriptedSurface {
    pub fn new(size: (usize, usize), events: impl IntoIterator<Item = Event>) -> Self {
        Self {
            size,
            events: events.into_iter().collect(),
            frames: Vec::new(),
            clears: 0,
        }
    }
    pub fn keys(size: (usize, usize), keys: impl IntoIterator<Item = Key>) -> Self {
        Self::new(size, keys.into_iter().map(Event::Key))
    }
}

impl Surface for ScriptedSurface {
    type Error = io::Error;
    fn size(&self) -> (usize, usize) {
        self.size
    }
    fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        self.frames.push(lines.to_vec());
        Ok(())
    }
    fn event(&mut self) -> io::Result<Event> {
        let event = self
            .events
            .pop_front()
            .unwrap_or(Event::Key(Key::Interrupt));
        if let Event::Resize { width, height } = event {
            self.size = (width, height);
        }
        Ok(event)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.clears += 1;
        Ok(())
    }
}
