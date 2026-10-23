use super::*;
use crate::git::{Branch, Commit};

const MARGIN: u16 = 1;
const PADDING: u16 = 1;
const ITEM_HEIGHT: usize = 3;
const INSET: u16 = 2;
const LABEL_WIDTH: u16 = 10;
const UPSTREAM_AHEAD: &str = "\u{f062}";
const UPSTREAM_BEHIND: &str = "\u{f063}";

type Spans = Vec<(String, Style)>;

pub(super) fn group(branch: &Branch) -> &str {
    branch.remote_parts().map_or("Local", |(remote, _)| remote)
}

fn label(branch: &Branch) -> &str {
    branch.remote_parts().map_or(&branch.name, |(_, name)| name)
}

fn item_group(item: &NavItem) -> &str {
    item.branch.as_ref().map_or("", group)
}

fn starts_group(items: &[NavItem], index: usize) -> bool {
    index == 0 || item_group(&items[index - 1]) != item_group(&items[index])
}

fn content_top(items: &[NavItem], offset: usize, index: usize) -> usize {
    (offset..=index)
        .map(|row| {
            usize::from(row == offset || starts_group(items, row))
                + if row < index { ITEM_HEIGHT } else { 0 }
        })
        .sum()
}

fn scroll_max(items: &[NavItem], height: u16) -> usize {
    let height = usize::from(height);
    let mut rows = 0;
    for index in (0..items.len()).rev() {
        rows += ITEM_HEIGHT + usize::from(starts_group(items, index));
        let sticky = usize::from(!starts_group(items, index));
        if rows + sticky > height + 1 {
            return (index + 1).min(items.len().saturating_sub(1));
        }
    }
    0
}

pub(super) fn visible_items(items: &[NavItem], height: u16) -> usize {
    items.len() - scroll_max(items, height)
}

pub(super) fn render_list(
    buffer: &mut Buffer,
    area: Rect,
    state: &mut ViewState,
    items: &[NavItem],
) -> navigator::Navigation {
    let maximum = scroll_max(items, area.height);
    let mut navigation = navigator::Navigation {
        scroll_max: maximum,
        ..navigator::Navigation::default()
    };
    if area.is_empty() || items.is_empty() {
        return navigation;
    }
    state.selected = state.selected.min(items.len() - 1);
    if !state.nav_manual_scroll {
        state.offset = state.offset.min(state.selected);
        while state.offset < state.selected
            && content_top(items, state.offset, state.selected) + 2 > usize::from(area.height)
        {
            state.offset += 1;
        }
    }
    state.offset = state.offset.min(maximum);
    let now = now();
    let mut y = area.y;
    for (index, item) in items.iter().enumerate().skip(state.offset) {
        let Some(branch) = &item.branch else {
            continue;
        };
        if index == state.offset || starts_group(items, index) {
            if y >= area.bottom() {
                break;
            }
            header(buffer, Rect::new(area.x, y, area.width, 1), group(branch));
            y += 1;
        }
        if y >= area.bottom() {
            break;
        }
        let height = 2.min(area.bottom() - y);
        navigation
            .items
            .push((index, Rect::new(area.x, y, area.width, height)));
        let selected = index == state.selected && !item.disabled;
        let appearance = navigator::row_style(
            selected,
            selected && state.focus == Focus::Navigator,
            state.nav_hovered == Some(index),
        );
        let card = Rect::new(
            area.x + MARGIN,
            y,
            area.width.saturating_sub(MARGIN * 2 + 1),
            height,
        );
        buffer.set_style(card, appearance);
        row(buffer, card, branch, appearance, selected, now);
        y = y.saturating_add(ITEM_HEIGHT as u16);
    }
    navigation
}

fn header(buffer: &mut Buffer, area: Rect, title: &str) {
    let x = area.x + MARGIN + PADDING;
    let width = area.width.saturating_sub(MARGIN * 2 + PADDING + 1);
    let title = elide_end(title, usize::from(width));
    let title_width = title.width() as u16;
    put(
        buffer,
        x,
        area.y,
        width,
        &title,
        style(Role::Muted).add_modifier(Modifier::BOLD),
    );
    let rule = width.saturating_sub(title_width + 1);
    put(
        buffer,
        x + title_width + 1,
        area.y,
        rule,
        &"─".repeat(usize::from(rule)),
        style(Role::Border),
    );
}

