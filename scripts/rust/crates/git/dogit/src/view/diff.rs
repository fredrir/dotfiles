use super::*;
use crate::presentation::{DiffPresentation, DiffPresentationConfig, WrapPolicy};

pub fn diff_horizontal_limit(diff: &ExactDiff, width: u16, split: bool) -> usize {
    configured_diff_horizontal_limit(diff, width, split, &DiffPresentationConfig::default())
}

pub fn configured_diff_horizontal_limit(
    diff: &ExactDiff,
    width: u16,
    split: bool,
    config: &DiffPresentationConfig,
) -> usize {
    let split = split && width >= 72;
    if split && config.split_wrap != WrapPolicy::Disabled {
        return 0;
    }
    let width = usize::from(if split {
        width.saturating_sub(1) / 2
    } else {
        width
    });
    diff.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .map(|line| {
            let gutter = if split {
                number_gutter(line, Some(false), config)
                    .width()
                    .max(number_gutter(line, Some(true), config).width())
            } else {
                number_gutter(line, None, config).width()
            };
            let content = String::from_utf8_lossy(&line.content);
            let length = syntax_line(content.trim_end_matches(['\r', '\n']), config.tab_width)
                .width()
                + if line.no_newline {
                    "  [no final newline]".width()
                } else {
                    0
                };
            length.saturating_sub(width.saturating_sub(gutter + 3))
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

pub fn diff_rows(diff: &ExactDiff, split: bool) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (hunk, block) in diff.hunks.iter().enumerate() {
        rows.push(DiffRow {
            hunk,
            line: None,
            right_line: None,
            ..DiffRow::default()
        });
        if !split {
            rows.extend(block.lines.iter().map(|line| DiffRow {
                hunk,
                line: Some(line.id),
                right_line: None,
                ..DiffRow::default()
            }));
            rows.push(DiffRow {
                hunk,
                hunk_end: true,
                ..DiffRow::default()
            });
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
                    ..DiffRow::default()
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
                    ..DiffRow::default()
                });
            }
        }
        rows.push(DiffRow {
            hunk,
            hunk_end: true,
            ..DiffRow::default()
        });
    }
    rows
}

