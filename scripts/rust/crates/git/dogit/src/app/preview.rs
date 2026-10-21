use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

const CACHE_ENTRIES: usize = 16;
const CACHE_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct Loaded {
    pub content: Arc<Content>,
    pub rows: Rows,
}

impl Loaded {
    pub(super) fn new(
        content: Arc<Content>,
        width: u16,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let config = content
            .presentation
            .as_ref()
            .map_or(&content.key.request.presentation, |value| &value.config);
        let rows = Rows::new(
            content.diff.as_deref(),
            content.presentation.as_deref(),
            width,
            content.key.request.presentation.split,
            config,
            Some(cancel),
        )?;
        Ok(Self { content, rows })
    }
}

pub(super) struct Rows {
    layout: Option<view::DiffLayout>,
    width: u16,
    split: bool,
    config: DiffPresentationConfig,
    horizontal_limit: usize,
    changed_lines: BTreeSet<usize>,
    counts: (usize, usize),
}

impl Rows {
    pub(super) fn new(
        diff: Option<&ExactDiff>,
        presentation: Option<&DiffPresentation>,
        width: u16,
        split: bool,
        config: &DiffPresentationConfig,
        cancel: Option<&AtomicBool>,
    ) -> Result<Self, String> {
        let split = split && width >= 72;
        let layout = diff
            .map(|diff| {
                view::configured_diff_layout_cancellable(
                    diff,
                    width,
                    split,
                    presentation,
                    config,
                    cancel,
                )
            })
            .transpose()?;
        let horizontal_limit = diff.map_or(0, |diff| {
            view::configured_diff_horizontal_limit(diff, width, split, config)
        });
        let mut counts = (0, 0);
        let mut changed_lines = BTreeSet::new();
        if let Some(diff) = diff {
            for line in diff.hunks.iter().flat_map(|hunk| &hunk.lines) {
                if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
                    return Err("cancelled".into());
                }
                match line.kind {
                    LineKind::Addition => counts.0 += 1,
                    LineKind::Deletion => counts.1 += 1,
                    LineKind::Context => continue,
                }
                changed_lines.insert(line.id);
            }
        }
        Ok(Self {
            layout,
            width,
            split,
            config: config.clone(),
            horizontal_limit,
            changed_lines,
            counts,
        })
    }
}

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
            + content
                .image
                .as_ref()
                .map_or(0, |image| image.cache_bytes())
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

#[derive(Default)]
pub(super) struct ImageState {
    source: Option<Arc<crate::image_preview::Preview>>,
    worker: Worker<Result<ratatui_image::protocol::Protocol, String>>,
    picker: Option<ratatui_image::picker::Picker>,
    area: Option<ratatui::layout::Rect>,
    protocol: Option<ratatui_image::protocol::Protocol>,
    error: Option<String>,
}

impl ImageState {
    pub(super) fn set_waker(&mut self, wake: ui_terminal::InputWake) {
        self.worker.set_waker(wake);
    }

    pub(super) fn is_some(&self) -> bool {
        self.source.is_some()
    }

    pub(super) fn set(&mut self, source: Option<Arc<crate::image_preview::Preview>>) {
        if let (Some(old), Some(new)) = (&self.source, &source)
            && Arc::ptr_eq(old, new)
        {
            return;
        }
        self.worker.invalidate();
        self.source = source;
        self.area = None;
        self.protocol = None;
        self.error = None;
    }

    pub(super) fn collect(&mut self) -> bool {
        let Some(result) = self.worker.take() else {
            return false;
        };
        match result {
            Ok(protocol) => self.protocol = Some(protocol),
            Err(error) => self.error = Some(error),
        }
        true
    }

    pub(super) fn resize(&mut self, canvas: ratatui::layout::Rect) {
        let Some(source) = self.source.as_ref().filter(|source| source.image.is_ok()) else {
            return;
        };
        let area = view::image_area(canvas);
        let size = ratatui::layout::Rect::new(0, 0, area.width, area.height);
        if size.is_empty() || self.area == Some(size) {
            return;
        }
        self.area = Some(size);
        self.error = None;
        let source = Arc::clone(source);
        let picker = self
            .picker
            .get_or_insert_with(crate::commit::avatar::picker)
            .clone();
        self.worker.submit(move |_| {
            let image = source.image.as_ref().map_err(Clone::clone)?.clone();
            picker
                .new_protocol(
                    image,
                    size,
                    ratatui_image::Resize::Fit(Some(image::imageops::FilterType::Triangle)),
                )
                .map_err(|error| format!("Cannot display image: {error}"))
        });
    }

    pub(super) fn view(&self) -> Option<view::ImageView<'_>> {
        let source = self.source.as_ref()?;
        Some(view::ImageView {
            label: &source.label,
            protocol: self.protocol.as_ref(),
            error: self
                .error
                .as_deref()
                .or_else(|| source.image.as_ref().err().map(String::as_str)),
        })
    }
}

impl App {
    pub(super) fn apply_rows(&mut self, rows: Rows) {
        let cursor = self.row_anchor(self.view.diff_cursor);
        let viewport = self.row_anchor(self.view.diff_offset);
        self.row_width = rows.width;
        self.row_split = rows.split;
        if let Some(layout) = rows.layout {
            self.row_map = layout.rows;
            self.row_paint = Some(layout.paint);
            self.view.horizontal = self.view.horizontal.min(rows.horizontal_limit);
        } else {
            self.row_map.clear();
            self.row_paint = None;
        }
        self.row_config = Some(rows.config);
        self.changed_lines = rows.changed_lines;
        self.diff_counts = rows.counts;
        if let Some(index) = cursor.and_then(|anchor| self.row_for_anchor(anchor)) {
            self.view.diff_cursor = index;
        }
        if let Some(index) = viewport.and_then(|anchor| self.row_for_anchor(anchor)) {
            self.view.diff_offset = index;
        }
    }

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
