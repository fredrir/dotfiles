use super::*;
use unicode_segmentation::UnicodeSegmentation;

pub fn confirmation_scroll_max(detail: &str, outer: Rect) -> u16 {
    let width = outer.width.saturating_sub(8).min(78);
    let rows = wrap_detail(detail, width.saturating_sub(6)).len();
    let height = confirmation_height(rows).min(outer.height.saturating_sub(2));
    rows.saturating_sub(usize::from(height.saturating_sub(4)))
        .min(usize::from(u16::MAX)) as u16
}

fn confirmation_height(rows: usize) -> u16 {
    rows.saturating_add(4).clamp(8, usize::from(u16::MAX)) as u16
}

pub(super) fn render_overlay(
    frame: &mut Frame,
    outer: Rect,
    canvas: Rect,
    overlay: &Overlay,
    editor_offset: &mut usize,
) -> Option<Rect> {
    if let Overlay::Compose {
        value,
        cursor,
        selection,
    } = overlay
    {
        return render_compose(
            frame,
            outer,
            canvas,
            value,
            *cursor,
            selection.as_ref(),
            editor_offset,
        );
    }
    let width = outer.width.saturating_sub(8).min(78);
    let confirmation = match overlay {
        Overlay::Confirm { detail, .. } => Some(wrap_detail(detail, width.saturating_sub(6))),
        _ => None,
    };
    let desired_height = match overlay {
        Overlay::Compose { .. } => unreachable!(),
        Overlay::Commands { items, .. } => (items.len() as u16 + 7).min(19),
        Overlay::Input { value, .. } => (value.lines().count() as u16 + 7).clamp(8, 16),
        Overlay::Confirm { .. } => confirmation
            .as_ref()
            .map_or(8, |lines| confirmation_height(lines.len())),
    };
    let height = desired_height.min(outer.height.saturating_sub(2));
    if width < 12 || height < 4 {
        return None;
    }
    let x = if canvas.width > width + 4 {
        canvas.x + (canvas.width - width) / 2
    } else {
        outer.x + (outer.width - width) / 2
    };
    let y = outer.y + (outer.height - height) / 2;
    let area = Rect::new(x, y, width, height);
    Clear.render(area, frame.buffer_mut());
    frame.buffer_mut().set_style(area, style(Role::Background));
    draw_overlay_frame(frame.buffer_mut(), area);
    let content = Rect::new(
        area.x + 3,
        area.y + 2,
        area.width - 6,
        area.height.saturating_sub(4),
    );
    match overlay {
        Overlay::Compose { .. } => unreachable!(),
        Overlay::Input { title, value } => {
            border_title(
                frame.buffer_mut(),
                area.x + 2,
                area.y,
                area.width - 4,
                title,
                true,
            );
            Paragraph::new(safe_text_multiline(value))
                .style(style(Role::Plain))
                .wrap(Wrap { trim: false })
                .render(content, frame.buffer_mut());

            set_input_cursor(frame, content, value);
        }
        Overlay::Commands {
            query,
            items,
            selected,
        } => {
            border_title(
                frame.buffer_mut(),
                area.x + 2,
                area.y,
                area.width - 4,
                "Commands",
                true,
            );
            put(
                frame.buffer_mut(),
                content.x,
                content.y,
                content.width,
                query,
                style(Role::Plain),
            );
            if content.height > 2 {
                put(
                    frame.buffer_mut(),
                    content.x,
                    content.y + 1,
                    content.width,
                    &"─".repeat(usize::from(content.width)),
                    style(Role::Border),
                );
                let rows = usize::from(content.height - 2);
                let offset = selected.saturating_sub(rows.saturating_sub(1));
                for (index, item) in items.iter().enumerate().skip(offset).take(rows) {
                    let y = content.y + 2 + (index - offset) as u16;
                    if index == *selected {
                        put(
                            frame.buffer_mut(),
                            content.x.saturating_sub(2),
                            y,
                            1,
                            "┃",
                            style(Role::Accent),
                        );
                    }
                    let detail_width =
                        item.detail.width().min(usize::from(content.width / 3)) as u16;
                    put(
                        frame.buffer_mut(),
                        content.x,
                        y,
                        content.width.saturating_sub(detail_width + 2),
                        &item.label,
                        if index == *selected {
                            style(Role::Accent)
                        } else {
                            style(Role::Plain)
                        },
                    );
                    put(
                        frame.buffer_mut(),
                        content.right().saturating_sub(detail_width),
                        y,
                        detail_width,
                        &item.detail,
                        style(Role::Plain),
                    );
                }
            }
            set_input_cursor(
                frame,
                Rect::new(content.x, content.y, content.width, 1),
                query,
            );
        }
        Overlay::Confirm {
            title,
            destructive,
            scroll,
            ..
        } => {
            border_title(
                frame.buffer_mut(),
                area.x + 2,
                area.y,
                area.width - 4,
                title,
                true,
            );
            let lines = confirmation.as_deref().unwrap_or_default();
            let offset =
                usize::from(*scroll).min(lines.len().saturating_sub(usize::from(content.height)));
            Paragraph::new(
                lines
                    .iter()
                    .map(|line| Line::from(line.as_str()))
                    .collect::<Vec<_>>(),
            )
            .style(if *destructive {
                style(Role::Warning)
            } else {
                style(Role::Plain)
            })
            .scroll((offset as u16, 0))
            .render(content, frame.buffer_mut());
            put(
                frame.buffer_mut(),
                content.x,
                area.bottom() - 2,
                content.width,
                "Enter to confirm    Esc to cancel",
                style(Role::Plain),
            );
            if lines.len() > usize::from(content.height) && content.height > 0 {
                let label = format!(
                    "{}–{} / {}",
                    offset + 1,
                    (offset + usize::from(content.height)).min(lines.len()),
                    lines.len()
                );
                let label_width = (label.width() as u16 + 2).min(area.width.saturating_sub(4));
                border_title(
                    frame.buffer_mut(),
                    area.right().saturating_sub(label_width + 2),
                    area.bottom() - 1,
                    label_width,
                    &label,
                    false,
                );
                let visible = usize::from(content.height);
                let thumb = (visible * visible / lines.len()).max(1);
                let top = offset * (visible - thumb) / lines.len().saturating_sub(visible).max(1);
                for row in top..top + thumb {
                    put(
                        frame.buffer_mut(),
                        area.right() - 2,
                        content.y + row as u16,
                        1,
                        "┃",
                        style(Role::Accent),
                    );
                }
            }
        }
    }
    None
}

