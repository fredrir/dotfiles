#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
    SymlinkDirectory,
    Symlink,
    Other,
}

impl EntryKind {
    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory | Self::SymlinkDirectory)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<L> {
    pub location: L,
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectoryStatus {
    Present,
    Missing,
    Unreadable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directory<L> {
    pub location: L,
    pub parent: Option<L>,
    pub label: String,
    pub entries: Vec<Entry<L>>,
    pub status: DirectoryStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Search,
    Location,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptTarget {
    CurrentDirectory,
    HighlightedEntry,
    HighlightedEntryOrCurrentDirectory,
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionPolicy {
    pub accept_target: AcceptTarget,
    pub allow_missing_directory: bool,
    pub selectable: fn(EntryKind) -> bool,
}

impl SelectionPolicy {
    pub fn new(accept_target: AcceptTarget) -> Self {
        Self {
            accept_target,
            allow_missing_directory: false,
            selectable: |_| true,
        }
    }

    pub fn allow_missing_directory(mut self, allow: bool) -> Self {
        self.allow_missing_directory = allow;
        self
    }

    pub fn selectable(mut self, predicate: fn(EntryKind) -> bool) -> Self {
        self.selectable = predicate;
        self
    }

    pub fn permits(self, kind: EntryKind) -> bool {
        (self.selectable)(kind)
    }
}

impl Default for SelectionPolicy {
    fn default() -> Self {
        Self::new(AcceptTarget::HighlightedEntryOrCurrentDirectory)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection<L> {
    pub location: L,
    pub kind: EntryKind,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<L> {
    Selected(Selection<L>),
    Cancelled,
    Interrupted,
    Unavailable,
}
