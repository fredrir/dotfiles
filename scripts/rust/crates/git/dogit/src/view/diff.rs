use super::*;

pub fn diff_horizontal_limit(diff: &ExactDiff, width: u16, split: bool) -> usize {
    let split = split && width >= 72;
    let width = usize::from(if split {
        width.saturating_sub(1) / 2
    } else {
        width
    });
    let digits = |number: Option<usize>| number.map_or(4, |value| value.to_string().len().max(4));
    diff.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .map(|line| {
            let gutter = if split {
                digits(line.old_line).max(digits(line.new_line)) + 3
            } else {
                digits(line.old_line) + digits(line.new_line) + 4
            };
            let content = String::from_utf8_lossy(&line.content);
            let length = safe_text(content.trim_end_matches(['\r', '\n'])).width()
                + if line.no_newline {
                    "  [no final newline]".width()
                } else {
                    0
                };
            length.saturating_sub(width.saturating_sub(gutter + 2))
        })
        .max()
        .unwrap_or(0)
}

pub fn text_horizontal_limit<'a>(lines: impl IntoIterator<Item = &'a str>, width: u16) -> usize {
    lines
        .into_iter()
        .map(|line| ansi_line(line).width())
        .max()
        .unwrap_or(0)
        .saturating_sub(usize::from(width.saturating_sub(4)))
}

/// Display order is shared by rendering and keyboard/mouse navigation.
pub fn diff_rows(diff: &ExactDiff, split: bool) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (hunk, block) in diff.hunks.iter().enumerate() {
        rows.push(DiffRow {
            hunk,
            line: None,
            right_line: None,
        });
        if !split {
            rows.extend(block.lines.iter().map(|line| DiffRow {
                hunk,
                line: Some(line.id),
                right_line: None,
            }));
            continue;
        }
        let mut index = 0;
        while index < block.lines.len() {
            let line = &block.lines[index];
            if line.kind == LineKind::Context {
                rows.push(DiffRow {
                    hunk,
                    line: Some(line.id),
                    right_line: Some(line.id),
                });
                index += 1;
                continue;
            }
            let mut removed = Vec::new();
            let mut added = Vec::new();
            while index < block.lines.len() && block.lines[index].kind != LineKind::Context {
                let line = &block.lines[index];
                if line.kind == LineKind::Deletion {
                    removed.push(line.id);
                } else {
                    added.push(line.id);
                }
                index += 1;
            }
            for position in 0..removed.len().max(added.len()) {
                rows.push(DiffRow {
                    hunk,
                    line: removed.get(position).copied(),
                    right_line: added.get(position).copied(),
                });
            }
        }
    }
    rows
}

pub(super) fn render_diff(
    buffer: &mut Buffer,
    area: Rect,
    state: &mut ViewState,
    data: &RenderData<'_>,
    diff: &ExactDiff,
    geometry: &mut Geometry,
) {
    if area.is_empty() {
        return;
    }
    let split = state.split && area.width >= 72;
    let cached = data
        .diff_rows
        .filter(|(_, mode)| *mode == split)
        .map(|(rows, _)| rows);
    let aligned = (split && cached.is_none()).then(|| diff_rows(diff, true));
    let all_rows = cached.or(aligned.as_deref());
    let row_count = all_rows.map_or_else(
        || diff.hunks.iter().map(|hunk| hunk.lines.len() + 1).sum(),
        <[DiffRow]>::len,
    );
    if row_count == 0 {
        let message = diff.partial_reason.as_deref().unwrap_or("");
        put(
            buffer,
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            message,
            style(Role::Plain),
        );
        return;
    }
    state.diff_cursor = state.diff_cursor.min(row_count - 1);
    state.diff_offset = state
        .diff_offset
        .min(row_count.saturating_sub(usize::from(area.height)));
    if state.focus == Focus::Canvas && !state.canvas_manual_scroll {
        if state.diff_cursor < state.diff_offset {
            state.diff_offset = state.diff_cursor;
        } else if state.diff_cursor >= state.diff_offset + usize::from(area.height) {
            state.diff_offset = state.diff_cursor + 1 - usize::from(area.height);
        }
    }
    let visible_rows;
    let rows = if let Some(rows) = all_rows {
        &rows[state.diff_offset..rows.len().min(state.diff_offset + usize::from(area.height))]
    } else {
        visible_rows = unified_window(diff, state.diff_offset, usize::from(area.height));
        &visible_rows
    };
    let half = area.width.saturating_sub(1) / 2;
    for (visible, row) in rows.iter().enumerate() {
        let position = state.diff_offset + visible;
        let y = area.y + visible as u16;
        geometry.line_ids.push(row.line);
        geometry.right_line_ids.push(row.right_line);
        geometry.hunk_indices.push(Some(row.hunk));
        let cursor = state.focus == Focus::Canvas && position == state.diff_cursor;
        if row.line.is_none() && row.right_line.is_none() {
            let hunk = &diff.hunks[row.hunk];
            let header = if diff.path.as_os_str().is_empty() && !hunk.path.is_empty() {
                let path = hunk.path.rsplit(" b/").next().unwrap_or(&hunk.path);
                format!("{}   {}", path.trim_matches('"'), hunk.header.trim())
            } else {
                hunk.header.trim().to_string()
            };
            put(
                buffer,
                area.x,
                y,
                area.width,
                &"─".repeat(usize::from(area.width)),
                style(Role::Border),
            );
            put(
                buffer,
                area.x + 2,
                y,
                area.width.saturating_sub(4),
                &format!(" {} ", header.trim()),
                if cursor {
                    style(Role::Accent)
                } else {
                    style(Role::DiffHeader)
                },
            );
            continue;
        }
        if split {
            put(buffer, area.x + half, y, 1, "│", style(Role::Border));
            for (id, x, width, right) in [
                (row.line, area.x, half, false),
                (
                    row.right_line,
                    area.x + half + 1,
                    area.width - half - 1,
                    true,
                ),
            ] {
                if let Some(line) = id.and_then(|id| find_line(&diff.hunks[row.hunk], id)) {
                    draw_diff_line(
                        buffer,
                        Rect::new(x, y, width, 1),
                        line,
                        cursor,
                        data.selected_lines.contains(&line.id),
                        state.horizontal,
                        Some(right),
                    );
                }
            }
        } else if let Some(line) = row.line.and_then(|id| find_line(&diff.hunks[row.hunk], id)) {
            draw_diff_line(
                buffer,
                Rect::new(area.x, y, area.width, 1),
                line,
                cursor,
                data.selected_lines.contains(&line.id),
                state.horizontal,
                None,
            );
        }
    }
}

