use crate::{Item, MatchMode, Mode, Navigation, Outcome, SelectionState};
use ui_terminal::{Event, Key, Screen, Surface};
use ui_theme::{Role, Style};
use ui_widgets::{KeyHint, Line, PromptBuffer, Span, choice, hints};

pub struct Picker<'a, K> {
    title: String,
    state: SelectionState<K>,
    style: &'a Style,
    max_rows: usize,
}

impl<'a, K: Clone + Eq> Picker<'a, K> {
    pub fn new(
        title: impl Into<String>,
        items: impl IntoIterator<Item = Item<K>>,
        style: &'a Style,
    ) -> Self {
        Self {
            title: title.into(),
            state: SelectionState::new(items.into_iter().collect(), Mode::Single),
            style,
            max_rows: 14,
        }
    }
    pub fn mode(mut self, mode: Mode) -> Self {
        self.state.set_mode(mode);
        self
    }
    pub fn matching(mut self, matching: MatchMode) -> Self {
        self.state.matching = matching;
        self
    }
    pub fn navigation(mut self, navigation: Navigation) -> Self {
        self.state.navigation = navigation;
        self
    }
    pub fn initial_focus(mut self, id: &K) -> Self {
        self.state.focus(id);
        self
    }
    pub fn max_rows(mut self, rows: usize) -> Self {
        self.max_rows = rows.max(1);
        self
    }
    pub fn run(mut self) -> std::io::Result<Outcome<K>> {
        let Some(mut screen) = Screen::open()? else {
            return Ok(Outcome::Unavailable);
        };
        self.run_in(&mut screen)
    }
    pub fn run_in<T: Surface>(&mut self, terminal: &mut T) -> Result<Outcome<K>, T::Error> {
        let result = self.interact(terminal);
        let cleared = terminal.clear();
        match (result, cleared) {
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            (Ok(outcome), Ok(())) => Ok(outcome),
        }
    }
    fn interact<T: Surface>(&mut self, terminal: &mut T) -> Result<Outcome<K>, T::Error> {
        let mut prompt = PromptBuffer::default();
        let mut searching = false;
        let mut help = false;
        let mut style = ui_theme::LiveStyle::new(self.style);
        let mut dirty = true;
        loop {
            dirty |= style.poll();
            let (width, height) = terminal.size();
            let rows = height.saturating_sub(3).min(self.max_rows);
            self.state.viewport.settle(self.state.rows().len(), rows);
            if dirty {
                terminal.draw(&self.frame(
                    (width, height, rows),
                    searching,
                    help,
                    style.style(),
                ))?;
                dirty = false;
            }
            let Some(event) = terminal.poll_event(std::time::Duration::from_secs(1))? else {
                continue;
            };
            dirty = true;
            let Event::Key(key) = event else {
                continue;
            };
            match key {
                Key::Interrupt => return Ok(Outcome::Interrupted),
                Key::Escape if searching => {
                    searching = false;
                    prompt.clear();
                    self.state.set_query("");
                }
                Key::Escape => return Ok(Outcome::Cancelled),
                Key::Char('q') if !searching => return Ok(Outcome::Cancelled),
                Key::Enter => {
                    let selected = self.state.selected();
                    if !selected.is_empty() {
                        return Ok(Outcome::Selected(selected));
                    }
                }
                Key::Up => self.state.move_by(-1),
                Key::Down => self.state.move_by(1),
                Key::PageUp => self.state.move_by(-(rows.max(1) as isize)),
                Key::PageDown => self.state.move_by(rows.max(1) as isize),
                Key::Home => self.state.first(),
                Key::End => self.state.last(),
                Key::Backspace => {
                    prompt.backspace();
                    self.state.set_query(prompt.as_str());
                }
                Key::Kill => {
                    prompt.clear();
                    self.state.set_query("");
                }
                Key::WordBack => {
                    prompt.word_back();
                    self.state.set_query(prompt.as_str());
                }
                Key::Tab => self.state.toggle(),
                Key::Char(' ') if !searching && self.state.mode() == Mode::Multiple => {
                    self.state.toggle()
                }
                Key::Char('*') if !searching => self.state.toggle_visible(),
                Key::Char('?') if !searching => help = !help,
                Key::Char('/') if !searching => searching = true,
                Key::Char('j') if !searching => self.state.move_by(1),
                Key::Char('k') if !searching => self.state.move_by(-1),
                Key::Char(character) => {
                    searching = true;
                    prompt.insert(character);
                    self.state.set_query(prompt.as_str());
                }
                _ => {}
            }
        }
    }
    fn frame(
        &self,
        size: (usize, usize, usize),
        searching: bool,
        help: bool,
        style: &Style,
    ) -> Vec<String> {
        let (width, height, rows) = size;
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut lines = vec![Line::styled(&self.title, Role::Strong)];
        for slot in self.state.viewport.visible(self.state.rows().len(), rows) {
            let index = self.state.rows()[slot];
            let item = &self.state.items()[index];
            let focused = slot == self.state.viewport.cursor;
            let mut line = if self.state.mode() == Mode::Multiple {
                choice(&item.label, focused, self.state.checked(index))
            } else {
                Line::from_spans([
                    Span::new(if focused { "▸ " } else { "  " }, Role::Accent),
                    Span::new(
                        &item.label,
                        if focused { Role::Strong } else { Role::Plain },
                    ),
                ])
            };
            if !item.detail.is_empty() {
                line.spans
                    .push(Span::new(format!("  {}", item.detail), Role::Muted));
            }
            if item.disabled {
                line.spans.push(Span::new("  unavailable", Role::Warning));
            }
            lines.push(line);
        }
        if self.state.rows().is_empty() && rows > 0 {
            lines.push(Line::styled(
                if self.state.items().is_empty() {
                    "(empty)"
                } else {
                    "(no matches)"
                },
                Role::Muted,
            ));
        }
        if searching {
            lines.push(Line::plain(format!("find {}▏", self.state.query())));
        } else {
            lines.push(Line::styled(
                format!(
                    "{} matches · {} selected",
                    self.state.rows().len(),
                    self.state.selected_count()
                ),
                Role::Muted,
            ));
        }
        lines.push(if help {
            hints(&[
                KeyHint::new("↑↓/jk", "move"),
                KeyHint::new("pgup/pgdn", "page"),
                KeyHint::new("/", "find"),
                KeyHint::new("esc", "back"),
            ])
        } else if self.state.mode() == Mode::Multiple {
            hints(&[
                KeyHint::new("space/tab", "toggle"),
                KeyHint::new("*", "all visible"),
                KeyHint::new("enter", "accept"),
                KeyHint::new("esc", "cancel"),
            ])
        } else {
            hints(&[
                KeyHint::new("↑↓", "move"),
                KeyHint::new("enter", "select"),
                KeyHint::new("/", "find"),
                KeyHint::new("?", "help"),
                KeyHint::new("esc", "cancel"),
            ])
        });
        lines
            .into_iter()
            .take(height)
            .map(|line| line.paint(style, width))
            .collect()
    }
}
