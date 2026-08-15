//! What the working tree holds that `HEAD` does not.
//!
//! One status walk answers three questions at once — what is staged, what is
//! modified, and what is untracked — and gitoxide runs all three in parallel.
//! Each path is then looked up once in `HEAD`'s tree, which settles both how
//! it is labelled and what discarding it would mean: a path `HEAD` knows can
//! be put back, and a path it does not know cannot.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gix::bstr::{BString, ByteSlice};
use gix::object::tree::EntryKind;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change as WorktreeChange, EntryStatus};

use crate::{Repo, Result, measure};

/// Everything in the working tree that differs from `HEAD`, in path order.
pub struct Survey {
    /// The working tree's root, which every path is relative to.
    pub root: PathBuf,
    pub entries: Vec<Entry>,
}

/// One such path.
pub struct Entry {
    /// Repository-relative and slash-separated, the way git records it.
    pub path: BString,
    /// How it differs, in `git status`'s vocabulary.
    pub label: &'static str,
    pub kind: Kind,
    pub fate: Fate,
    pub counts: Counts,
    /// The `HEAD` side, when `HEAD` has this path; `None` means discarding it
    /// is a deletion.
    pub(crate) head: Option<Source>,
    /// What to read the working-tree side as, which decides whether a symlink
    /// is diffed as the path it points at or followed. `None` for a path with
    /// no content to read, such as a submodule or a device.
    pub(crate) worktree: Option<EntryKind>,
}

/// What the path is, which decides how it is discarded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// In the index or in `HEAD`: the index decides its fate.
    Tracked,
    /// An untracked file, symlink, or anything else with a name.
    File,
    /// An untracked directory, discarded whole.
    Directory,
    /// An untracked directory that is a repository of its own.
    Repository,
}

/// What discarding does to it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Fate {
    /// Tracked, and in `HEAD`: it goes back to what `HEAD` says.
    Restore,
    /// Untracked, or tracked but absent from `HEAD`: it is gone for good.
    Delete,
    /// Reported and then left alone.
    Keep,
}

/// What the diff against `HEAD` amounts to.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Counts {
    /// Nothing to count.
    #[default]
    None,
    /// A directory is counted in files rather than diffed.
    Files(usize),
    /// git will not diff this one.
    Binary,
    /// Lines, as `git diff --numstat` counts them.
    Lines { added: u32, removed: u32 },
}

/// A blob in `HEAD`, and how to read it.
pub(crate) struct Source {
    pub(crate) id: gix::ObjectId,
    pub(crate) kind: EntryKind,
}

impl Entry {
    /// The path as it should be shown: a directory wears its trailing slash,
    /// the way `git status` writes one.
    pub fn shown(&self) -> String {
        let mut shown = visible(&self.path);
        if self.kind == Kind::Directory || self.kind == Kind::Repository {
            shown.push('/');
        }
        shown
    }

    /// The aside after the counts, where there is one.
    pub fn note(&self) -> Option<String> {
        match self.counts {
            Counts::Files(1) => Some("1 file".to_string()),
            Counts::Files(files) => Some(format!("{files} files")),
            Counts::Binary => Some("binary".to_string()),
            Counts::None if self.kind == Kind::Repository => Some("nested repository".to_string()),
            Counts::None | Counts::Lines { .. } => None,
        }
    }
}

impl Survey {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entries that share a fate, in path order.
    pub fn with(&self, fate: Fate) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |entry| entry.fate == fate)
    }

    /// Every line that would be thrown away, added and removed.
    pub fn totals(&self) -> (u32, u32) {
        self.entries
            .iter()
            .fold((0, 0), |(added, removed), entry| match entry.counts {
                Counts::Lines {
                    added: more,
                    removed: fewer,
                } => (added + more, removed + fewer),
                _ => (added, removed),
            })
    }
}

/// What the status walk said about a path before `HEAD` was consulted.
#[derive(Default)]
struct Change {
    conflict: bool,
    added: bool,
    deleted: bool,
    typechange: bool,
    /// The kind recorded in the index, when the walk mentioned it.
    index: Option<EntryKind>,
    /// The kind found on disk, when the walk looked.
    worktree: Option<EntryKind>,
}

