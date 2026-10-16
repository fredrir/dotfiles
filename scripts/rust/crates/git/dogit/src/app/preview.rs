use super::*;
use std::collections::VecDeque;

const CACHE_ENTRIES: usize = 16;
const CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Cache {
    entries: VecDeque<(Arc<Content>, usize)>,
    bytes: usize,
}

impl Cache {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub(super) fn get(&mut self, key: &PreviewKey) -> Option<Arc<Content>> {
        let index = self
            .entries
            .iter()
            .position(|(entry, _)| entry.key == *key)?;
        let entry = self.entries.remove(index)?;
        let content = Arc::clone(&entry.0);
        self.entries.push_back(entry);
        Some(content)
    }

    pub(super) fn insert(&mut self, content: Arc<Content>) {
        let bytes = content.diff.as_ref().map_or(0, |diff| diff.cache_bytes())
            + content.presentation.as_ref().map_or(0, |presentation| {
                presentation
                    .lines
                    .values()
                    .map(|line| line.len() + 64)
                    .sum::<usize>()
            })
            + content.text.iter().map(String::len).sum::<usize>()
            + content.structural.as_ref().map_or(0, String::len)
            + content.paths.as_ref().map_or(0, |paths| {
                paths
                    .iter()
                    .map(|path| path.as_os_str().len() + 32)
                    .sum::<usize>()
            });
        if bytes > CACHE_BYTES {
            return;
        }
        while self.entries.len() >= CACHE_ENTRIES || self.bytes + bytes > CACHE_BYTES {
            if let Some((_, size)) = self.entries.pop_front() {
                self.bytes -= size;
            }
        }
        self.bytes += bytes;
        self.entries.push_back((content, bytes));
    }
}

impl App {
    pub(super) fn preview_target(&self) -> Option<&Target> {
        self.loaded_preview
            .as_ref()
            .map_or_else(|| self.selected(), |key| key.request.target.as_ref())
    }

    pub(super) fn preview_commit(&self) -> Option<&crate::commit::CommitDetails> {
        let request = &self.loaded_preview.as_ref()?.request;
        if request.workspace != Workspace::History {
            return None;
        }
        let oid = request.history_commit.as_deref().or({
            if let Some(Target::Commit(oid)) = &request.target {
                Some(oid.as_str())
            } else {
                None
            }
        })?;
        self.history.get(oid)
    }
}