fn row(
    buffer: &mut Buffer,
    area: Rect,
    branch: &Branch,
    appearance: Style,
    selected: bool,
    now: i64,
) {
    let x = area.x + PADDING;
    let width = area.width.saturating_sub(PADDING * 2);
    if width < 4 {
        return;
    }
    let background = appearance.bg.unwrap_or(Color::Reset);
    let name_style = if branch.current {
        appearance.fg(foreground(Role::Accent))
    } else {
        appearance.fg(foreground(Role::Plain))
    };
    let name_style = if branch.current || selected {
        name_style.add_modifier(Modifier::BOLD)
    } else {
        name_style
    };
    if branch.current {
        put(buffer, x, area.y, 1, "●", name_style);
    }
    let divergence = divergence(branch);
    let used = right_spans(
        buffer,
        Rect::new(x + 2, area.y, width - 2, 1),
        &divergence,
        background,
    );
    let name_width = (width - 2).saturating_sub(used + u16::from(used > 0) * 2);
    put(
        buffer,
        x + 2,
        area.y,
        name_width,
        &elide_middle(label(branch), usize::from(name_width)),
        name_style,
    );
    if area.height < 2 {
        return;
    }
    let muted = appearance.fg(foreground(Role::Muted));
    let mut details = upstream_sync(branch);
    if !details.is_empty() {
        details.push(("  ".into(), muted));
    }
    details.push((age(branch.timestamp, now), muted));
    let line = Rect::new(x + 2, area.y + 1, width - 2, 1);
    let used = right_spans(buffer, line, &details, background);
    let subject_width = line.width.saturating_sub(used + 2);
    put(
        buffer,
        line.x,
        line.y,
        subject_width,
        &elide_end(&safe_text(&branch.subject), usize::from(subject_width)),
        muted,
    );
}

fn divergence(branch: &Branch) -> Spans {
    match branch.divergence {
        _ if branch.current => Vec::new(),
        None | Some((0, 0)) => Vec::new(),
        Some((0, _)) => vec![("merged".into(), style(Role::Muted))],
        Some((ahead, behind)) => [
            (ahead, "↑", Role::DiffAdded),
            (behind, "↓", Role::DiffRemoved),
        ]
        .into_iter()
        .filter(|(count, _, _)| *count > 0)
        .map(|(count, arrow, role)| (format!("{arrow}{count}"), style(role)))
        .collect(),
    }
}

fn upstream_sync(branch: &Branch) -> Spans {
    if branch.remote || branch.upstream.is_none() {
        return Vec::new();
    }
    if branch.upstream_gone {
        return vec![("gone".into(), style(Role::Warning))];
    }
    let (ahead, behind) = branch.tracking;
    [
        (ahead, UPSTREAM_AHEAD, Role::DiffAdded),
        (behind, UPSTREAM_BEHIND, Role::DiffRemoved),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, arrow, role)| (format!("{count} {arrow}"), style(role)))
    .collect()
}

fn spans_width(spans: &[(String, Style)]) -> u16 {
    let separators = spans
        .windows(2)
        .filter(|pair| !pair[0].0.ends_with(' ') && !pair[1].0.starts_with(' '))
        .count();
    (spans.iter().map(|(text, _)| text.width()).sum::<usize>() + separators) as u16
}

fn right_spans(
    buffer: &mut Buffer,
    area: Rect,
    spans: &[(String, Style)],
    background: Color,
) -> u16 {
    let width = spans_width(spans);
    if width == 0 || width + 4 > area.width {
        return 0;
    }
    left_spans(
        buffer,
        Rect::new(area.right() - width, area.y, width, 1),
        spans,
        background,
    );
    width
}

fn left_spans(
    buffer: &mut Buffer,
    area: Rect,
    spans: &[(String, Style)],
    background: Color,
) -> u16 {
    let mut x = area.x;
    for (index, (text, appearance)) in spans.iter().enumerate() {
        if index > 0 && !spans[index - 1].0.ends_with(' ') && !text.starts_with(' ') {
            x += 1;
        }
        let available = area.right().saturating_sub(x);
        let text = elide_end(text, usize::from(available));
        let width = text.width() as u16;
        put(buffer, x, area.y, width, &text, appearance.bg(background));
        x += width;
    }
    x - area.x
}