pub(super) fn render_no_changes(buffer: &mut Buffer, area: Rect) {
    if area.is_empty() {
        return;
    }
    let message = "No changes";
    let width = (message.width() as u16).min(area.width);
    put(
        buffer,
        area.x + (area.width - width) / 2,
        area.y + area.height / 2,
        width,
        message,
        style(Role::Muted).add_modifier(Modifier::DIM),
    );
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
    let default_config = DiffPresentationConfig::default();
    let config = data
        .presentation
        .map_or(&default_config, |value| &value.config);
    let cached_paint = data
        .diff_paint
        .filter(|paint| paint.split == split && paint.width == area.width);
    let cached = data
        .diff_rows
        .filter(|(_, mode, width)| {
            *mode == split && (!split || (*width == area.width && cached_paint.is_some()))
        })
        .map(|(rows, _, _)| rows);
    let fallback = (split && cached.is_none())
        .then(|| configured_diff_layout(diff, area.width, split, data.presentation, config));
    let all_rows = cached.or_else(|| fallback.as_ref().map(|layout| layout.rows.as_slice()));
    let paint = fallback
        .as_ref()
        .map(|layout| &layout.paint)
        .or(cached_paint);
    let row_count = all_rows.map_or_else(
        || diff.hunks.iter().map(|hunk| hunk.lines.len() + 2).sum(),
        <[DiffRow]>::len,
    );
    if row_count == 0 {
        if let Some(message) = if diff.binary {
            Some("Binary: No preview available")
        } else {
            diff.partial_reason.as_deref()
        } {
            put(
                buffer,
                area.x + 2,
                area.y + 1,
                area.width.saturating_sub(4),
                message,
                style(Role::Plain),
            );
        } else {
            render_no_changes(buffer, area);
        }
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
    let focused_hunk = if state.focus == Focus::Canvas {
        all_rows.map_or_else(
            || {
                unified_window(diff, state.diff_cursor, 1)
                    .first()
                    .map(|row| row.hunk)
            },
            |rows| rows.get(state.diff_cursor).map(|row| row.hunk),
        )
    } else {
        None
    };
    let focused_border_style = focused_hunk
        .map(|hunk| hunk_border_style(&diff.hunks[hunk]))
        .unwrap_or_else(|| style(Role::Border));
    let half = area.width.saturating_sub(1) / 2;
    for (visible, row) in rows.iter().enumerate() {
        let y = area.y + visible as u16;
        geometry.line_ids.push(row.line);
        geometry.right_line_ids.push(row.right_line);
        geometry.hunk_indices.push(Some(row.hunk));
        let focused = focused_hunk == Some(row.hunk);
        let border_style = if focused {
            focused_border_style
        } else {
            style(Role::Border)
        };
        if row.hunk_end {
            if focused {
                put(
                    buffer,
                    area.x,
                    y,
                    area.width,
                    &"─".repeat(usize::from(area.width)),
                    border_style,
                );
            }
            continue;
        }
        if row.is_hunk_header() {
            let hunk = &diff.hunks[row.hunk];
            let new_file = !hunk.path.is_empty()
                && (row.hunk == 0 || diff.hunks[row.hunk - 1].path != hunk.path);
            if !config.hunk_headers && !new_file {
                if focused {
                    put(
                        buffer,
                        area.x,
                        y,
                        area.width,
                        &"─".repeat(usize::from(area.width)),
                        border_style,
                    );
                }
                continue;
            }
            put(
                buffer,
                area.x,
                y,
                area.width,
                &"─".repeat(usize::from(area.width)),
                border_style,
            );
            let width = area.width.saturating_sub(4);
            if new_file {
                let path = data
                    .directory_scope
                    .or_else(|| diff.path.parent())
                    .and_then(|root| hunk.file_path.strip_prefix(root).ok())
                    .unwrap_or(&hunk.file_path);
                let path = path.to_string_lossy();
                let context = if config.hunk_headers {
                    format!("   {}", safe_text(hunk.header.trim()))
                } else {
                    String::new()
                };
                let context_width = context.width().min(usize::from(width / 2)) as u16;
                let path_width = (files::display_path(&path).width() + 2)
                    .min(usize::from(width.saturating_sub(context_width + 2)))
                    as u16;
                put(
                    buffer,
                    area.x + 2,
                    y,
                    width,
                    &" ".repeat(usize::from(path_width + context_width + 2)),
                    style(Role::Muted),
                );
                files::FileLabel::new(&path, false)
                    .render(buffer, Rect::new(area.x + 3, y, path_width, 1));
                put(
                    buffer,
                    area.x + 3 + path_width,
                    y,
                    context_width,
                    &context,
                    style(Role::DiffHeader),
                );
            } else {
                put(
                    buffer,
                    area.x + 2,
                    y,
                    width,
                    &format!(" {} ", hunk.header.trim()),
                    style(Role::DiffHeader),
                );
            }
            continue;
        }
        if split {
            put(buffer, area.x + half, y, 1, "│", style(Role::Border));
            for (id, x, width, right, part) in [
                (row.line, area.x, half, false, row.left_part),
                (
                    row.right_line,
                    area.x + half + 1,
                    area.width - half - 1,
                    true,
                    row.right_part,
                ),
            ] {
                if let Some((id, paint)) = id.zip(paint) {
                    paint.draw(
                        buffer,
                        Rect::new(x, y, width, 1),
                        wrap::PaintedCell {
                            id,
                            right,
                            part,
                            selected: data.selected_lines.contains(&id),
                            horizontal: state.horizontal,
                        },
                    );
                }
            }
        } else if let Some(line) = row.line.and_then(|id| find_line(&diff.hunks[row.hunk], id)) {
            draw_diff_line(
                buffer,
                Rect::new(area.x, y, area.width, 1),
                line,
                LineAppearance {
                    selected: data.selected_lines.contains(&line.id),
                    horizontal: state.horizontal,
                    right: None,
                    presentation: data.presentation,
                    config,
                },
            );
        }
    }
}

fn hunk_border_style(hunk: &crate::diff::Hunk) -> Style {
    let added = hunk
        .lines
        .iter()
        .any(|line| line.kind == LineKind::Addition);
    let removed = hunk
        .lines
        .iter()
        .any(|line| line.kind == LineKind::Deletion);
    match (added, removed) {
        (true, true) => file_side_style(DiffSide::Worktree),
        (true, false) => style(Role::DiffAdded),
        (false, true) => style(Role::DiffRemoved),
        (false, false) => style(Role::Border),
    }
}

fn unified_window(diff: &ExactDiff, mut offset: usize, height: usize) -> Vec<DiffRow> {
    let mut rows = Vec::with_capacity(height);
    for (hunk, block) in diff.hunks.iter().enumerate() {
        if rows.len() >= height {
            break;
        }
        let count = block.lines.len() + 2;
        if offset >= count {
            offset -= count;
            continue;
        }
        for position in (offset..count).take(height - rows.len()) {
            rows.push(DiffRow {
                hunk,
                hunk_end: position == count - 1,
                line: position
                    .checked_sub(1)
                    .and_then(|index| block.lines.get(index))
                    .map(|line| line.id),
                ..DiffRow::default()
            });
        }
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

struct LineAppearance<'a> {
    selected: bool,
    horizontal: usize,
    right: Option<bool>,
    presentation: Option<&'a DiffPresentation>,
    config: &'a DiffPresentationConfig,
}

fn draw_diff_line(
    buffer: &mut Buffer,
    area: Rect,
    line: &crate::diff::DiffLine,
    appearance: LineAppearance<'_>,
) {
    let LineAppearance {
        selected,
        horizontal,
        right,
        presentation,
        config,
    } = appearance;
    let gutter = number_gutter(line, right, config);
    let gutter_width = gutter.width() as u16;
    Paragraph::new(gutter).render(
        Rect::new(
            area.x + 1,
            area.y,
            gutter_width.min(area.width.saturating_sub(1)),
            1,
        ),
        buffer,
    );
    let content = content_line(line, presentation, config);
    highlight_gutter(
        buffer,
        area,
        gutter_width,
        content.spans.first().and_then(|span| span.style.bg),
    );
    Paragraph::new(content)
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
        highlight_selected_line(buffer, area);
    }
}

pub(super) fn highlight_gutter(
    buffer: &mut Buffer,
    area: Rect,
    gutter_width: u16,
    background: Option<Color>,
) {
    if let Some(background) = background.filter(|color| *color != Color::Reset) {
        buffer.set_style(
            Rect::new(
                area.x,
                area.y,
                gutter_width.saturating_add(2).min(area.width),
                area.height,
            ),
            Style::default().bg(background),
        );
    }
}

pub(super) fn highlight_selected_line(buffer: &mut Buffer, area: Rect) {
    let background = if color_depth() == ColorDepth::Ansi16 {
        Color::Blue
    } else {
        palette().background(Role::Selection).ratatui()
    };
    let appearance = if background == Color::Reset {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().bg(background)
    };
    buffer.set_style(area, appearance);
}

pub(super) fn content_line(
    line: &crate::diff::DiffLine,
    presentation: Option<&DiffPresentation>,
    config: &DiffPresentationConfig,
) -> Line<'static> {
    let raw = String::from_utf8_lossy(&line.content);
    let content = presentation
        .and_then(|value| value.lines.get(&line.id))
        .map_or(raw.trim_end_matches(['\r', '\n']), String::as_str);
    let mut content = syntax_line(content, config.tab_width);
    if line.no_newline {
        content
            .spans
            .push(Span::styled("  [no final newline]", style(Role::Plain)));
    }
    if presentation.is_none() {
        let role = match line.kind {
            LineKind::Addition => Role::DiffAdded,
            LineKind::Deletion => Role::DiffRemoved,
            LineKind::Context => Role::DiffContext,
        };
        for span in &mut content.spans {
            span.style.fg = style(role).fg;
        }
    }
    content
}