fn render_compose(
    frame: &mut Frame,
    outer: Rect,
    canvas: Rect,
    value: &str,
    cursor: usize,
    selection: Option<&std::ops::Range<usize>>,
    offset: &mut usize,
) -> Option<Rect> {
    let docked = canvas.width >= 36 && outer.height >= 18;
    let width = if docked {
        canvas.width
    } else {
        outer.width.saturating_sub(8).min(78)
    };
    let height = if docked {
        (outer.height / 2).clamp(9, 14)
    } else {
        16
    }
    .min(outer.height.saturating_sub(2));
    if width < 12 || height < 5 {
        return None;
    }
    let x = if docked {
        canvas.x
    } else {
        outer.x + (outer.width - width) / 2
    };
    let y = if docked {
        canvas.bottom().saturating_sub(height)
    } else {
        outer.y + (outer.height - height) / 2
    };
    let area = Rect::new(x, y, width, height);
    Clear.render(area, frame.buffer_mut());
    frame.buffer_mut().set_style(area, style(Role::Background));
    draw_overlay_frame(frame.buffer_mut(), area);
    border_title(
        frame.buffer_mut(),
        x + 2,
        y,
        width - 4,
        "Commit Message",
        true,
    );
    let content = Rect::new(x + 3, y + 2, width - 6, height - 4);
    let layout = ui_widgets::EditorLayout::new(value, usize::from(content.width));
    let (cursor_row, column) = layout.position(value, cursor);
    let total = layout.rows.len();
    let visible = usize::from(content.height);
    *offset = (*offset).min(total.saturating_sub(visible));
    if cursor_row < *offset {
        *offset = cursor_row;
    } else if cursor_row >= *offset + visible {
        *offset = cursor_row + 1 - visible;
    }
    for visible_row in 0..visible {
        let row = *offset + visible_row;
        let y = content.y + visible_row as u16;
        let Some(row) = layout.rows.get(row) else {
            break;
        };
        let appearance = style(Role::Plain);
        let mut column = 0;
        for (index, grapheme) in value[row.range.clone()].grapheme_indices(true) {
            let start = row.range.start + index;
            let length = ui_widgets::cell_width(grapheme, column);
            let selected = selection.is_some_and(|selected| {
                start < selected.end && start + grapheme.len() > selected.start
            });
            let appearance = if selected {
                style(Role::Selection).add_modifier(Modifier::REVERSED)
            } else {
                appearance
            };
            let text = if grapheme == "\t" {
                " ".repeat(length)
            } else {
                grapheme.into()
            };
            put(
                frame.buffer_mut(),
                content.x + column as u16,
                y,
                content.width.saturating_sub(column as u16),
                &text,
                appearance,
            );
            column += length;
        }
        if column < usize::from(content.width)
            && value.as_bytes().get(row.range.end) == Some(&b'\n')
            && selection.is_some_and(|selected| selected.contains(&row.range.end))
        {
            put(
                frame.buffer_mut(),
                content.x + column as u16,
                y,
                1,
                " ",
                style(Role::Selection).add_modifier(Modifier::REVERSED),
            );
        }
    }
    if value.is_empty() {
        put(
            frame.buffer_mut(),
            content.x,
            content.y,
            content.width,
            "Commit message",
            style(Role::Muted),
        );
    }
    frame.set_cursor_position((
        content.x + column as u16,
        content.y + (cursor_row - *offset) as u16,
    ));
    Some(content)
}

