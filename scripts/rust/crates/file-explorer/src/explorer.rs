use std::fmt;

use workstation::Style;

use crate::render::{RenderContext, RenderLimits, render};
use crate::state::{PromptEdit, State};
use crate::{
    AcceptTarget, DefaultView, EntryKind, ExplorerView, FileSource, InputKind, Key, Outcome,
    SelectionPolicy, SystemTerminal, Terminal,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub max_width: usize,
    pub max_rows: usize,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            max_width: 78,
            max_rows: 14,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Prefetch {
    None,
    #[default]
    FocusedDirectory,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExplorerConfig {
    pub selection: SelectionPolicy,
    pub layout: Layout,
    pub prefetch: Prefetch,
}

#[derive(Debug)]
pub enum ExplorerError<S, T> {
    Source(S),
    Terminal(T),
}

pub type SystemExplorerError<S> = ExplorerError<S, std::io::Error>;
pub type TerminalExplorerError<S, T> = ExplorerError<S, T>;
pub type SystemExplorerResult<L, S> = Result<Outcome<L>, SystemExplorerError<S>>;
pub type TerminalExplorerResult<L, S, T> = Result<Outcome<L>, TerminalExplorerError<S, T>>;

impl<S: fmt::Display, T: fmt::Display> fmt::Display for ExplorerError<S, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => write!(formatter, "unable to open file explorer: {error}"),
            Self::Terminal(error) => write!(formatter, "file explorer terminal: {error}"),
        }
    }
}

impl<S, T> std::error::Error for ExplorerError<S, T>
where
    S: std::error::Error + 'static,
    T: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Terminal(error) => Some(error),
        }
    }
}

pub struct Explorer<'a, S, V = DefaultView>
where
    S: FileSource,
{
    source: S,
    start: S::Location,
    style: &'a Style,
    view: V,
    config: ExplorerConfig,
    initial_focus: Option<S::Location>,
}

impl<'a, S> Explorer<'a, S, DefaultView>
where
    S: FileSource,
{
    pub fn new(source: S, start: S::Location, style: &'a Style) -> Self {
        Self {
            source,
            start,
            style,
            view: DefaultView,
            config: ExplorerConfig::default(),
            initial_focus: None,
        }
    }
}

impl<'a, S, V> Explorer<'a, S, V>
where
    S: FileSource,
{
    pub fn config(mut self, config: ExplorerConfig) -> Self {
        self.config = config;
        self
    }

    pub fn accept_target(mut self, accept_target: AcceptTarget) -> Self {
        self.config.selection.accept_target = accept_target;
        self
    }

    pub fn selection_policy(mut self, selection: SelectionPolicy) -> Self {
        self.config.selection = selection;
        self
    }

    pub fn allow_missing_directory(mut self, allow: bool) -> Self {
        self.config.selection.allow_missing_directory = allow;
        self
    }

    pub fn selectable(mut self, predicate: fn(EntryKind) -> bool) -> Self {
        self.config.selection.selectable = predicate;
        self
    }

    pub fn initial_focus(mut self, location: S::Location) -> Self {
        self.initial_focus = Some(location);
        self
    }

    pub fn layout(mut self, layout: Layout) -> Self {
        self.config.layout = layout;
        self
    }

    pub fn prefetch(mut self, prefetch: Prefetch) -> Self {
        self.config.prefetch = prefetch;
        self
    }

    pub fn view<W>(self, view: W) -> Explorer<'a, S, W> {
        Explorer {
            source: self.source,
            start: self.start,
            style: self.style,
            view,
            config: self.config,
            initial_focus: self.initial_focus,
        }
    }
}

