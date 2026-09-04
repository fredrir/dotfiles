use std::fs;
use std::path::Path;

use gix::diff::blob::{ResourceKind, pipeline, platform::prepare_diff};
use gix::object::tree::EntryKind;
use rayon::prelude::*;

use crate::survey::{Counts, Entry, Kind};
use crate::{Repo, Result};

pub(crate) fn lines(repo: &Repo, index: &gix::index::File, entries: &mut [Entry]) -> Result<()> {
    let repos = repo.git.clone().into_sync();
    let root = repo.root();
    let measured: Vec<Result<Counts>> = entries
        .par_iter()
        .map_init(
            || Scales::new(&repos, index, root),
            |scales, entry| match scales {
                Ok(scales) => scales.of(entry, root),
                // The failure belongs to the thread, not to this entry, so it
                // is restated rather than moved out of the shared slot.
                Err(error) => Err(error.to_string().into()),
            },
        )
        .collect();
    for (entry, counts) in entries.iter_mut().zip(measured) {
        entry.counts = counts?;
    }
    Ok(())
}

struct Scales {
    repo: gix::Repository,
    cache: gix::diff::blob::Platform,
}

impl Scales {
    fn new(
        repos: &gix::ThreadSafeRepository,
        index: &gix::index::File,
        root: &Path,
    ) -> Result<Scales> {
        let repo = repos.to_thread_local();
        let attributes = repo
            .attributes_only(
                index,
                gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            )?
            .detach();
        let cache = gix::diff::resource_cache(
            &repo,
            pipeline::Mode::ToGit,
            attributes,
            pipeline::WorktreeRoots {
                old_root: None,
                new_root: Some(root.to_owned()),
            },
        )?;
        Ok(Scales { repo, cache })
    }

    fn of(&mut self, entry: &Entry, root: &Path) -> Result<Counts> {
        match entry.kind {
            Kind::Repository => Ok(Counts::None),
            Kind::Directory => Ok(Counts::Files(files_in(&crate::on_disk(
                root,
                entry.path.as_ref(),
            )))),
            Kind::Tracked | Kind::File => self.diff(entry),
        }
    }

    fn diff(&mut self, entry: &Entry) -> Result<Counts> {
        // A submodule, a device, or anything else without content to read is
        // reported without a number rather than opened to find out.
        let Some(worktree) = entry.worktree else {
            return Ok(Counts::None);
        };
        let nothing = self.repo.object_hash().null();
        let (head, kind) = match &entry.head {
            Some(source) => (source.id, source.kind),
            None => (nothing, EntryKind::Blob),
        };
        self.cache.clear_resource_cache_keep_allocation();
        self.cache.set_resource(
            head,
            kind,
            entry.path.as_ref(),
            ResourceKind::OldOrSource,
            &self.repo.objects,
        )?;
        // A null id with a working-tree root set means "read it from disk",
        // and a path that is not there reads as nothing at all. A path that is
        // there but is not readable content — a tracked file whose name a
        // directory has taken, say — is counted from the `HEAD` side alone,
        // since all of that side is what is going away.
        if self
            .cache
            .set_resource(
                nothing,
                worktree,
                entry.path.as_ref(),
                ResourceKind::NewOrDestination,
                &self.repo.objects,
            )
            .is_err()
        {
            return self.head_only(entry);
        }
        let prepared = match self.cache.prepare_diff() {
            Ok(prepared) => prepared,
            // Neither side exists: a file that was staged and then deleted
            // has nothing left to count.
            Err(prepare_diff::Error::SourceAndDestinationRemoved) => {
                return Ok(Counts::Lines {
                    added: 0,
                    removed: 0,
                });
            }
            Err(error) => return Err(error.into()),
        };
        match prepared.operation {
            prepare_diff::Operation::InternalDiff { algorithm } => {
                let input = prepared.interned_input();
                let diff = gix::diff::blob::Diff::compute(algorithm, &input);
                Ok(Counts::Lines {
                    added: diff.count_additions(),
                    removed: diff.count_removals(),
                })
            }
            // Binary content, or content only an external program can read:
            // either way there are no lines to count.
            prepare_diff::Operation::ExternalCommand { .. }
            | prepare_diff::Operation::SourceOrDestinationIsBinary => Ok(Counts::Binary),
        }
    }

    fn head_only(&self, entry: &Entry) -> Result<Counts> {
        let Some(source) = &entry.head else {
            return Ok(Counts::Lines {
                added: 0,
                removed: 0,
            });
        };
        let blob = self.repo.find_object(source.id)?;
        if is_binary(&blob.data) {
            return Ok(Counts::Binary);
        }
        Ok(Counts::Lines {
            added: 0,
            removed: count_lines(&blob.data),
        })
    }
}

fn is_binary(content: &[u8]) -> bool {
    const PROBE: usize = 8000;
    content.iter().take(PROBE).any(|byte| *byte == 0)
}

fn count_lines(content: &[u8]) -> u32 {
    if content.is_empty() {
        return 0;
    }
    let breaks = content.iter().filter(|byte| **byte == b'\n').count();
    let unterminated = usize::from(!content.ends_with(b"\n"));
    u32::try_from(breaks + unterminated).unwrap_or(u32::MAX)
}

fn files_in(directory: &Path) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut found = 0;
    for entry in entries.flatten() {
        // The kind comes from the directory listing, which describes a
        // symlink itself rather than what it points at.
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                let path = entry.path();
                if !is_repository(&path) {
                    found += files_in(&path);
                }
            }
            Ok(_) => found += 1,
            Err(_) => {}
        }
    }
    found
}

pub(crate) fn is_repository(directory: &Path) -> bool {
    directory.join(".git").exists()
}

#[cfg(test)]
#[path = "../tests/unit/measure_tests.rs"]
mod tests;
