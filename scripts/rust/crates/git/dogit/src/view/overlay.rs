use super::*;
use unicode_segmentation::UnicodeSegmentation;

pub fn confirmation_scroll_max(detail: &str, outer: Rect) -> u16 {
    let width = outer.width.saturating_sub(8).min(78);
    let rows = wrap_detail(detail, width.saturating_sub(6)).len();
    let height = confirmation_height(rows).min(outer.height.saturating_sub(2));
    rows.saturating_sub(usize::from(height.saturating_sub(5)))
        .min(usize::from(u16::MAX)) as u16
}

pub fn inspection_scroll_max(message: &str, outer: Rect) -> u16 {
    let width = outer.width.saturating_sub(8).min(96);
    let rows = inspection_lines(message, width.saturating_sub(6)).len();
    let height = (rows.saturating_add(6).max(12).min(usize::from(u16::MAX)) as u16)
        .min(outer.height.saturating_sub(4));
    rows.saturating_sub(usize::from(height.saturating_sub(6)))
        .min(usize::from(u16::MAX)) as u16
}

fn inspection_lines(message: &str, width: u16) -> Vec<Line<'static>> {
    let (title, body) = message
        .split_once('\n')
        .map_or((message, None), |(title, body)| (title, Some(body)));
    let mut lines = subject::lines(title, width);
    if let Some(body) = body {
        lines.extend(wrap_detail(body, width).into_iter().map(|line| {
            let role = if line.starts_with("Co-authored-by:") || line.starts_with("Signed-off-by:")
            {
                Role::Muted
            } else {
                Role::Plain
            };
            Line::styled(line, style(role))
        }));
    }
    lines
}

fn render_inspection(frame: &mut Frame, outer: Rect, message: &str, scroll: u16) -> Option<Rect> {
    let width = outer.width.saturating_sub(8).min(96);
    let rows = inspection_lines(message, width.saturating_sub(6));
    let height = (rows
        .len()
        .saturating_add(6)
        .max(12)
        .min(usize::from(u16::MAX)) as u16)
        .min(outer.height.saturating_sub(4));
    if width < 12 || height < 7 {
        return None;
    }
    let area = Rect::new(
        outer.x + (outer.width - width) / 2,
        outer.y + (outer.height - height) / 2,
        width,
        height,
    );
    Clear.render(area, frame.buffer_mut());
    frame.buffer_mut().set_style(area, style(Role::Background));
    draw_overlay_frame(frame.buffer_mut(), area, Role::Accent);
    let close = render_close_button(frame.buffer_mut(), area, Role::Accent);
    border_title(
        frame.buffer_mut(),
        area.x + 2,
        area.y,
        width - 8,
        "Inspect commit message",
        true,
    );
    let content = Rect::new(area.x + 3, area.y + 2, width - 6, height - 6);
    let offset = usize::from(scroll.min(inspection_scroll_max(message, outer)));
    for (index, line) in rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(usize::from(content.height))
    {
        line.render(
            Rect::new(
                content.x,
                content.y + (index - offset) as u16,
                content.width,
                1,
            ),
            frame.buffer_mut(),
        );
    }

    let progress = format!(
        " {}–{} / {} ",
        offset + 1,
        (offset + usize::from(content.height)).min(rows.len()),
        rows.len()
    );
    border_title(
        frame.buffer_mut(),
        area.x + 2,
        area.bottom() - 1,
        width - 4,
        &progress,
        false,
    );
    Some(close)
}

fn confirmation_height(rows: usize) -> u16 {
    rows.saturating_add(5).clamp(8, usize::from(u16::MAX)) as u16
}

