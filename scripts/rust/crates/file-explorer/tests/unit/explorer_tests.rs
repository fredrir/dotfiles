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
