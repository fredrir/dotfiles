use std::collections::BTreeMap;

use super::*;
use crate::presentation::{DiffPresentation, DiffPresentationConfig, WrapPolicy};

#[derive(Clone, Debug)]
pub struct DiffPaint {
    pub width: u16,
    pub split: bool,
    pub(super) horizontal: bool,
    lines: BTreeMap<(usize, bool), PaintedLine>,
}

#[derive(Clone, Debug)]
struct PaintedLine {
    gutter: Line<'static>,
    background: Option<Color>,
    chunks: Vec<Line<'static>>,
    offsets: Vec<usize>,
}

pub struct DiffLayout {
    pub rows: Vec<DiffRow>,
    pub paint: DiffPaint,
}

pub fn configured_diff_layout(
    diff: &ExactDiff,
    width: u16,
    split: bool,
    presentation: Option<&DiffPresentation>,
    config: &DiffPresentationConfig,
) -> DiffLayout {
    let split = split && width >= 72;
    let mut paint = DiffPaint {
        width,
        split,
        horizontal: config.split_wrap == WrapPolicy::Disabled,
        lines: BTreeMap::new(),
    };
    let aligned = diff_rows(diff, split);
    if !split {
        return DiffLayout {
            rows: aligned,
            paint,
        };
    }
    let half = width.saturating_sub(1) / 2;
    for line in diff.hunks.iter().flat_map(|hunk| &hunk.lines) {
        let content = diff::content_line(line, presentation, config);
        for (right, side_width) in [(false, half), (true, width - half - 1)] {
            if (right && line.kind == LineKind::Deletion)
                || (!right && line.kind == LineKind::Addition)
            {
                continue;
            }
            let gutter = diff::number_gutter(line, Some(right), config);
            let content_width = usize::from(side_width).saturating_sub(gutter.width() + 3);
            let (chunks, offsets) = wrapped_chunks(&content, content_width, config.split_wrap);
            paint.lines.insert(
                (line.id, right),
                PaintedLine {
                    gutter,
                    background: content.spans.first().and_then(|span| span.style.bg),
                    chunks,
                    offsets,
                },
            );
        }
    }
    let mut rows = Vec::with_capacity(aligned.len());
    for row in aligned {
        if row.line.is_none() && row.right_line.is_none() {
            rows.push(row);
            continue;
        }
        let left = row.line.and_then(|id| paint.lines.get(&(id, false)));
        let right = row.right_line.and_then(|id| paint.lines.get(&(id, true)));
        let count = left
            .map_or(0, |line| line.chunks.len())
            .max(right.map_or(0, |line| line.chunks.len()));
        for part in 0..count {
            let left_offset = left.and_then(|line| line.offsets.get(part));
            let right_offset = right.and_then(|line| line.offsets.get(part));
            rows.push(DiffRow {
                hunk: row.hunk,
                hunk_end: false,
                line: left_offset.and(row.line),
                right_line: right_offset.and(row.right_line),
                left_offset: left_offset.copied().unwrap_or(0),
                right_offset: right_offset.copied().unwrap_or(0),
                left_part: part,
                right_part: part,
            });
        }
    }
    DiffLayout { rows, paint }
}

fn wrapped_chunks(
    content: &Line<'static>,
    width: usize,
    policy: WrapPolicy,
) -> (Vec<Line<'static>>, Vec<usize>) {
    if policy == WrapPolicy::Disabled {
        return (vec![content.clone()], vec![0]);
    }
    if width == 0 {
        return (vec![Line::default()], vec![0]);
    }
    let mut chunks = vec![Line::default()];
    let mut offsets = vec![0];
    let mut cells = 0;
    let mut source_offset = 0;
    text::visit_graphemes(content, |symbol, appearance| {
        let length = symbol.width();
        if cells > 0 && cells + length > width {
            if policy
                .row_limit()
                .is_some_and(|limit| chunks.len() >= limit)
            {
                if let Some(last) = chunks.last_mut() {
                    *last = truncated(last, width);
                }
                return false;
            }
            chunks.push(Line::default());
            offsets.push(source_offset);
            cells = 0;
        }
        if let Some(last) = chunks.last_mut() {
            text::push_styled(last, symbol, appearance);
        }
        cells += length;
        source_offset += length;
        true
    });
    (chunks, offsets)
}

fn truncated(line: &Line<'_>, width: usize) -> Line<'static> {
    let mut result = Line::default();
    let mut cells = 0;
    let mut marker_style = style(Role::Plain);
    text::visit_graphemes(line, |symbol, appearance| {
        if cells + symbol.width() > width.saturating_sub(1) {
            return false;
        }
        text::push_styled(&mut result, symbol, appearance);
        cells += symbol.width();
        marker_style = appearance;
        true
    });
    text::push_styled(&mut result, "→", marker_style);
    result
}

pub(super) struct PaintedCell {
    pub id: usize,
    pub right: bool,
    pub part: usize,
    pub selected: bool,
    pub horizontal: usize,
}

impl DiffPaint {
    pub(super) fn draw(&self, buffer: &mut Buffer, area: Rect, cell: PaintedCell) {
        let PaintedCell {
            id,
            right,
            part,
            selected,
            horizontal,
        } = cell;
        let Some(line) = self.lines.get(&(id, right)) else {
            return;
        };
        let Some(chunk) = line.chunks.get(part) else {
            return;
        };
        let gutter_width = line.gutter.width().min(usize::from(area.width)) as u16;
        if part == 0 {
            Paragraph::new(line.gutter.clone()).render(
                Rect::new(
                    area.x + 1,
                    area.y,
                    gutter_width.min(area.width.saturating_sub(1)),
                    1,
                ),
                buffer,
            );
        }
        diff::highlight_gutter(buffer, area, gutter_width, line.background);
        let horizontal = if self.horizontal { horizontal } else { 0 };
        Paragraph::new(chunk.clone())
            .scroll((0, horizontal.min(usize::from(u16::MAX)) as u16))
            .render(
                Rect::new(
                    area.x + gutter_width + 2,
                    area.y,
                    area.width.saturating_sub(gutter_width + 3),
                    1,
                ),
                buffer,
            );
        if selected {
            diff::highlight_selected_line(buffer, area);
        }
    }
}
