use std::path::PathBuf;

use file_explorer::{
    AcceptTarget, DefaultView, Directory, DirectoryStatus, Entry, EntryKind, Explorer,
    ExplorerError, ExplorerView, FileSource, InputKind, Line, Outcome, Role, Span, ViewContext,
};
use hostkit::Route;
use workstation::Style;

use crate::cli::Direction;
use crate::place;
use crate::remote::{Peer, Target};

pub enum Chosen {
    Picked(String),
    Cancelled,
    Interrupted,
    Unavailable,
}

pub struct Browser<'a> {
    pub direction: Direction,
    pub peer: &'a Peer,
    pub style: &'a Style,
    pub this: &'a str,
    pub route: Option<Route>,
    pub name: Option<String>,
    pub local_display: String,
    pub start: String,
    pub mirror: Option<String>,
    pub remote_home: String,
    pub home: PathBuf,
    pub here: PathBuf,
}

struct RemoteSource<'a> {
    peer: &'a Peer,
    home: &'a str,
}

impl FileSource for RemoteSource<'_> {
    type Location = String;
    type Error = String;

    fn read_directory(&self, location: &String) -> Result<Directory<String>, Self::Error> {
        self.directory(
            location,
            self.peer.list(&Target::Absolute(location.clone()))?,
        )
    }

    fn refresh_directory(&self, location: &String) -> Result<Directory<String>, Self::Error> {
        self.directory(
            location,
            self.peer.refresh(&Target::Absolute(location.clone()))?,
        )
    }

    fn input_kind(&self, text: &str) -> InputKind {
        match text.starts_with('/') || text.starts_with('~') {
            true => InputKind::Location,
            false => InputKind::Search,
        }
    }

    fn resolve_input(&self, _current: &String, text: &str) -> Result<String, Self::Error> {
        Ok(place::expand_remote(text, self.home))
    }

    fn prefetch(&self, location: &String) {
        self.peer.prefetch(location.clone());
    }
}

impl RemoteSource<'_> {
    fn directory(
        &self,
        requested: &str,
        listing: crate::remote::Listing,
    ) -> Result<Directory<String>, String> {
        if listing.path.is_empty() {
            return Err(format!(
                "the other machine lost the requested path: {requested}"
            ));
        }
        let path = match requested.split('/').any(|component| component == "..") {
            true => listing.path,
            false => requested.to_string(),
        };
        let parent = place::parent_of(&path);
        let parent = (parent != path).then(|| parent.to_string());
        let entries = listing
            .entries
            .into_iter()
            .map(|entry| Entry {
                location: place::join(&path, &entry.name),
                name: entry.name,
                kind: match entry.directory {
                    true => EntryKind::Directory,
                    false => EntryKind::File,
                },
            })
            .collect();
        Ok(Directory {
            label: format!("{}:{}", self.peer.host(), place::shorten(&path, self.home)),
            location: path,
            parent,
            entries,
            status: match listing.missing {
                true => DirectoryStatus::Missing,
                false => DirectoryStatus::Present,
            },
        })
    }
}

struct HcopyView<'a, 'b> {
    browser: &'a Browser<'b>,
}

impl HcopyView<'_, '_> {
    fn chosen(&self, context: &ViewContext<'_, String>) -> Option<String> {
        match self.browser.direction {
            Direction::Push => Some(self.browser.push_destination(&context.directory.location)),
            Direction::Pull => context
                .selection
                .map(|selection| selection.location.clone()),
        }
    }

    fn landing(&self, chosen: &str) -> String {
        match self.browser.direction {
            Direction::Push => self.browser.local_display.clone(),
            Direction::Pull => place::landing(
                chosen,
                &self.browser.remote_home,
                &self.browser.home,
                &self.browser.here,
            )
            .map(|(_, shown)| shown)
            .unwrap_or_else(|_| "(nowhere under this home)".to_string()),
        }
    }
}