// Keep every path's printable representation accessible, including long
// paths with spaces. Explicit wrapping also makes scroll bounds deterministic.
fn wrap_detail(detail: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let width = usize::from(width);
    let mut rows = Vec::new();
    for line in safe_text_multiline(detail).split('\n') {
        let mut row = String::new();
        let mut column = 0;
        for character in line.chars() {
            let length = character.width().unwrap_or(0);
            if column + length > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                column = 0;
            }
            row.push(character);
            column += length;
        }
        rows.push(row);
    }
    rows
}

fn draw_overlay_frame(buffer: &mut Buffer, area: Rect) {
    let border = style(Role::Accent);
    for y in area.y + 1..area.bottom() - 1 {
        put(buffer, area.x, y, 1, "│", border);
        put(buffer, area.right() - 1, y, 1, "│", border);
    }
    for y in [area.y, area.bottom() - 1] {
        put(
            buffer,
            area.x,
            y,
            area.width,
            &"─".repeat(usize::from(area.width)),
            border,
        );
    }
    for (x, y, symbol) in [
        (area.x, area.y, "┌"),
        (area.right() - 1, area.y, "┐"),
        (area.x, area.bottom() - 1, "└"),
        (area.right() - 1, area.bottom() - 1, "┘"),
    ] {
        put(buffer, x, y, 1, symbol, border);
    }
}

fn set_input_cursor(frame: &mut Frame, area: Rect, value: &str) {
    if area.is_empty() {
        return;
    }
    let mut row = 0usize;
    let mut column = 0usize;
    for character in value.chars() {
        if character == '\n' {
            row += 1;
            column = 0;
        } else {
            column += character.width().unwrap_or(0);
            if column >= usize::from(area.width) {
                row += column / usize::from(area.width);
                column %= usize::from(area.width);
            }
        }
    }
    frame.set_cursor_position((
        area.x + column.min(usize::from(area.width - 1)) as u16,
        area.y + row.min(usize::from(area.height - 1)) as u16,
    ));
}
