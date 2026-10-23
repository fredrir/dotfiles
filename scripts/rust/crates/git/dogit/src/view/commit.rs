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
    let title = header(buffer, area, details, false, reserved);
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
        Self {
            lines: subject::header_lines(&details.subject, inner_width, first_width),
            verification,
            aside_width,
            title_width: inner_width,
        }
    }

    fn height(&self) -> u16 {
        3
    }
}

fn header(
    buffer: &mut Buffer,
    area: Rect,
    details: &CommitDetails,
    show_verification: bool,
    reserved: u16,
) -> Rect {
    if area.is_empty() {
        return Rect::default();
    }
    let header = Header::new(area.width, details, show_verification, reserved);
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
) -> u16 {
    let github_rows = details.github.as_ref().map_or(0, |github| {
        let url_width = width.saturating_sub(INSET * 2 + LABEL_WIDTH);
        1 + overlay::wrap_detail(&github.url, url_width).len().min(3) as u16
    });
    Header::new(width, details, show_verification, reserved).height() + 1 + ROWS + github_rows + 1
}

#[derive(Default)]
pub(super) struct Details {
    pub fields: Vec<(usize, Rect)>,
    pub title: Rect,
    pub scroll_max: usize,
    pub scrollbar: Option<Scrollbar>,
}

pub(super) fn details(
    buffer: &mut Buffer,
    area: Rect,
    state: &mut ViewState,
    details: &CommitDetails,
    avatar: Option<&Avatar>,
    show_verification: bool,
    reserved: u16,
) -> Details {
    if area.width < 8 || area.height < 4 {
        return Details::default();
    }
    let focused = state.focus == Focus::CommitDetails;
    let title = header(buffer, area, details, show_verification, reserved);
    let content_y = title.bottom() + 1;
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
    let has_avatar = avatar.is_some()
        && area.width >= INSET * 2 + LABEL_WIDTH + 18 + GAP + aside_width
        && area.bottom().saturating_sub(content_y + 1) >= ROWS;
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
    let visible = usize::from(area.bottom().saturating_sub(content_y + 1));
    let selected_row = rows
        .iter()
        .position(|(field, ..)| *field == Some(state.commit_field))
        .unwrap_or(0);
    let maximum = rows.len().saturating_sub(visible);
    if !state.commit_manual_scroll {
        state.commit_offset = if focused {
            rows.iter()
                .rposition(|(field, ..)| *field == Some(state.commit_field))
                .unwrap_or(selected_row)
                .saturating_sub(visible.saturating_sub(1))
                .min(selected_row)
        } else {
            0
        };
    }
    state.commit_offset = state.commit_offset.min(maximum);
    let offset = state.commit_offset;
    let avatar = avatar.filter(|_| has_avatar && offset == 0);
    let mut fields: Vec<(usize, Rect)> = Vec::new();
    for (index, (field, label, value, role)) in rows.iter().enumerate().skip(offset).take(visible) {
        let y = content_y + (index - offset) as u16;
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
            Rect::new(area.right() - INSET - COLUMNS, content_y, COLUMNS, ROWS),
            buffer,
        );
    }

    let track = Rect::new(area.x, content_y, area.width, visible as u16);
    Details {
        fields,
        title,
        scroll_max: maximum,
        scrollbar: Scrollbar::new(track, rows.len(), visible, offset, 3),
    }
}
