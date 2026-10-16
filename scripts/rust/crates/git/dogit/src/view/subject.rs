use super::*;
use unicode_segmentation::UnicodeSegmentation;

pub(super) fn prefix_style(prefix: &str) -> Style {
    let badge = chrome::identity_style(prefix);
    badge
        .fg(badge.bg.unwrap_or(Color::Reset))
        .bg(badge.fg.unwrap_or(Color::Reset))
}

pub(super) fn render_list(buffer: &mut Buffer, area: Rect, subject: &str, offset: usize) {
    if area.is_empty() {
        return;
    }
    let clean = safe_text(subject);
    let parsed = crate::commit::subject::Subject::parse(&clean);
    let mut spans = Vec::new();
    let mut consumed = 0;
    for prefix in parsed.prefixes {
        let Some(start) = clean[consumed..].find(prefix).map(|index| consumed + index) else {
            continue;
        };
        spans.push(Span::styled(&clean[consumed..start], style(Role::Plain)));
        let end = start + prefix.len();
        let end = end + usize::from(clean[end..].starts_with(':'));
        spans.push(Span::styled(
            &clean[start..end],
            chrome::identity_text_style(prefix),
        ));
        consumed = end;
    }
    spans.push(Span::styled(&clean[consumed..], style(Role::Plain)));
    ratatui::widgets::Paragraph::new(Line::from(spans))
        .scroll((0, offset.min(u16::MAX as usize) as u16))
        .render(area, buffer);
    if offset == 0 && clean.width() > usize::from(area.width) {
        put(buffer, area.right() - 1, area.y, 1, "…", style(Role::Plain));
    }
}

pub(super) fn lines(subject: &str, width: u16) -> Vec<Line<'static>> {
    header_lines(subject, width, width)
}

pub(super) fn header_lines(subject: &str, width: u16, first_width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let first_width = usize::from(first_width).min(width);
    if width == 0 {
        return vec![Line::default()];
    }
    let clean = safe_text(subject);
    let parsed = crate::commit::subject::Subject::parse(&clean);
    let mut lines = vec![Line::default()];
    for prefix in parsed.prefixes {
        let badge_width = (prefix.width() + 2).min(width);
        let label = if badge_width >= 3 {
            let label = elide_end(prefix, badge_width - 2);
            format!(" {label} ")
        } else {
            elide_end(prefix, badge_width)
        };
        append(&mut lines, label, prefix_style(prefix), width, first_width);
    }
    for word in parsed.message.split_whitespace() {
        if word.width() <= width {
            append(
                &mut lines,
                word.to_owned(),
                style(Role::Accent),
                width,
                first_width,
            );
        } else {
            if lines.last().is_some_and(|line| line.width() > 0)
                || (lines.len() == 1 && first_width < width)
            {
                lines.push(Line::default());
            }
            let mut part = String::new();
            for grapheme in word.graphemes(true) {
                if part.width() + grapheme.width() > width && !part.is_empty() {
                    append(
                        &mut lines,
                        std::mem::take(&mut part),
                        style(Role::Accent),
                        width,
                        first_width,
                    );
                    lines.push(Line::default());
                }
                part.push_str(grapheme);
            }
            append(&mut lines, part, style(Role::Accent), width, first_width);
        }
    }
    lines
}

fn append(
    lines: &mut Vec<Line<'static>>,
    token: String,
    appearance: Style,
    width: usize,
    first_width: usize,
) {
    let used = lines.last().map_or(0, Line::width);
    let available = if lines.len() == 1 { first_width } else { width };
    if used + usize::from(used > 0) + token.width() > available {
        lines.push(Line::default());
    }
    if let Some(line) = lines.last_mut() {
        if line.width() > 0 {
            line.spans.push(Span::raw(" "));
        }
        line.spans.push(Span::styled(token, appearance));
    }
}

pub(super) fn render(buffer: &mut Buffer, area: Rect, lines: &[Line<'_>]) {
    for (index, line) in lines.iter().take(usize::from(area.height)).enumerate() {
        line.render(
            Rect::new(area.x, area.y + index as u16, area.width, 1),
            buffer,
        );
    }
    if lines.len() > usize::from(area.height) && !area.is_empty() {
        put(
            buffer,
            area.right() - 1,
            area.bottom() - 1,
            1,
            "…",
            style(Role::Accent),
        );
    }
}