fn age(timestamp: i64, now: i64) -> String {
    let seconds = now.saturating_sub(timestamp).max(0);
    match seconds {
        0..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        86_400..2_592_000 => format!("{}d", seconds / 86_400),
        2_592_000..31_536_000 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64)
}

enum Row<'a> {
    Title,
    Field(&'static str, Spans),
    Blank,
    Section(String),
    Commit(usize, &'a Commit, bool),
    More(usize),
}

fn rows<'a>(panel: &BranchPanel<'a>, now: i64) -> Vec<Row<'a>> {
    let branch = panel.branch;
    let muted = style(Role::Muted);
    let mut rows = vec![Row::Title, Row::Blank];
    if let Some((remote, _)) = branch.remote_parts() {
        rows.push(Row::Field(
            "Remote:",
            vec![(remote.into(), style(Role::Plain))],
        ));
    } else if let Some(upstream) = &branch.upstream {
        let mut value = vec![(upstream.clone(), style(Role::Plain))];
        let (ahead, behind) = branch.tracking;
        if branch.upstream_gone {
            value.push(("gone".into(), style(Role::Warning)));
        } else if ahead == 0 && behind == 0 {
            value.push(("in sync".into(), muted));
        }
        if ahead > 0 {
            value.push((format!("{ahead} ahead"), style(Role::DiffAdded)));
        }
        if behind > 0 {
            value.push((format!("{behind} behind"), style(Role::DiffRemoved)));
        }
        rows.push(Row::Field("Upstream:", value));
    } else {
        rows.push(Row::Field("Upstream:", vec![("none".into(), muted)]));
    }
    rows.push(Row::Field(
        "Updated:",
        vec![
            (
                crate::commit::relative_time(branch.timestamp, now),
                style(Role::Plain),
            ),
            (format!("by {}", branch.author), muted),
        ],
    ));
    rows.push(Row::Field(
        "Tip:",
        vec![
            (short(&branch.oid), style(Role::Accent)),
            (safe_text(&branch.subject), style(Role::Plain)),
        ],
    ));
    let Some(overview) = panel.overview else {
        return rows;
    };
    if let Some(base) = overview.base.as_ref().filter(|_| !branch.current) {
        rows.push(Row::Field(
            "Base:",
            vec![
                (short(&base.oid), style(Role::Accent)),
                (crate::commit::relative_time(base.timestamp, now), muted),
            ],
        ));
    }
    rows.push(Row::Blank);
    let against = &overview.against;
    rows.push(Row::Section(if against.is_empty() {
        "No upstream".into()
    } else if overview.total == 0 && branch.current {
        format!("Up to date with {against}")
    } else if overview.total == 0 {
        format!("Merged into {against}")
    } else {
        format!(
            "{} {} ahead of {against}",
            overview.total,
            if overview.total == 1 {
                "commit"
            } else {
                "commits"
            }
        )
    }));
    for (index, commit) in overview.commits.iter().enumerate() {
        rows.push(Row::Blank);
        rows.push(Row::Commit(index, commit, true));
        rows.push(Row::Commit(index, commit, false));
    }
    if overview.total > overview.commits.len() {
        rows.push(Row::Blank);
        rows.push(Row::More(overview.total - overview.commits.len()));
    }
    rows
}

fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

pub(super) fn panel_height(panel: &BranchPanel<'_>) -> usize {
    rows(panel, now()).len() + 1
}

#[derive(Default)]
pub(super) struct Panel {
    pub rows: Vec<(usize, Rect)>,
    pub total: usize,
    pub scroll_max: usize,
}

pub(super) struct Scroll<'a> {
    pub offset: &'a mut usize,
    pub manual: bool,
    pub focused: bool,
}

