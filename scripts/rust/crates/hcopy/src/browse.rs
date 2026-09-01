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
mod tests {
    use super::*;

    fn source<'a>(peer: &'a Peer) -> RemoteSource<'a> {
        RemoteSource {
            peer,
            home: "/home/fredrir",
        }
    }

    fn browser<'a>(direction: Direction, peer: &'a Peer, style: &'a Style) -> Browser<'a> {
        Browser {
            direction,
            peer,
            style,
            this: "macie",
            route: Some(Route::Cable),
            name: Some("my-app".into()),
            local_display: "~/projects/my-app".into(),
            start: "/home/fredrir/projects".into(),
            mirror: Some("/home/fredrir/projects/my-app".into()),
            remote_home: "/home/fredrir".into(),
            home: PathBuf::from("/Users/fredrir"),
            here: PathBuf::from("/Users/fredrir/projects"),
        }
    }

    fn directory(entries: Vec<Entry<String>>, status: DirectoryStatus) -> Directory<String> {
        Directory {
            location: "/home/fredrir/projects".into(),
            parent: Some("/home/fredrir".into()),
            label: "archie:~/projects".into(),
            entries,
            status,
        }
    }

    fn context<'a>(
        directory: &'a Directory<String>,
        selection: Option<&'a file_explorer::Selection<String>>,
    ) -> ViewContext<'a, String> {
        ViewContext {
            directory,
            focused: directory.entries.first(),
            selection,
            prompt: None,
            error: None,
        }
    }

    #[test]
    fn remote_listing_maps_to_opaque_full_locations() {
        let peer = Peer::new("archie");
        let directory = source(&peer)
            .directory(
                "/home/fredrir/projects",
                crate::remote::Listing {
                    path: "/home/fredrir/projects".into(),
                    home: "/home/fredrir".into(),
                    entries: vec![crate::remote::Entry {
                        name: "my-app".into(),
                        directory: true,
                    }],
                    missing: false,
                },
            )
            .unwrap();

        assert_eq!(directory.location, "/home/fredrir/projects");
        assert_eq!(directory.parent.as_deref(), Some("/home/fredrir"));
        assert_eq!(directory.label, "archie:~/projects");
        assert_eq!(
            directory.entries[0].location,
            "/home/fredrir/projects/my-app"
        );
        assert_eq!(directory.entries[0].kind, EntryKind::Directory);
    }

    #[test]
    fn remote_alias_identity_survives_canonical_listings_and_missing_refreshes() {
        let peer = Peer::new("archie");
        let source = source(&peer);
        let requested = "/home/fredrir/dotfiles";
        let present = source
            .directory(
                requested,
                crate::remote::Listing {
                    path: "/srv/homes/fredrir/dotfiles".into(),
                    home: "/home/fredrir".into(),
                    entries: vec![crate::remote::Entry {
                        name: "scripts".into(),
                        directory: true,
                    }],
                    missing: false,
                },
            )
            .unwrap();
        assert_eq!(present.location, requested);
        assert_eq!(present.parent.as_deref(), Some("/home/fredrir"));
        assert_eq!(
            present.entries[0].location,
            "/home/fredrir/dotfiles/scripts"
        );

        let missing = source
            .directory(
                requested,
                crate::remote::Listing {
                    path: requested.into(),
                    home: "/home/fredrir".into(),
                    entries: Vec::new(),
                    missing: true,
                },
            )
            .unwrap();
        assert_eq!(missing.location, requested);
        assert_eq!(missing.status, DirectoryStatus::Missing);

        let resolved = source
            .directory(
                "/home/fredrir/projects/../dotfiles",
                crate::remote::Listing {
                    path: "/home/fredrir/dotfiles".into(),
                    home: "/home/fredrir".into(),
                    entries: Vec::new(),
                    missing: false,
                },
            )
            .unwrap();
        assert_eq!(resolved.location, "/home/fredrir/dotfiles");
    }

    #[test]
    fn a_missing_listing_never_loses_the_requested_destination() {
        let peer = Peer::new("archie");
        let directory = source(&peer)
            .directory(
                "/home/fredrir/new/place",
                crate::remote::Listing {
                    path: "/home/fredrir/new/place".into(),
                    home: "/home/fredrir".into(),
                    entries: Vec::new(),
                    missing: true,
                },
            )
            .unwrap();

        assert_eq!(directory.location, "/home/fredrir/new/place");
        assert_eq!(directory.status, DirectoryStatus::Missing);
    }

    #[test]
    fn hcopy_view_keeps_push_and_pull_selection_semantics() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let item = Entry {
            location: "/home/fredrir/projects/notes.md".into(),
            name: "notes.md".into(),
            kind: EntryKind::File,
        };
        let directory = directory(vec![item.clone()], DirectoryStatus::Present);
        let selection = file_explorer::Selection {
            location: item.location,
            kind: item.kind,
            label: item.name,
        };

        let push = browser(Direction::Push, &peer, &style);
        let pull = browser(Direction::Pull, &peer, &style);
        assert_eq!(
            HcopyView { browser: &push }.chosen(&context(&directory, Some(&selection))),
            Some("/home/fredrir/projects/my-app".to_string())
        );
        assert_eq!(
            HcopyView { browser: &pull }.chosen(&context(&directory, Some(&selection))),
            Some("/home/fredrir/projects/notes.md".to_string())
        );
        assert_eq!(
            HcopyView { browser: &pull }.chosen(&context(&directory, None)),
            None
        );
        let mirror = Directory {
            location: "/home/fredrir/projects/my-app".to_string(),
            parent: Some("/home/fredrir/projects".to_string()),
            label: "archie:~/projects/my-app".to_string(),
            entries: Vec::new(),
            status: DirectoryStatus::Present,
        };
        assert_eq!(
            HcopyView { browser: &push }.chosen(&context(&mirror, None)),
            Some("/home/fredrir/projects/my-app".to_string())
        );
    }

    #[test]
    fn missing_state_wording_is_direction_specific() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let directory = directory(Vec::new(), DirectoryStatus::Missing);
        let context = context(&directory, None);
        let push = browser(Direction::Push, &peer, &style);
        let pull = browser(Direction::Pull, &peer, &style);
        assert_eq!(
            HcopyView { browser: &push }.state_label(&context, false),
            Some("(not there yet, it will be created)".into())
        );
        assert_eq!(
            HcopyView { browser: &pull }.state_label(&context, false),
            Some("(not found)".into())
        );
    }

    #[test]
    fn mirror_and_replacement_badges_compare_full_locations() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Push, &peer, &style);
        let view = HcopyView { browser: &browser };
        let directory = directory(Vec::new(), DirectoryStatus::Present);
        let context = context(&directory, None);
        let mirror = Entry {
            location: "/home/fredrir/projects/my-app".into(),
            name: "my-app".into(),
            kind: EntryKind::Directory,
        };
        let elsewhere = Entry {
            location: "/home/fredrir/scratch/my-app".into(),
            ..mirror.clone()
        };
        assert_eq!(
            view.badge(&context, &mirror),
            Some(Line::styled("mirror", Role::Muted))
        );
        assert_eq!(
            view.badge(&context, &elsewhere),
            Some(Line::styled("replaces", Role::Muted))
        );
    }
}