// Jump across whole hunks and slice directly into the first visible one. Large
// unified patches require work proportional to the viewport on each redraw.
fn unified_window(diff: &ExactDiff, mut offset: usize, height: usize) -> Vec<DiffRow> {
    let mut rows = Vec::with_capacity(height);
    for (hunk, block) in diff.hunks.iter().enumerate() {
        if rows.len() >= height {
            break;
        }
        if offset > block.lines.len() {
            offset -= block.lines.len() + 1;
            continue;
        }
        if offset == 0 {
            rows.push(DiffRow {
                hunk,
                line: None,
                right_line: None,
            });
        }
        rows.extend(
            block
                .lines
                .iter()
                .skip(offset.saturating_sub(1))
                .take(height - rows.len())
                .map(|line| DiffRow {
                    hunk,
                    line: Some(line.id),
                    right_line: None,
                }),
        );
        offset = 0;
    }
    rows
}

fn find_line(hunk: &crate::diff::Hunk, id: usize) -> Option<&crate::diff::DiffLine> {
    hunk.lines
        .binary_search_by_key(&id, |line| line.id)
        .ok()
        .map(|index| &hunk.lines[index])
}

fn draw_diff_line(
    buffer: &mut Buffer,
    area: Rect,
    line: &crate::diff::DiffLine,
    cursor: bool,
    selected: bool,
    horizontal: usize,
    right: Option<bool>,
) {
    let role = match line.kind {
        LineKind::Addition => Role::DiffAdded,
        LineKind::Deletion => Role::DiffRemoved,
        LineKind::Context => Role::DiffContext,
    };
    if selected || cursor {
        put(
            buffer,
            area.x,
            area.y,
            1,
            if selected { "▎" } else { "┃" },
            style(Role::Accent),
        );
    }
    let marker = match line.kind {
        LineKind::Addition => "+",
        LineKind::Deletion => "-",
        LineKind::Context => " ",
    };
    let number = |value: Option<usize>| value.map_or(String::new(), |value| value.to_string());
    let gutter = match right {
        Some(true) => format!("{:>4} {marker} ", number(line.new_line)),
        Some(false) => format!("{:>4} {marker} ", number(line.old_line)),
        None => format!(
            "{:>4} {:>4} {marker} ",
            number(line.old_line),
            number(line.new_line)
        ),
    };
    let gutter_width = gutter.len() as u16;
    put(
        buffer,
        area.x + 1,
        area.y,
        area.width.saturating_sub(1),
        &gutter,
        if line.kind == LineKind::Context {
            style(Role::Muted)
        } else {
            style(role)
        },
    );
    let content = String::from_utf8_lossy(&line.content);
    let mut content = safe_text(content.trim_end_matches(['\r', '\n']));
    if line.no_newline {
        content.push_str("  [no final newline]");
    }
    let text = window(
        &content,
        horizontal,
        usize::from(area.width.saturating_sub(gutter_width + 2)),
    );
    put(
        buffer,
        area.x + gutter_width + 1,
        area.y,
        area.width.saturating_sub(gutter_width + 2),
        &text,
        if selected {
            style(role).add_modifier(Modifier::BOLD)
        } else {
            style(role)
        },
    );
}

pub(super) fn render_text(buffer: &mut Buffer, area: Rect, state: &ViewState, lines: &[String]) {
    for (row, text) in lines
        .iter()
        .skip(state.diff_offset)
        .take(usize::from(area.height))
        .enumerate()
    {
        let y = area.y + row as u16;
        let mut line = ansi_line(text);
        for span in &mut line.spans {
            span.style.fg = style(Role::Plain).fg;
        }
        Paragraph::new(line)
            .scroll((0, state.horizontal.min(usize::from(u16::MAX)) as u16))
            .render(
                Rect::new(area.x + 2, y, area.width.saturating_sub(4), 1),
                buffer,
            );
    }
}

pub(super) fn render_structural(buffer: &mut Buffer, area: Rect, state: &ViewState, text: &str) {
    for (row, text) in text
        .lines()
        .skip(state.diff_offset)
        .take(usize::from(area.height))
        .enumerate()
    {
        Paragraph::new(ansi_line(text))
            .scroll((0, state.horizontal.min(usize::from(u16::MAX)) as u16))
            .render(
                Rect::new(
                    area.x + 2,
                    area.y + row as u16,
                    area.width.saturating_sub(4),
                    1,
                ),
                buffer,
            );
    }
}