pub(super) fn render_panel(
    buffer: &mut Buffer,
    area: Rect,
    state: &mut ViewState,
    panel: &BranchPanel<'_>,
    scroll: Scroll<'_>,
) -> Panel {
    let now = now();
    let rows = rows(panel, now);
    let commits = panel.overview.map_or(0, |overview| overview.commits.len());
    state.branch_cursor = state.branch_cursor.min(commits.saturating_sub(1));
    let viewport = usize::from(area.height);
    let maximum = rows.len().saturating_sub(viewport);
    if !scroll.manual
        && scroll.focused
        && let Some(cursor) = rows.iter().position(
            |row| matches!(row, Row::Commit(index, _, true) if *index == state.branch_cursor),
        )
    {
        let first = if state.branch_cursor == 0 {
            rows.iter()
                .position(|row| matches!(row, Row::Section(_)))
                .unwrap_or(cursor)
        } else {
            cursor
        };
        *scroll.offset = (*scroll.offset).min(first);
        if cursor + 2 > *scroll.offset + viewport {
            *scroll.offset = cursor + 2 - viewport.min(cursor + 2);
        }
    }
    *scroll.offset = (*scroll.offset).min(maximum);
    let offset = *scroll.offset;
    let mut geometry = Panel {
        total: rows.len(),
        scroll_max: maximum,
        ..Panel::default()
    };
    if area.width < 12 || area.is_empty() {
        return geometry;
    }
    let inner = Rect::new(
        area.x + INSET,
        area.y,
        area.width.saturating_sub(INSET * 2),
        area.height,
    );
    for (line, row) in rows.iter().enumerate().skip(offset).take(viewport) {
        let y = area.y + (line - offset) as u16;
        let line_area = Rect::new(inner.x, y, inner.width, 1);
        match row {
            Row::Title => title(buffer, line_area, panel.branch),
            Row::Field(label, value) => {
                let label_width = LABEL_WIDTH.min(inner.width);
                put(buffer, inner.x, y, label_width, label, style(Role::Muted));
                left_spans(
                    buffer,
                    Rect::new(
                        inner.x + label_width,
                        y,
                        inner.width.saturating_sub(label_width),
                        1,
                    ),
                    &value
                        .iter()
                        .enumerate()
                        .map(|(index, (text, appearance))| {
                            (
                                if index > 0 {
                                    format!("  {text}")
                                } else {
                                    text.clone()
                                },
                                *appearance,
                            )
                        })
                        .collect::<Vec<_>>(),
                    Color::Reset,
                );
            }
            Row::Section(text) => put(
                buffer,
                inner.x,
                y,
                inner.width,
                &elide_end(text, usize::from(inner.width)),
                style(Role::Muted).add_modifier(Modifier::BOLD),
            ),
            Row::More(count) => put(
                buffer,
                inner.x,
                y,
                inner.width,
                &format!("{count} more"),
                style(Role::Muted),
            ),
            Row::Commit(index, commit, first) => {
                let selected = *index == state.branch_cursor && scroll.focused;
                let appearance = navigator::row_style(selected, selected, false);
                let card = Rect::new(
                    area.x + MARGIN,
                    y,
                    area.width.saturating_sub(MARGIN * 2 + 1),
                    1,
                );
                buffer.set_style(card, appearance);
                let text = Rect::new(
                    card.x + PADDING,
                    y,
                    card.width.saturating_sub(PADDING * 2),
                    1,
                );
                if *first {
                    let mut line = subject::list_line(&commit.subject, text.width);
                    for span in &mut line.spans {
                        span.style = span.style.bg(appearance.bg.unwrap_or(Color::Reset));
                        if selected {
                            span.style = span.style.add_modifier(Modifier::BOLD);
                        }
                    }
                    line.render(text, buffer);
                } else {
                    history::metadata(
                        buffer,
                        text,
                        &CommitSummary {
                            oid: commit.oid.clone(),
                            parents: Vec::new(),
                            author: commit.author.clone(),
                            timestamp: commit.timestamp,
                        },
                        now,
                        appearance,
                    );
                }
                match geometry.rows.iter_mut().find(|(row, _)| row == index) {
                    Some((_, rect)) => rect.height += 1,
                    None => geometry
                        .rows
                        .push((*index, Rect::new(area.x, y, area.width, 1))),
                }
            }
            Row::Blank => {}
        }
    }
    geometry
}

fn title(buffer: &mut Buffer, area: Rect, branch: &Branch) {
    let divergence = divergence(branch);
    let used = right_spans(buffer, area, &divergence, Color::Reset);
    let width = area.width.saturating_sub(used + u16::from(used > 0) * 2);
    let appearance = if branch.current {
        style(Role::Accent)
    } else {
        style(Role::Plain)
    }
    .add_modifier(Modifier::BOLD);
    let x = if branch.current && width > 2 {
        put(buffer, area.x, area.y, 1, "●", appearance);
        area.x + 2
    } else {
        area.x
    };
    let width = width.saturating_sub(x - area.x);
    put(
        buffer,
        x,
        area.y,
        width,
        &elide_middle(&branch.name, usize::from(width)),
        appearance,
    );
}
