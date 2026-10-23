use super::*;
use crate::commit::{
    CommitDetails,
    avatar::{Avatar, COLUMNS, ROWS},
};

pub(super) fn subject(
    buffer: &mut Buffer,
    area: Rect,
    details: &CommitDetails,
    reserved: u16,
) -> (Rect, Rect) {
    if area.is_empty() {
        return (area, Rect::default());
    }
    let title = header(
        buffer,
        area,
        Header::new(area.width, details, false, reserved),
    );
    let height = title.height.saturating_add(1).min(area.height);
    (
        Rect::new(area.x, area.y + height, area.width, area.height - height),
        title,
    )
}

const INSET: u16 = 2;
const GAP: u16 = 2;
const LABEL_WIDTH: u16 = 10;

struct Header {
    lines: Vec<Line<'static>>,
    verification: Vec<Badge>,
    aside_width: u16,
    title_width: u16,
}

pub(super) fn verification_badges(details: &CommitDetails) -> Vec<Badge> {
    let verified = details.verification().0 == "";
    let signed = match details.signature.as_deref() {
        Some("G" | "S" | "U" | "B" | "X" | "Y" | "R") => true,
        None => verified,
        _ => false,
    };
    match (signed, verified) {
        (true, true) => vec![Badge::new(" Signed".into(), style(Role::DiffAdded))],
        (true, false) => vec![
            Badge::new("Signed".into(), style(Role::Muted)),
            Badge::new("Unverified".into(), style(Role::Warning)),
        ],
        (false, true) => vec![
            Badge::new("Unsigned".into(), style(Role::Muted)),
            Badge::new(" Verified".into(), style(Role::DiffAdded)),
        ],
        (false, false) => Vec::new(),
    }
}

impl Header {
    fn new(width: u16, details: &CommitDetails, show_verification: bool, reserved: u16) -> Self {
        let verification = verification_badges(details);
        let inner_width = width.saturating_sub(INSET * 2);
        let aside_width = if show_verification {
            verification
                .iter()
                .map(|badge| badge.width() + 1)
                .sum::<usize>()
                .saturating_sub(1)
                .min(usize::from(inner_width)) as u16
        } else {
            0
        };
        let first_width =
            inner_width.saturating_sub(aside_width + u16::from(aside_width > 0) * GAP + reserved);
        let subject = details
            .message
            .as_deref()
            .map(|message| message.split('\n').next().unwrap_or_default())
            .unwrap_or(&details.subject);
        Self {
            lines: subject::header_lines(subject, inner_width, first_width),
            verification,
            aside_width,
            title_width: inner_width,
        }
    }

    fn height(&self) -> u16 {
        3
    }

    fn message_lines(&self, details: &CommitDetails) -> Vec<Line<'static>> {
        let mut lines = self.lines.clone();
        lines.push(Line::default());
        if let Some((_, body)) = details
            .message
            .as_deref()
            .and_then(|message| message.split_once('\n'))
        {
            lines.extend(
                overlay::wrap_detail(body, self.title_width)
                    .into_iter()
                    .map(|line| Line::styled(line, style(Role::Plain))),
            );
        }
        if details
            .message
            .as_deref()
            .is_some_and(|message| message.contains('\n'))
        {
            lines.push(Line::default());
        }
        lines
    }
}

fn header(buffer: &mut Buffer, area: Rect, header: Header) -> Rect {
    if area.is_empty() {
        return Rect::default();
    }
    let height = header.height().min(area.height);
    chrome::render_badges(
        buffer,
        Rect::new(
            area.right().saturating_sub(INSET + header.aside_width),
            area.y,
            header.aside_width,
            1,
        ),
        header.verification,
        false,
    );
    subject::render(
        buffer,
        Rect::new(area.x + INSET, area.y, header.title_width, height),
        &header.lines,
    );
    Rect::new(area.x, area.y, area.width, height)
}

pub(super) fn height(
    width: u16,
    details: &CommitDetails,
    show_verification: bool,
    reserved: u16,
) -> usize {
    let github_rows = details.github.as_ref().map_or(0, |github| {
        let url_width = width.saturating_sub(INSET * 2 + LABEL_WIDTH);
        1 + overlay::wrap_detail(&github.url, url_width).len().min(3) as u16
    });
    let header = Header::new(width, details, show_verification, reserved);
    usize::from(ROWS + github_rows + 1) + header.message_lines(details).len()
}

#[derive(Default)]
pub(super) struct Details {
    pub area: Rect,
    pub fields: Vec<(usize, Rect)>,
    pub title: Rect,
    pub scroll_max: usize,
    pub scrollbar: Option<Scrollbar>,
}

pub(super) struct Options {
    pub show_verification: bool,
    pub reserved: u16,
    pub inline: bool,
}