impl Change {
    fn label(&self) -> &'static str {
        // The order is `git status`'s: a conflict outranks everything, then
        // what the index says about the path, then what the disk says.
        if self.conflict {
            "unmerged"
        } else if self.added {
            "added"
        } else if self.deleted {
            "deleted"
        } else if self.typechange {
            "typechange"
        } else {
            "modified"
        }
    }
}

impl Repo {
    /// Survey the working tree, limited to `patterns` when there are any.
    ///
    /// Pathspecs are read the way git reads them, relative to the current
    /// directory and with the same magic.
    pub fn survey(&self, patterns: &[BString]) -> Result<Survey> {
        let mut changes: BTreeMap<BString, Change> = BTreeMap::new();
        let mut untracked: BTreeMap<BString, (Kind, Option<EntryKind>)> = BTreeMap::new();

        let mut walk = self
            .git
            .status(gix::progress::Discard)?
            // Renames are a story about where a change went, and a discard
            // ends every story the same way.
            .index_worktree_rewrites(None)
            .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
            // What a submodule's own working tree holds is not ours to throw
            // away, so it is not listed either. A staged change to which
            // commit a submodule sits at is in this index, and still is.
            .index_worktree_submodules(gix::status::Submodule::Given {
                ignore: gix::submodule::config::Ignore::All,
                check_dirty: false,
            })
            .untracked_files(gix::status::UntrackedFiles::Collapsed)
            // An untracked directory arrives as one entry, and discarding it
            // is one `remove_dir_all`: everything inside goes, whether or not
            // the walk ever named it. That is only true to `git clean -d` if
            // the directory holds nothing but untracked files, and gitoxide
            // will only hold back the collapse for that reason when it knows
            // the walk is a deletion. Unset, the collapse is deliberately
            // "more generous in relation to ignored files", and a directory
            // with a `node_modules` or a `.env` in it still folds into the
            // single entry that takes them with it. Told this much, such a
            // directory stays open and the untracked files in it are named
            // one by one, which is what git lists there too.
            .dirwalk_options(|options| {
                options.for_deletion(Some(
                    // Ignored directories are not recursed into, which costs
                    // nothing here: what is inside one is never deleted, so a
                    // repository hiding in there is in no danger either.
                    gix::dir::walk::ForDeletionMode::IgnoredDirectoriesCanHideNestedRepositories,
                ))
            })
            .into_iter(patterns.to_vec())?;

        for item in walk.by_ref() {
            match item? {
                gix::status::Item::TreeIndex(staged) => {
                    let (path, change) = staged_change(staged);
                    let entry = changes.entry(path).or_default();
                    entry.added |= change.added;
                    entry.deleted |= change.deleted;
                    entry.typechange |= change.typechange;
                    entry.index = entry.index.or(change.index);
                }
                gix::status::Item::IndexWorktree(WorktreeItem::Modification {
                    rela_path,
                    entry,
                    status,
                    ..
                }) => {
                    let Some(change) = worktree_change(&status, &entry) else {
                        continue;
                    };
                    let entry = changes.entry(rela_path).or_default();
                    entry.conflict |= change.conflict;
                    entry.deleted |= change.deleted;
                    entry.typechange |= change.typechange;
                    entry.index = entry.index.or(change.index);
                    entry.worktree = entry.worktree.or(change.worktree);
                }
                gix::status::Item::IndexWorktree(WorktreeItem::DirectoryContents {
                    entry, ..
                }) => {
                    if entry.status != gix::dir::entry::Status::Untracked {
                        continue;
                    }
                    untracked.insert(entry.rela_path, found(entry.disk_kind));
                }
                // Both rewrite kinds are switched off above.
                gix::status::Item::IndexWorktree(WorktreeItem::Rewrite { .. }) => {}
            }
        }

        // A path can be staged as deleted and be back on disk under the same
        // name: the restore already covers it, so it is not also an untracked
        // file to delete.
        untracked.retain(|path, _| !changes.contains_key(path));

        let head = self.git.find_tree(self.git.head_tree_id_or_empty()?)?;
        let mut entries = Vec::with_capacity(changes.len() + untracked.len());
        for (path, change) in changes {
            let source = head
                .lookup_entry(path.split_str("/"))?
                .and_then(|entry| source(&entry));
            entries.push(Entry {
                label: change.label(),
                kind: Kind::Tracked,
                fate: if source.is_some() {
                    Fate::Restore
                } else {
                    Fate::Delete
                },
                counts: Counts::None,
                // What is on disk if the walk looked, what the index says if
                // it did not, and what `HEAD` says for a path the index has
                // lost track of.
                worktree: change
                    .worktree
                    .or(change.index)
                    .or(source.as_ref().map(|source| source.kind)),
                head: source,
                path,
            });
        }
        for (path, (kind, worktree)) in untracked {
            entries.push(Entry {
                path,
                label: if kind == Kind::Repository {
                    "repository"
                } else {
                    "untracked"
                },
                kind,
                fate: if kind == Kind::Repository {
                    Fate::Keep
                } else {
                    Fate::Delete
                },
                counts: Counts::None,
                head: None,
                worktree,
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));

        // The walk read the index already; borrowing it saves every counting
        // thread from reading its own copy to find the `.gitattributes` that
        // decide how a file is diffed.
        let outcome = walk
            .into_outcome()
            .ok_or("the status walk did not finish")?;
        let index: &gix::index::File = match &outcome.worktree_index {
            gix::worktree::IndexPersistedOrInMemory::Persisted(shared) => shared,
            gix::worktree::IndexPersistedOrInMemory::InMemory(index) => index,
        };
        measure::lines(self, index, &mut entries)?;

        Ok(Survey {
            root: self.root().to_path_buf(),
            entries,
        })
    }
}

/// What a difference between `HEAD`'s tree and the index says about a path.
fn staged_change(staged: gix::diff::index::Change) -> (BString, Change) {
    use gix::diff::index::ChangeRef;
    let mut change = Change::default();
    let path = match staged {
        ChangeRef::Addition {
            location,
            entry_mode,
            ..
        } => {
            change.added = true;
            change.index = kind(entry_mode);
            location
        }
        ChangeRef::Deletion { location, .. } => {
            change.deleted = true;
            location
        }
        ChangeRef::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        }
        | ChangeRef::Rewrite {
            source_entry_mode: previous_entry_mode,
            location,
            entry_mode,
            ..
        } => {
            change.typechange = family(previous_entry_mode) != family(entry_mode);
            change.index = kind(entry_mode);
            location
        }
    };
    (path.into_owned(), change)
}