impl<S, V> Explorer<'_, S, V>
where
    S: FileSource,
    S::Error: fmt::Display,
    V: ExplorerView<S::Location>,
{
    pub fn run(&self) -> SystemExplorerResult<S::Location, S::Error> {
        let directory = self
            .source
            .read_directory(&self.start)
            .map_err(ExplorerError::Source)?;
        let Some(mut terminal) = SystemTerminal::open().map_err(ExplorerError::Terminal)? else {
            return Ok(Outcome::Unavailable);
        };
        self.run_loaded(&mut terminal, directory)
    }

    pub fn run_in<T>(
        &self,
        terminal: &mut T,
    ) -> TerminalExplorerResult<S::Location, S::Error, T::Error>
    where
        T: Terminal,
    {
        let directory = self
            .source
            .read_directory(&self.start)
            .map_err(ExplorerError::Source)?;
        self.run_loaded(terminal, directory)
    }

    fn run_loaded<T>(
        &self,
        terminal: &mut T,
        directory: crate::Directory<S::Location>,
    ) -> TerminalExplorerResult<S::Location, S::Error, T::Error>
    where
        T: Terminal,
    {
        let mut state = State::new(directory, self.config.selection);
        if let Some(location) = &self.initial_focus {
            state.focus_location(location);
        }
        let result = self.interact(terminal, &mut state);
        let cleared = terminal.clear().map_err(ExplorerError::Terminal);
        match (result, cleared) {
            (result @ Err(_), _) => result,
            (Ok(_), Err(error)) => Err(error),
            (Ok(outcome), Ok(())) => Ok(outcome),
        }
    }

    fn interact<T>(
        &self,
        terminal: &mut T,
        state: &mut State<S::Location>,
    ) -> TerminalExplorerResult<S::Location, S::Error, T::Error>
    where
        T: Terminal,
    {
        let mut last_prefetched = None;
        let mut help = false;
        loop {
            self.prefetch_focused(state, &mut last_prefetched);
            let mut frame = self.frame(state, terminal, help);
            if state.settle(frame.viewport_rows) {
                frame = self.frame(state, terminal, help);
            }
            let viewport_rows = frame.viewport_rows.max(1);
            terminal
                .draw(&frame.lines)
                .map_err(ExplorerError::Terminal)?;
            let key = terminal.read_key().map_err(ExplorerError::Terminal)?;
            if let Some(outcome) = self.apply_key(state, key, viewport_rows, &mut help) {
                return Ok(outcome);
            }
        }
    }

    fn frame<T>(
        &self,
        state: &State<S::Location>,
        terminal: &T,
        help: bool,
    ) -> crate::render::RenderedFrame
    where
        T: Terminal,
    {
        let selection = state.selection();
        let prompt = state.prompt().zip(state.input_kind());
        render(
            &RenderContext {
                directory: state.directory(),
                rows: state.rows(),
                cursor: state.cursor(),
                offset: state.offset(),
                prompt,
                error: state.error(),
                selection: selection.as_ref(),
                help,
            },
            &self.view,
            self.style,
            terminal.size(),
            RenderLimits {
                max_width: self.config.layout.max_width,
                max_rows: self.config.layout.max_rows,
            },
        )
    }

    fn apply_key(
        &self,
        state: &mut State<S::Location>,
        key: Key,
        viewport_rows: usize,
        help: &mut bool,
    ) -> Option<Outcome<S::Location>> {
        if key == Key::Interrupt {
            return Some(Outcome::Interrupted);
        }
        if state.prompt().is_some() {
            return self.apply_prompt_key(state, key, viewport_rows);
        }
        match key {
            Key::Up | Key::Char('k') => state.move_by(-1),
            Key::Down | Key::Char('j') => state.move_by(1),
            Key::PageUp => state.page_by(-1, viewport_rows),
            Key::PageDown => state.page_by(1, viewport_rows),
            Key::Home | Key::Char('g') => state.first(),
            Key::End | Key::Char('G') => state.last(),
            Key::Right | Key::Tab | Key::Char('l') => self.open_focused(state),
            Key::Left | Key::Char('h') => self.open_parent(state),
            Key::Char('/') => state.begin_prompt(InputKind::Search),
            Key::Char('r') => self.refresh(state),
            Key::Char('?') => *help = !*help,
            Key::Enter => return self.accept(state),
            Key::Escape | Key::Char('q') => return Some(Outcome::Cancelled),
            _ => {}
        }
        None
    }

    fn apply_prompt_key(
        &self,
        state: &mut State<S::Location>,
        key: Key,
        viewport_rows: usize,
    ) -> Option<Outcome<S::Location>> {
        match key {
            Key::Escape => state.cancel_prompt(),
            Key::Backspace => self.edit_prompt(state, PromptEdit::Backspace),
            Key::Kill => self.edit_prompt(state, PromptEdit::Kill),
            Key::WordBack => self.edit_prompt(state, PromptEdit::WordBack),
            Key::Char(character) => self.edit_prompt(state, PromptEdit::Insert(character)),
            Key::Up => state.move_by(-1),
            Key::Down => state.move_by(1),
            Key::PageUp => state.page_by(-1, viewport_rows),
            Key::PageDown => state.page_by(1, viewport_rows),
            Key::Home => state.first(),
            Key::End => state.last(),
            Key::Right | Key::Tab if state.input_kind() == Some(InputKind::Search) => {
                self.open_focused(state)
            }
            Key::Enter if state.input_kind() == Some(InputKind::Location) => {
                self.open_typed_location(state)
            }
            Key::Enter => return self.accept(state),
            _ => {}
        }
        None
    }

    fn edit_prompt(&self, state: &mut State<S::Location>, edit: PromptEdit) {
        state.edit_prompt(edit, |text| self.source.input_kind(text));
    }

    fn accept(&self, state: &mut State<S::Location>) -> Option<Outcome<S::Location>> {
        match state.selection() {
            Some(selection) => Some(Outcome::Selected(selection)),
            None => {
                state.set_error("nothing selectable here");
                None
            }
        }
    }

    fn open_focused(&self, state: &mut State<S::Location>) {
        let Some(entry) = state.focused() else {
            return;
        };
        if !entry.kind.is_directory() {
            return;
        }
        let location = entry.location.clone();
        self.open_directory(state, location, None);
    }

    fn open_parent(&self, state: &mut State<S::Location>) {
        let Some(parent) = state.directory().parent.clone() else {
            return;
        };
        let restore = state.directory().location.clone();
        self.open_directory(state, parent, Some(restore));
    }

    fn open_typed_location(&self, state: &mut State<S::Location>) {
        let Some(text) = state.prompt().map(str::to_string) else {
            return;
        };
        let current = state.directory().location.clone();
        match self.source.resolve_input(&current, &text) {
            Ok(location) => self.open_directory(state, location, None),
            Err(error) => state.set_error(error.to_string()),
        }
    }

    fn open_directory(
        &self,
        state: &mut State<S::Location>,
        location: S::Location,
        restore: Option<S::Location>,
    ) {
        match self.source.read_directory(&location) {
            Ok(directory) => state.replace_directory(directory, restore.as_ref()),
            Err(error) => state.set_error(error.to_string()),
        }
    }

    fn refresh(&self, state: &mut State<S::Location>) {
        let location = state.directory().location.clone();
        let focused = state.focused().map(|entry| entry.location.clone());
        match self.source.refresh_directory(&location) {
            Ok(directory) => state.replace_directory(directory, focused.as_ref()),
            Err(error) => state.set_error(error.to_string()),
        }
    }

    fn prefetch_focused(&self, state: &State<S::Location>, previous: &mut Option<S::Location>) {
        if self.config.prefetch == Prefetch::None {
            return;
        }
        let focused = state.focused().map(|entry| entry.location.clone());
        if previous.as_ref() == focused.as_ref() {
            return;
        }
        *previous = focused.clone();
        let Some(entry) = state.focused().filter(|entry| entry.kind.is_directory()) else {
            return;
        };
        self.source.prefetch(&entry.location);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::convert::Infallible;

    use crate::{Directory, DirectoryStatus, Entry, EntryKind, ScriptedTerminal, Selection, Size};

    use super::*;

    struct Source {
        directories: HashMap<String, Directory<String>>,
        prefetched: RefCell<Vec<String>>,
        refreshes: Cell<usize>,
    }

    impl Source {
        fn new() -> Self {
            let root = Directory {
                location: "/".to_string(),
                parent: None,
                label: "/".to_string(),
                entries: vec![
                    Entry {
                        location: "/docs".to_string(),
                        name: "docs".to_string(),
                        kind: EntryKind::Directory,
                    },
                    Entry {
                        location: "/notes.txt".to_string(),
                        name: "notes.txt".to_string(),
                        kind: EntryKind::File,
                    },
                ],
                status: DirectoryStatus::Present,
            };
            let docs = Directory {
                location: "/docs".to_string(),
                parent: Some("/".to_string()),
                label: "/docs".to_string(),
                entries: vec![Entry {
                    location: "/docs/guide.md".to_string(),
                    name: "guide.md".to_string(),
                    kind: EntryKind::File,
                }],
                status: DirectoryStatus::Present,
            };
            Self {
                directories: [("/".to_string(), root), ("/docs".to_string(), docs)]
                    .into_iter()
                    .collect(),
                prefetched: RefCell::new(Vec::new()),
                refreshes: Cell::new(0),
            }
        }
    }

    impl FileSource for Source {
        type Location = String;
        type Error = Infallible;

        fn read_directory(&self, location: &String) -> Result<Directory<String>, Self::Error> {
            Ok(self.directories[location].clone())
        }

        fn refresh_directory(&self, location: &String) -> Result<Directory<String>, Self::Error> {
            self.refreshes.set(self.refreshes.get() + 1);
            self.read_directory(location)
        }

        fn input_kind(&self, text: &str) -> InputKind {
            if text.starts_with('/') {
                InputKind::Location
            } else {
                InputKind::Search
            }
        }

        fn resolve_input(&self, _current: &String, text: &str) -> Result<String, Self::Error> {
            Ok(text.to_string())
        }

        fn prefetch(&self, location: &String) {
            self.prefetched.borrow_mut().push(location.clone());
        }
    }

    fn terminal(keys: impl IntoIterator<Item = Key>) -> ScriptedTerminal {
        ScriptedTerminal::new(
            Size {
                width: 60,
                height: 12,
            },
            keys,
        )
    }

    #[test]
    fn a_scripted_session_can_open_and_select_an_entry() {
        let source = Source::new();
        let style = Style::plain();
        let explorer = Explorer::new(source, "/".to_string(), &style)
            .accept_target(AcceptTarget::HighlightedEntry);
        let mut terminal = terminal([Key::Right, Key::Enter]);

        let outcome = explorer.run_in(&mut terminal).unwrap();

        assert_eq!(
            outcome,
            Outcome::Selected(Selection {
                location: "/docs/guide.md".to_string(),
                kind: EntryKind::File,
                label: "guide.md".to_string(),
            })
        );
        assert_eq!(terminal.clears, 1);
        assert!(terminal.frames.len() >= 2);
    }

    #[test]
    fn cancellation_and_interruption_are_distinct_and_always_clear() {
        let style = Style::plain();
        for (key, expected) in [
            (Key::Escape, Outcome::Cancelled),
            (Key::Interrupt, Outcome::Interrupted),
        ] {
            let explorer = Explorer::new(Source::new(), "/".to_string(), &style);
            let mut terminal = terminal([key]);
            assert_eq!(explorer.run_in(&mut terminal).unwrap(), expected);
            assert_eq!(terminal.clears, 1);
        }
    }

    #[test]
    fn a_zero_match_search_cannot_select_the_directory_by_accident() {
        let source = Source::new();
        let style = Style::plain();
        let explorer = Explorer::new(source, "/".to_string(), &style)
            .accept_target(AcceptTarget::HighlightedEntryOrCurrentDirectory);
        let mut terminal = terminal([
            Key::Char('/'),
            Key::Char('z'),
            Key::Enter,
            Key::Escape,
            Key::Escape,
        ]);

        assert_eq!(explorer.run_in(&mut terminal).unwrap(), Outcome::Cancelled);
        assert!(
            terminal
                .frames
                .iter()
                .flatten()
                .any(|line| line.contains("no matches"))
        );
    }

    #[test]
    fn refresh_and_prefetch_are_driven_only_by_relevant_actions() {
        let source = Source::new();
        let style = Style::plain();
        let explorer = Explorer::new(source, "/".to_string(), &style);
        let mut terminal = terminal([Key::Char('r'), Key::Escape]);

        assert_eq!(explorer.run_in(&mut terminal).unwrap(), Outcome::Cancelled);
        assert_eq!(explorer.source.refreshes.get(), 1);
        assert_eq!(explorer.source.prefetched.borrow().as_slice(), ["/docs"]);
    }
}