impl ExplorerView<String> for HcopyView<'_, '_> {
    fn header(&self, context: &ViewContext<'_, String>) -> Vec<Line> {
        let browser = self.browser;
        let (from_host, to_host) = match browser.direction {
            Direction::Push => (browser.this, browser.peer.host()),
            Direction::Pull => (browser.peer.host(), browser.this),
        };
        let mut heading = vec![
            Span::new(browser.direction.program(), Role::Strong),
            Span::new("   ", Role::Plain),
            Span::new(from_host, Role::Strong),
            Span::new(" → ", Role::Muted),
            Span::new(to_host, Role::Strong),
        ];
        if let Some(route) = browser.route {
            heading.extend([
                Span::new("   ", Role::Plain),
                Span::new(route.name(), Role::Accent),
            ]);
        }

        let chosen = self.chosen(context);
        let local = format!(
            "{}:{}",
            browser.this,
            chosen
                .as_deref()
                .map(|path| self.landing(path))
                .unwrap_or_else(|| "(nothing selected)".to_string())
        );
        let remote = format!(
            "{}:{}",
            browser.peer.host(),
            chosen
                .as_deref()
                .map(|path| place::shorten(path, &browser.remote_home))
                .unwrap_or_else(|| "(nothing selected)".to_string())
        );
        let (from, to) = match browser.direction {
            Direction::Push => (local, remote),
            Direction::Pull => (remote, local),
        };
        vec![
            Line::from_spans(heading),
            Line::default(),
            Line::from_spans([
                Span::new("  from  ", Role::Muted),
                Span::new(from, Role::Accent),
            ]),
            Line::from_spans([
                Span::new("  to    ", Role::Muted),
                Span::new(to, Role::Strong),
            ]),
        ]
    }

    fn badge(&self, _context: &ViewContext<'_, String>, entry: &Entry<String>) -> Option<Line> {
        if Some(&entry.name) != self.browser.name.as_ref() || self.browser.mirror.is_none() {
            return None;
        }
        let label = if self.browser.mirror.as_ref() == Some(&entry.location) {
            "mirror"
        } else if self.browser.direction == Direction::Push {
            "replaces"
        } else {
            return None;
        };
        Some(Line::styled(label, Role::Muted))
    }

    fn accept_label(&self, _context: &ViewContext<'_, String>) -> String {
        match self.browser.direction {
            Direction::Push => "push here".to_string(),
            Direction::Pull => "pull this".to_string(),
        }
    }

    fn state_label(&self, context: &ViewContext<'_, String>, has_matches: bool) -> Option<String> {
        if context.directory.status == DirectoryStatus::Missing {
            return Some(match self.browser.direction {
                Direction::Push => "(not there yet, it will be created)".to_string(),
                Direction::Pull => "(not found)".to_string(),
            });
        }
        ExplorerView::<String>::state_label(&DefaultView, context, has_matches)
    }
}

impl Browser<'_> {
    fn push_destination(&self, directory: &str) -> String {
        if let Some(mirror) = &self.mirror
            && (mirror == directory
                || self
                    .peer
                    .cached(mirror)
                    .is_some_and(|listing| listing.path == directory))
        {
            return mirror.clone();
        }
        match &self.name {
            Some(name) => place::join(directory, name),
            None => directory.to_string(),
        }
    }

    pub fn choose(&self) -> Result<Chosen, String> {
        let source = RemoteSource {
            peer: self.peer,
            home: &self.remote_home,
        };
        let view = HcopyView { browser: self };
        let mut explorer = Explorer::new(source, self.start.clone(), self.style)
            .accept_target(match self.direction {
                Direction::Push => AcceptTarget::CurrentDirectory,
                Direction::Pull => AcceptTarget::HighlightedEntryOrCurrentDirectory,
            })
            .allow_missing_directory(self.direction == Direction::Push)
            .view(view);
        if let Some(mirror) = &self.mirror {
            explorer = explorer.initial_focus(mirror.clone());
        }
        Ok(
            match explorer.run().map_err(|error| match error {
                ExplorerError::Source(error) => error,
                ExplorerError::Terminal(error) => error.to_string(),
            })? {
                Outcome::Selected(selection) => {
                    let path = match self.direction {
                        Direction::Push => self.push_destination(&selection.location),
                        Direction::Pull => selection.location,
                    };
                    Chosen::Picked(path)
                }
                Outcome::Cancelled => Chosen::Cancelled,
                Outcome::Interrupted => Chosen::Interrupted,
                Outcome::Unavailable => Chosen::Unavailable,
            },
        )
    }
}

#[cfg(test)]
#[path = "../tests/unit/browse_tests.rs"]
mod tests;