pub(super) fn render_overlay(
    frame: &mut Frame,
    outer: Rect,
    overlay: &Overlay,
    editor_offset: &mut usize,
    geometry: &mut Geometry,
) {
    if let Overlay::Inspect { message, scroll } = overlay {
        geometry.overlay_close = render_inspection(frame, outer, message, *scroll);
        return;
    }
    let width = outer.width.saturating_sub(8).min(78);
    let confirmation = match overlay {
        Overlay::Confirm { detail, .. } => Some(wrap_detail(detail, width.saturating_sub(6))),
        _ => None,
    };
    let desired_height = match overlay {
        Overlay::Inspect { .. } => 0,
        Overlay::Error { .. } => outer.height.saturating_sub(4).min(24),
        Overlay::Compose { .. } => 16,
        Overlay::Commands { items, .. } => (items.len() as u16 + 7).min(19),
        Overlay::Input { .. } => 8,
        Overlay::Confirm { .. } => confirmation
            .as_ref()
            .map_or(8, |lines| confirmation_height(lines.len())),
    };
    let height = desired_height.min(outer.height.saturating_sub(2));
    if width < 12 || height < 5 {
        return;
    }
    let x = outer.x + (outer.width - width) / 2;
    let y = outer.y + (outer.height - height) / 2;
    let area = Rect::new(x, y, width, height);
    Clear.render(area, frame.buffer_mut());
    frame.buffer_mut().set_style(area, style(Role::Background));
    let border_role = if matches!(overlay, Overlay::Error { .. }) {
        Role::Danger
    } else {
        Role::Accent
    };
    draw_overlay_frame(frame.buffer_mut(), area, border_role);
    geometry.overlay_close = Some(render_close_button(frame.buffer_mut(), area, border_role));
    let title_width = area.width - 8;
    let content = Rect::new(
        area.x + 3,
        area.y + 2,
        area.width - 6,
        area.height.saturating_sub(4),
    );
    match overlay {
        Overlay::Inspect { .. } => {}
        Overlay::Error { title, editor } => {
            put(
                frame.buffer_mut(),
                area.x + 2,
                area.y,
                title_width,
                &format!(" {} ", elide_end(title, usize::from(title_width - 2))),
                style(Role::Danger).add_modifier(Modifier::BOLD),
            );
            let content = Rect::new(
                content.x,
                content.y,
                content.width,
                content.height.saturating_sub(2),
            );
            render_editor(
                frame,
                content,
                editor,
                editor_offset,
                "No error details available",
            );
            let rows = ui_widgets::EditorLayout::new(&editor.value, usize::from(content.width))
                .rows
                .len();
            let progress = format!(
                " {}–{} / {} ",
                *editor_offset + 1,
                (*editor_offset + usize::from(content.height)).min(rows),
                rows
            );
            put(
                frame.buffer_mut(),
                area.x + 2,
                area.bottom() - 1,
                area.width - 4,
                &elide_end(&progress, usize::from(area.width - 4)),
                style(Role::Danger),
            );
            geometry.editor = Some(content);
        }
        Overlay::Compose { editor } => {
            border_title(
                frame.buffer_mut(),
                area.x + 2,
                area.y,
                title_width,
                "Commit Message",
                true,
            );
            render_editor(frame, content, editor, editor_offset, "Commit message");
            geometry.editor = Some(content);
        }
        Overlay::Input { title, editor } => {
            border_title(
                frame.buffer_mut(),
                area.x + 2,
                area.y,
                title_width,
                title,
                true,
            );
            render_editor(frame, content, editor, editor_offset, "");
            geometry.editor = Some(content);
        }
        Overlay::Commands {
            editor,
            items,
            selected,
        } => {
            border_title(
                frame.buffer_mut(),
                area.x + 2,
                area.y,
                title_width,
                "Commands",
                true,
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
            let input = Rect::new(content.x, content.y, content.width, 1);
            render_editor(frame, input, editor, editor_offset, "Search commands");
            geometry.editor = Some(input);
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
                title_width,
                title,
                true,
            );
            let content = Rect::new(
                content.x,
                content.y,
                content.width,
                area.height.saturating_sub(5),
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
            geometry.overlay_confirm = Some(render_confirm_button(frame.buffer_mut(), area));
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
}

fn render_close_button(buffer: &mut Buffer, area: Rect, role: Role) -> Rect {
    let button = Rect::new(area.right() - 4, area.y, 2, 1);
    put(
        buffer,
        button.x,
        button.y,
        button.width,
        "\u{f00d} ",
        style(role).add_modifier(Modifier::BOLD),
    );
    button
}

fn render_confirm_button(buffer: &mut Buffer, area: Rect) -> Rect {
    let width = 11.min(area.width - 4);
    let button = Rect::new(area.right() - width - 2, area.bottom() - 2, width, 1);
    let appearance = Style::default()
        .fg(Color::Black)
        .bg(foreground(Role::Success))
        .add_modifier(Modifier::BOLD);
    buffer.set_style(button, appearance);
    let label = if width >= 11 {
        "Confirm ↵"
    } else {
        "Confirm"
    };
    Paragraph::new(label)
        .style(appearance)
        .centered()
        .render(button, buffer);
    button
}

fn render_editor(
    frame: &mut Frame,
    content: Rect,
    editor: &EditorView,
    offset: &mut usize,
    placeholder: &str,
) {
    if content.is_empty() {
        return;
    }
    let value = editor.value.as_str();
    let cursor = editor.cursor;
    let selection = editor.selection.as_ref();
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
            placeholder,
            style(Role::Muted),
        );
    }
    frame.set_cursor_position((
        content.x + column as u16,
        content.y + (cursor_row - *offset) as u16,
    ));
}

pub(super) fn wrap_detail(detail: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let width = usize::from(width);
    let mut rows = Vec::new();
    for line in safe_text_multiline(detail).split('\n') {
        let mut row = String::new();
        let mut column = 0;
        for token in line.split_inclusive(char::is_whitespace) {
            let word = token.trim_end();
            if column + word.width() > width && !row.is_empty() {
                let trimmed = row.trim_end();
                if !trimmed.is_empty() {
                    rows.push(trimmed.to_owned());
                }
                row.clear();
                column = 0;
            }
            for grapheme in token.graphemes(true) {
                let length = grapheme.width();
                if column + length > width && !row.is_empty() {
                    if grapheme.chars().all(char::is_whitespace) {
                        continue;
                    }
                    rows.push(std::mem::take(&mut row));
                    column = 0;
                }
                row.push_str(grapheme);
                column += length;
            }
        }
        rows.push(row);
    }
    rows
}

fn draw_overlay_frame(buffer: &mut Buffer, area: Rect, role: Role) {
    let border = style(role);
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