pub(super) fn number_gutter(
    line: &crate::diff::DiffLine,
    right: Option<bool>,
    config: &DiffPresentationConfig,
) -> Line<'static> {
    let mut spans = Vec::new();
    if config.line_numbers {
        if config.left_format.is_empty()
            && (right == Some(false) || (right.is_none() && line.kind == LineKind::Deletion))
        {
            number_format(
                &mut spans,
                &config.right_format.replace("{np", "{nm"),
                "left",
                line,
                config,
            );
        } else {
            if right != Some(true) {
                number_format(&mut spans, &config.left_format, "left", line, config);
            }
            if right != Some(false) {
                number_format(&mut spans, &config.right_format, "right", line, config);
            }
        }
    }
    if config.keep_markers {
        spans.push(Span::styled(
            match line.kind {
                LineKind::Addition => "+",
                LineKind::Deletion => "-",
                LineKind::Context => " ",
            },
            match line.kind {
                LineKind::Addition => style(Role::DiffAdded),
                LineKind::Deletion => style(Role::DiffRemoved),
                LineKind::Context => style(Role::Plain),
            },
        ));
    }
    Line::from(spans)
}

fn number_format(
    spans: &mut Vec<Span<'static>>,
    format: &str,
    side: &str,
    line: &crate::diff::DiffLine,
    config: &DiffPresentationConfig,
) {
    let kind = match line.kind {
        LineKind::Addition => "plus",
        LineKind::Deletion => "minus",
        LineKind::Context => "zero",
    };
    let push = |spans: &mut Vec<Span<'static>>, value: &str, key: &str| {
        if let Some(prefix) = config.number_styles.get(key) {
            spans.extend(syntax_line(&format!("{prefix}{value}\x1b[0m"), config.tab_width).spans);
        } else {
            let role = match key {
                "plus" => Role::DiffAdded,
                "minus" => Role::DiffRemoved,
                "zero" => Role::Plain,
                _ => Role::Border,
            };
            spans.push(Span::styled(value.to_string(), style(role)));
        }
    };
    let mut remaining = format;
    while let Some(start) = remaining.find('{') {
        push(spans, &remaining[..start], side);
        let Some(end) = remaining[start..].find('}').map(|end| start + end) else {
            push(spans, &remaining[start..], side);
            return;
        };
        let token = &remaining[start + 1..end];
        let (name, layout) = token.split_once(':').unwrap_or((token, ""));
        let number = match name {
            "nm" => line.old_line,
            "np" => line.new_line,
            _ => {
                push(spans, &remaining[start..=end], side);
                remaining = &remaining[end + 1..];
                continue;
            }
        };
        let text = number.map_or(String::new(), |number| number.to_string());
        let alignment = layout
            .chars()
            .next()
            .filter(|ch| matches!(ch, '<' | '>' | '^'));
        let width = layout
            .trim_start_matches(['<', '>', '^'])
            .parse::<usize>()
            .unwrap_or(0)
            .min(64);
        let text = match alignment {
            Some('<') => format!("{text:<width$}"),
            Some('^') => format!("{text:^width$}"),
            _ => format!("{text:>width$}"),
        };
        push(spans, &text, kind);
        remaining = &remaining[end + 1..];
    }
    push(spans, remaining, side);
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