/// What a difference between the index and the working tree says about a path.
fn worktree_change(
    status: &EntryStatus<(), gix::submodule::Status>,
    entry: &gix::index::Entry,
) -> Option<Change> {
    let mut change = Change {
        index: kind(entry.mode),
        ..Change::default()
    };
    match status {
        EntryStatus::Conflict { .. } => change.conflict = true,
        EntryStatus::Change(WorktreeChange::Removed) => change.deleted = true,
        EntryStatus::Change(WorktreeChange::Type { worktree_mode }) => {
            change.typechange = true;
            change.worktree = kind(*worktree_mode);
        }
        EntryStatus::Change(
            WorktreeChange::Modification { .. } | WorktreeChange::SubmoduleModification(_),
        ) => {}
        // A path only promised to the index is an addition against `HEAD`,
        // which the staged side of the walk reports; and a stat that could be
        // refreshed is not a change at all.
        EntryStatus::IntentToAdd | EntryStatus::NeedsUpdate(_) => return None,
    }
    Some(change)
}

/// What the directory walk found, as something to discard.
fn found(disk: Option<gix::dir::entry::Kind>) -> (Kind, Option<EntryKind>) {
    match disk {
        Some(gix::dir::entry::Kind::Repository) => (Kind::Repository, None),
        Some(gix::dir::entry::Kind::Directory) => (Kind::Directory, None),
        Some(gix::dir::entry::Kind::Symlink) => (Kind::File, Some(EntryKind::Link)),
        Some(gix::dir::entry::Kind::File) => (Kind::File, Some(EntryKind::Blob)),
        // A socket or a device is a name to remove, not content to read.
        Some(gix::dir::entry::Kind::Untrackable) | None => (Kind::File, None),
    }
}

