
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use gix::bstr::BStr;
use gix::index::entry::{Flags, Mode, Stat};
use gix::object::tree::EntryKind;

use crate::measure::is_repository;
use crate::survey::{Fate, Kind, Survey};
use crate::{Repo, Result};

impl Repo {
    pub fn discard(&self, survey: &Survey) -> Result<()> {
        for entry in survey.with(Fate::Delete) {
            let path = crate::on_disk(&survey.root, entry.path.as_ref());
            remove(&path)?;
            if entry.kind == Kind::Tracked {
                // git takes the directories a removed file leaves empty with
                // it; an untracked directory is an entry of its own instead.
                prune(&survey.root, &path);
            }
        }

        // The entries to put back, as an index of their own: it is what the
        // checkout reads, and afterwards it holds the stat of every file that
        // was written, which is what the real index needs to record.
        let mut wanted = gix::index::State::new(self.git.object_hash());
        for entry in survey.with(Fate::Restore) {
            let Some(source) = &entry.head else { continue };
            wanted.dangerously_push_entry(
                Stat::default(),
                source.id,
                Flags::empty(),
                mode(source.kind),
                entry.path.as_ref(),
            );
        }
        wanted.sort_entries();

        if !wanted.entries().is_empty() {
            let mut options = self.git.checkout_options(
                gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            )?;
            // Every one of these paths has something in its way already —
            // that is why it is being restored.
            options.destination_is_initially_empty = false;
            options.overwrite_existing = true;
            options.keep_going = false;
            gix::worktree::state::checkout(
                &mut wanted,
                &survey.root,
                self.git.objects.clone().into_arc()?,
                &gix::progress::Discard,
                &gix::progress::Discard,
                &AtomicBool::default(),
                options,
            )?;
        }

        let tracked: HashSet<&BStr> = survey
            .entries
            .iter()
            .filter(|entry| entry.kind == Kind::Tracked)
            .map(|entry| entry.path.as_ref())
            .collect();
        if !tracked.is_empty() {
            let shared = self.git.index_or_empty()?;
            let mut index = gix::index::File::clone(&shared);
            // Out with every stage of every path in the plan — which drops the
            // conflicts and the staged additions — and in with what `HEAD`
            // holds for the ones that have something there.
            index.remove_entries(|_, path, _| tracked.contains(path));
            for entry in wanted.entries() {
                index.dangerously_push_entry(
                    entry.stat,
                    entry.id,
                    entry.flags,
                    entry.mode,
                    entry.path(&wanted),
                );
            }
            index.sort_entries();
            // The cached tree ids describe the entries that were just
            // replaced; left in place they would be written back as valid and
            // a later commit would believe them.
            index.remove_tree();
            index.write(gix::index::write::Options::default())?;
        }

        Ok(())
    }
}

fn mode(kind: EntryKind) -> Mode {
    match kind {
        EntryKind::Blob => Mode::FILE,
        EntryKind::BlobExecutable => Mode::FILE_EXECUTABLE,
        EntryKind::Link => Mode::SYMLINK,
        EntryKind::Commit => Mode::COMMIT,
        EntryKind::Tree => Mode::DIR,
    }
}

fn remove(path: &Path) -> Result<()> {
    let Ok(found) = fs::symlink_metadata(path) else {
        return Ok(());
    };
    if found.is_dir() {
        remove_directory(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn remove_directory(path: &Path) -> Result<bool> {
    if is_repository(path) {
        return Ok(false);
    }
    let mut emptied = true;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        // From the directory listing, so a symlink to a directory is a
        // symlink here and is unlinked rather than followed.
        if entry.file_type()?.is_dir() {
            emptied &= remove_directory(&entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    if emptied {
        fs::remove_dir(path)?;
    }
    Ok(emptied)
}

fn prune(root: &Path, path: &Path) {
    let mut directory = path.parent();
    while let Some(empty) = directory {
        if empty == root || fs::remove_dir(empty).is_err() {
            return;
        }
        directory = empty.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_goes_with_everything_in_it() {
        let root = tempfile::tempdir().unwrap();
        let tree = root.path().join("tree");
        fs::create_dir_all(tree.join("deep")).unwrap();
        fs::write(tree.join("deep/file"), "x").unwrap();
        assert!(remove_directory(&tree).unwrap());
        assert!(!tree.exists());
    }

    #[test]
    fn a_nested_repository_survives_and_keeps_its_parents() {
        let root = tempfile::tempdir().unwrap();
        let tree = root.path().join("tree");
        fs::create_dir_all(tree.join("nested/.git")).unwrap();
        fs::write(tree.join("nested/kept"), "x").unwrap();
        fs::write(tree.join("gone"), "x").unwrap();
        assert!(!remove_directory(&tree).unwrap());
        assert!(tree.join("nested/kept").exists());
        assert!(!tree.join("gone").exists());
    }

    #[test]
    fn pruning_stops_at_the_root_and_at_anything_left() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let file = root.join("a/b/file");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(root.join("a/kept"), "x").unwrap();
        prune(root, &file);
        assert!(!root.join("a/b").exists());
        assert!(root.join("a/kept").exists());
        assert!(root.exists());
    }
}