pub(super) fn details(
    buffer: &mut Buffer,
    area: Rect,
    state: &mut ViewState,
    details: &CommitDetails,
    avatar: Option<&Avatar>,
    options: Options,
) -> Details {
    if area.width < 8 || area.is_empty() {
        return Details::default();
    }
    let focused = state.focus == Focus::CommitDetails;
    let title = Header::new(
        area.width,
        details,
        options.show_verification,
        options.reserved,
    );
    let message = title.message_lines(details);
    let content_y = area.y;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64);
    let mut rows = vec![
        (Some(0), "Author:", details.author.clone(), Role::Plain),
        (Some(1), "Commit:", details.short_oid(), Role::Accent),
        (None, "", String::new(), Role::Plain),
        (
            None,
            "Authored:",
            crate::commit::relative_time(details.timestamp, now),
            Role::Muted,
        ),
        (Some(2), "Date:", details.date(), Role::Plain),
    ];
    let aside_width = COLUMNS;
    if let Some(github) = &details.github {
        rows.push((None, "", String::new(), Role::Plain));
        let url_width = area.width.saturating_sub(INSET * 2 + LABEL_WIDTH);
        for (index, part) in overlay::wrap_detail(&github.url, url_width)
            .into_iter()
            .take(3)
            .enumerate()
        {
            rows.push((
                Some(3),
                if index == 0 { "GitHub:" } else { "" },
                part,
                Role::Accent,
            ));
        }
    }
    let viewport = usize::from(area.height).saturating_sub(usize::from(!options.inline));
    let selected_row = rows
        .iter()
        .position(|(field, ..)| *field == Some(state.commit_field))
        .unwrap_or(0)
        + message.len();
    let total = message.len() + rows.len();
    let maximum = total.saturating_sub(viewport);
    if !state.commit_manual_scroll && !options.inline {
        state.commit_offset = if focused {
            rows.iter()
                .rposition(|(field, ..)| *field == Some(state.commit_field))
                .map_or(selected_row, |index| message.len() + index)
                .saturating_sub(viewport.saturating_sub(1))
                .min(selected_row)
        } else {
            0
        };
    }
    if !options.inline {
        state.commit_offset = state.commit_offset.min(maximum);
    }
    let offset = state.commit_offset;
    let height = if options.inline {
        (total + 1)
            .saturating_sub(offset)
            .min(usize::from(area.height)) as u16
    } else {
        area.height
    };
    let area = Rect::new(area.x, area.y, area.width, height);
    let visible = total.saturating_sub(offset).min(viewport);
    let title_area = Rect::new(
        area.x,
        area.y,
        area.width,
        title.lines.len().saturating_sub(offset).min(visible) as u16,
    );
    if offset == 0 && options.show_verification {
        chrome::render_badges(
            buffer,
            Rect::new(
                area.right().saturating_sub(INSET + title.aside_width),
                area.y,
                title.aside_width,
                1,
            ),
            title.verification,
            false,
        );
    }
    let metadata_offset = message.len().saturating_sub(offset);
    let avatar = avatar.filter(|_| {
        area.width >= INSET * 2 + LABEL_WIDTH + 18 + GAP + aside_width
            && offset <= message.len()
            && visible.saturating_sub(metadata_offset) >= usize::from(ROWS)
    });
    for (index, line) in message.iter().enumerate().skip(offset).take(visible) {
        let outdent = u16::from(index < title.lines.len() && subject::leads_with_badge(line));
        line.render(
            Rect::new(
                area.x + INSET - outdent,
                content_y + (index - offset) as u16,
                area.width.saturating_sub(INSET * 2),
                1,
            ),
            buffer,
        );
    }
    let mut fields: Vec<(usize, Rect)> = Vec::new();
    for (index, (field, label, value, role)) in rows
        .iter()
        .enumerate()
        .skip(offset.saturating_sub(message.len()))
        .take(visible.saturating_sub(metadata_offset))
    {
        let y = content_y + (message.len() + index - offset) as u16;
        let row = Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1);
        let active = focused && *field == Some(state.commit_field);
        if let Some(field) = field {
            if let Some((_, existing)) = fields.iter_mut().find(|(index, _)| index == field) {
                existing.height += 1;
            } else {
                fields.push((*field, row));
            }
        }
        let appearance = navigator::row_style(active, active, false);
        buffer.set_style(row, appearance);
        let text_width = area.width.saturating_sub(
            4 + if avatar.is_some() && index < usize::from(ROWS) {
                aside_width + GAP
            } else {
                0
            },
        );
        let label_width = LABEL_WIDTH.min(text_width);
        put(
            buffer,
            area.x + 2,
            y,
            label_width,
            label,
            appearance.fg(foreground(Role::Muted)),
        );
        put(
            buffer,
            area.x + 2 + label_width,
            y,
            text_width.saturating_sub(label_width),
            &elide_end(value, usize::from(text_width.saturating_sub(label_width))),
            appearance
                .fg(foreground(if active { Role::Accent } else { *role }))
                .add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
    }
    if let Some(avatar) = avatar {
        avatar.render(
            Rect::new(
                area.right() - INSET - COLUMNS,
                content_y + metadata_offset as u16,
                COLUMNS,
                ROWS,
            ),
            buffer,
        );
    }

    let track = Rect::new(area.x, content_y, area.width, visible as u16);
    Details {
        area,
        fields,
        title: title_area,
        scroll_max: maximum,
        scrollbar: (!options.inline)
            .then(|| Scrollbar::new(track, total, visible, offset, 3))
            .flatten(),
    }
}

pub(super) fn field_offset(
    width: u16,
    details: &CommitDetails,
    reserved: u16,
    field: usize,
) -> usize {
    Header::new(width, details, false, reserved)
        .message_lines(details)
        .len()
        + match field {
            0 => 0,
            1 => 1,
            2 => 4,
            _ => 6,
        }
}