/// The blob a tree entry points at, or nothing for a directory, which is not
/// a path that can be put back on its own.
fn source(entry: &gix::object::tree::Entry<'_>) -> Option<Source> {
    match entry.mode().kind() {
        kind @ (EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link) => Some(Source {
            id: entry.object_id(),
            kind,
        }),
        EntryKind::Commit | EntryKind::Tree => None,
    }
}

/// The kind of content behind an index mode, where there is content to read.
fn kind(mode: gix::index::entry::Mode) -> Option<EntryKind> {
    match mode.bits() {
        0o100644 => Some(EntryKind::Blob),
        0o100755 => Some(EntryKind::BlobExecutable),
        0o120000 => Some(EntryKind::Link),
        _ => None,
    }
}

/// The file type an index mode records, ignoring permissions: gaining or
/// losing the executable bit is a modification, not a change of kind.
fn family(mode: gix::index::entry::Mode) -> u32 {
    mode.bits() & 0o170000
}

/// A path with anything unprintable spelled out, so that a crafted file name
/// cannot forge a line of the plan.
fn visible(path: &BString) -> String {
    let mut shown = String::with_capacity(path.len());
    for character in path.to_str_lossy().chars() {
        match character {
            '\x00'..='\x1f' => {
                shown.push('^');
                shown.push((character as u8 + b'@') as char);
            }
            '\x7f' => shown.push_str("^?"),
            _ => shown.push(character),
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, counts: Counts) -> Entry {
        Entry {
            path: "some/path".into(),
            label: "untracked",
            kind,
            fate: Fate::Delete,
            counts,
            head: None,
            worktree: Some(EntryKind::Blob),
        }
    }

    #[test]
    fn a_conflict_outranks_the_rest() {
        let change = Change {
            conflict: true,
            deleted: true,
            ..Change::default()
        };
        assert_eq!(change.label(), "unmerged");
    }

    #[test]
    fn the_index_is_asked_before_the_disk() {
        let change = Change {
            added: true,
            typechange: true,
            ..Change::default()
        };
        assert_eq!(change.label(), "added");
        let change = Change {
            deleted: true,
            typechange: true,
            ..Change::default()
        };
        assert_eq!(change.label(), "deleted");
    }

    #[test]
    fn an_executable_bit_is_not_a_change_of_kind() {
        let file = gix::index::entry::Mode::FILE;
        let executable = gix::index::entry::Mode::FILE_EXECUTABLE;
        let link = gix::index::entry::Mode::SYMLINK;
        assert_eq!(family(file), family(executable));
        assert_ne!(family(file), family(link));
    }

    #[test]
    fn directories_are_shown_with_their_slash() {
        assert_eq!(
            entry(Kind::Directory, Counts::Files(2)).shown(),
            "some/path/"
        );
        assert_eq!(entry(Kind::File, Counts::None).shown(), "some/path");
    }

    #[test]
    fn notes_say_what_the_counts_cannot() {
        assert_eq!(
            entry(Kind::Directory, Counts::Files(1)).note().as_deref(),
            Some("1 file")
        );
        assert_eq!(
            entry(Kind::Directory, Counts::Files(9)).note().as_deref(),
            Some("9 files")
        );
        assert_eq!(
            entry(Kind::Repository, Counts::None).note().as_deref(),
            Some("nested repository")
        );
        assert_eq!(
            entry(
                Kind::File,
                Counts::Lines {
                    added: 1,
                    removed: 0
                }
            )
            .note()
            .as_deref(),
            None
        );
    }

    #[test]
    fn unprintable_names_cannot_forge_a_line() {
        assert_eq!(visible(&"plain".into()), "plain");
        assert_eq!(visible(&"two\nlines".into()), "two^Jlines");
    }

    #[test]
    fn totals_add_up_every_line_not_only_the_shown_ones() {
        let survey = Survey {
            root: PathBuf::new(),
            entries: vec![
                entry(
                    Kind::File,
                    Counts::Lines {
                        added: 3,
                        removed: 1,
                    },
                ),
                entry(Kind::File, Counts::Binary),
                entry(
                    Kind::File,
                    Counts::Lines {
                        added: 4,
                        removed: 0,
                    },
                ),
            ],
        };
        assert_eq!(survey.totals(), (7, 1));
    }
}
