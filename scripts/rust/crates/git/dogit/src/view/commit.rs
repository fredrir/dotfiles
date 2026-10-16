use super::*;
use crate::commit::{
    CommitDetails,
    avatar::{Avatar, COLUMNS, ROWS},
};

pub(super) fn subject(buffer: &mut Buffer, area: Rect, details: &CommitDetails) -> (Rect, Rect) {
    if area.is_empty() {
        return (area, Rect::default());
    }
    let title = header(buffer, area, details);
    let height = title.height.saturating_add(1).min(area.height);
    (
        Rect::new(area.x, area.y + height, area.width, area.height - height),
        title,
    )
}

fn header(buffer: &mut Buffer, area: Rect, details: &CommitDetails) -> Rect {
    if area.is_empty() {
        return Rect::default();
    }
    let (icon, status) = details.verification();
    let role = if icon == "" {
        Role::DiffAdded
    } else if status == "Unsigned" || icon == "…" {
        Role::Muted
    } else {
        Role::Warning
    };
    let badge_width = (status.width() as u16 + 4).min(area.width.saturating_sub(10));
    let badge = chrome::render_badges(
        buffer,
        Rect::new(
            area.right().saturating_sub(badge_width + 1),
            area.y,
            badge_width,
            1,
        ),
        vec![Badge::new(format!("{icon} {status}"), style(role))],
        false,
    );
    let width = badge.x.saturating_sub(area.x + 3);
    let rows = overlay::wrap_detail(&details.subject, width);
    let height = (rows.len().clamp(1, 3) as u16).min(area.height);
    for (index, line) in rows.iter().take(usize::from(height)).enumerate() {
        let line = if index + 1 == usize::from(height) && rows.len() > usize::from(height) {
            elide_end(&format!("{line} …"), usize::from(width))
        } else {
            line.clone()
        };
        put(
            buffer,
            area.x + 2,
            area.y + index as u16,
            width,
            &line,
            style(Role::Accent).add_modifier(Modifier::BOLD),
        );
    }
    Rect::new(area.x, area.y, area.width, height)
}

pub(super) fn details(
    buffer: &mut Buffer,
    area: Rect,
    state: &ViewState,
    details: &CommitDetails,
    avatar: Option<&Avatar>,
) -> (Vec<(usize, Rect)>, Rect) {
    if area.width < 8 || area.height < 4 {
        return (Vec::new(), Rect::default());
    }
    let focused = state.focus == Focus::CommitDetails;
    let title = header(buffer, area, details);
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
            "Authored",
            crate::commit::relative_time(details.timestamp, now),
            Role::Muted,
        ),
        (Some(2), "Date:", details.date(), Role::Plain),
    ];
    let has_avatar =
        avatar.is_some() && area.width >= 48 && area.bottom().saturating_sub(content_y + 1) >= ROWS;
    if has_avatar {
        rows.resize(usize::from(ROWS), (None, "", String::new(), Role::Plain));
    }
    if let Some(github) = &details.github {
        rows.push((None, "", String::new(), Role::Plain));
        let url_width = area.width.saturating_sub(14);
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
    // On short terminals, scroll the card itself to keep its selected action visible.
    let visible = usize::from(area.bottom().saturating_sub(content_y + 1));
    let selected_row = rows
        .iter()
        .position(|(field, ..)| *field == Some(state.commit_field))
        .unwrap_or(0);
    let offset = if focused {
        rows.iter()
            .rposition(|(field, ..)| *field == Some(state.commit_field))
            .unwrap_or(selected_row)
            .saturating_sub(visible.saturating_sub(1))
            .min(selected_row)
    } else {
        0
    };
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
        if active {
            put(buffer, row.x, y, 1, "┃", style(Role::Accent));
        }
        let text_width = area.width.saturating_sub(
            4 + if avatar.is_some() && index < usize::from(ROWS) {
                COLUMNS + 2
            } else {
                0
            },
        );
        let label_width = 10.min(text_width);
        put(
            buffer,
            area.x + 2,
            y,
            label_width,
            label,
            style(Role::Muted),
        );
        put(
            buffer,
            area.x + 2 + label_width,
            y,
            text_width.saturating_sub(label_width),
            &elide_end(value, usize::from(text_width.saturating_sub(label_width))),
            style(if active { Role::Accent } else { *role }).add_modifier(if active {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        );
    }
    if let Some(avatar) = avatar {
        avatar.render(
            Rect::new(area.right() - COLUMNS - 2, content_y, COLUMNS, ROWS),
            buffer,
        );
    }

    (fields, title)
}
