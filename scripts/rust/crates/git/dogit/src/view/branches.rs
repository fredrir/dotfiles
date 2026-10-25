use super::*;
use crate::git::{Branch, Commit};

const MARGIN: u16 = 1;
const PADDING: u16 = 1;
const ITEM_HEIGHT: usize = 4;
const HEADER_HEIGHT: usize = 2;
const CARD_HEIGHT: u16 = 3;
const NAME_MIN: u16 = 8;
const INSET: u16 = 2;
const RAIL_GAP: u16 = 2;
const LABEL_WIDTH: u16 = 10;
const UPSTREAM_AHEAD: &str = "\u{f062}";
const UPSTREAM_BEHIND: &str = "\u{f063}";

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
            usize::from(row == offset || starts_group(items, row)) * HEADER_HEIGHT
                + if row < index { ITEM_HEIGHT } else { 0 }
        })
        .sum()
}

fn scroll_max(items: &[NavItem], height: u16) -> usize {
    let height = usize::from(height);
    let mut rows = 0;
    for index in (0..items.len()).rev() {
        rows += ITEM_HEIGHT + usize::from(starts_group(items, index)) * HEADER_HEIGHT;
        let sticky = usize::from(!starts_group(items, index)) * HEADER_HEIGHT;
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
            && content_top(items, state.offset, state.selected) + usize::from(CARD_HEIGHT)
                > usize::from(area.height)
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
            header(buffer, Rect::new(area.x, y, area.width, 1), branch);
            y = y.saturating_add(HEADER_HEIGHT as u16);
        }
        if y >= area.bottom() {
            break;
        }
        let height = CARD_HEIGHT.min(area.bottom() - y);
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
        render_card(buffer, card, branch, appearance, selected, now);
        let gap = y.saturating_add(CARD_HEIGHT);
        if gap < area.bottom() && index + 1 < items.len() && !starts_group(items, index + 1) {
            divider(buffer, Rect::new(area.x, gap, area.width, 1));
        }
        y = y.saturating_add(ITEM_HEIGHT as u16);
    }
    navigation
}

fn header(buffer: &mut Buffer, area: Rect, branch: &Branch) {
    ruled_title(
        buffer,
        list_rule(area),
        &[Span::styled(
            chrome::identity_label(group(branch)),
            style(Role::Plain).add_modifier(Modifier::BOLD),
        )],
    );
}

fn divider(buffer: &mut Buffer, area: Rect) {
    let rule = list_rule(area);
    put(
        buffer,
        rule.x,
        rule.y,
        rule.width,
        &"─".repeat(usize::from(rule.width)),
        navigator::directory_guide_style(Color::Reset),
    );
}

fn list_rule(area: Rect) -> Rect {
    Rect::new(
        area.x + MARGIN + PADDING,
        area.y,
        area.width.saturating_sub(MARGIN * 2 + PADDING + 1),
        1,
    )
}

fn ruled_title(buffer: &mut Buffer, area: Rect, title: &[Span<'_>]) {
    let used = render_spans(buffer, area, title, Color::Reset);
    let rule = area.width.saturating_sub(used + 1);
    put(
        buffer,
        area.x + used + 1,
        area.y,
        rule,
        &"─".repeat(usize::from(rule)),
        style(Role::Border),
    );
}

fn render_card(
    buffer: &mut Buffer,
    area: Rect,
    branch: &Branch,
    appearance: Style,
    selected: bool,
    now: i64,
) {
    let text = Rect::new(
        area.x + PADDING,
        area.y,
        area.width.saturating_sub(PADDING * 2),
        area.height,
    );
    if text.width < 4 {
        return;
    }
    let background = appearance.bg.unwrap_or(Color::Reset);
    let room = Rect::new(
        text.x + NAME_MIN + 1,
        text.y,
        text.width.saturating_sub(NAME_MIN + 1),
        1,
    );
    let badges = chrome::render_badges(buffer, room, relation(branch), false);
    let name_width = if badges.width > 0 {
        badges.x.saturating_sub(text.x + 2)
    } else {
        text.width
    };
    let name = label(branch);
    put(
        buffer,
        text.x,
        text.y,
        name_width,
        &elide_middle(name, usize::from(name_width)),
        chrome::identity_text_style(name).bg(background),
    );
    if text.height > 1 {
        let mut line = subject::list_line(&branch.subject, text.width);
        paint(&mut line, appearance, selected);
        line.render(Rect::new(text.x, text.y + 1, text.width, 1), buffer);
    }
    if text.height > 2 {
        history::metadata(
            buffer,
            Rect::new(text.x, text.y + 2, text.width, 1),
            &CommitSummary {
                oid: branch.oid.clone(),
                parents: Vec::new(),
                author: branch.author.clone(),
                timestamp: branch.timestamp,
            },
            &upstream_sync(branch),
            now,
            appearance,
        );
    }
}

fn paint(line: &mut Line<'_>, appearance: Style, selected: bool) {
    for span in &mut line.spans {
        span.style = span
            .style
            .bg(appearance.bg.unwrap_or(Color::Reset))
            .add_modifier(appearance.add_modifier);
        if selected {
            span.style = span.style.add_modifier(Modifier::BOLD);
        }
    }
}

fn merged(branch: &Branch) -> bool {
    !branch.current && matches!(branch.divergence, Some((0, behind)) if behind > 0)
}

fn relation(branch: &Branch) -> Vec<Badge> {
    if branch.current {
        return vec![Badge::new("HEAD".into(), style(Role::Accent))];
    }
    if merged(branch) {
        return vec![Badge::dim("Merged".into())];
    }
    let (ahead, behind) = branch.divergence.unwrap_or_default();
    [
        (ahead, "↑", Role::DiffAdded),
        (behind, "↓", Role::DiffRemoved),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, arrow, role)| Badge::new(format!("{arrow}{count}"), style(role)))
    .collect()
}

fn upstream_sync(branch: &Branch) -> Vec<Span<'static>> {
    if branch.remote || branch.upstream.is_none() {
        return Vec::new();
    }
    if branch.upstream_gone {
        return vec![Span::styled("gone", style(Role::Warning))];
    }
    let (ahead, behind) = branch.tracking;
    [
        (ahead, UPSTREAM_AHEAD, Role::DiffAdded),
        (behind, UPSTREAM_BEHIND, Role::DiffRemoved),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, arrow, role)| Span::styled(format!("{count} {arrow}"), style(role)))
    .collect()
}

fn identity(name: &str) -> Style {
    chrome::identity_text_style(name).remove_modifier(Modifier::BOLD)
}

fn reference_spans(name: &str, remote: Option<&str>, modifier: Modifier) -> Vec<Span<'static>> {
    let split = remote.and_then(|remote| {
        name.strip_prefix(remote)?
            .strip_prefix('/')
            .map(|rest| (remote, rest))
    });
    match split {
        Some((remote, rest)) => vec![
            Span::styled(remote.to_owned(), identity(remote).add_modifier(modifier)),
            Span::styled("/", style(Role::Muted)),
            Span::styled(rest.to_owned(), identity(rest).add_modifier(modifier)),
        ],
        None => vec![Span::styled(
            name.to_owned(),
            identity(name).add_modifier(modifier),
        )],
    }
}

fn render_spans(buffer: &mut Buffer, area: Rect, spans: &[Span<'_>], background: Color) -> u16 {
    let mut x = area.x;
    for span in spans {
        let available = area.right().saturating_sub(x);
        if available == 0 {
            break;
        }
        let text = elide_end(&span.content, usize::from(available));
        let width = text.width() as u16;
        put(buffer, x, area.y, width, &text, span.style.bg(background));
        x += width;
    }
    x - area.x
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64)
}

enum Row<'a> {
    Title,
    Field(&'static str, Vec<Span<'static>>, Vec<Span<'static>>),
    Blank,
    Section(Vec<Span<'static>>),
    Subject(usize, &'a Commit, Line<'static>, bool),
    Metadata(usize, &'a Commit),
    Gap,
    More(usize),
}

fn rows<'a>(panel: &BranchPanel<'a>, width: u16, now: i64) -> Vec<Row<'a>> {
    let branch = panel.branch;
    let mut rows = vec![Row::Title, Row::Blank];
    if !branch.remote {
        rows.push(Row::Field("Upstream:", upstream_field(branch), Vec::new()));
    }
    let Some(overview) = panel.overview else {
        return rows;
    };
    if let Some(base) = overview.base.as_ref().filter(|base| base.oid != branch.oid) {
        let mut value = vec![
            Span::styled(short(&base.oid), style(Role::Accent)),
            Span::raw("  "),
        ];
        value.extend(subject::list_line(&base.subject, u16::MAX).spans);
        rows.push(Row::Field(
            "Base:",
            value,
            vec![Span::styled(
                crate::commit::compact_relative_time(base.timestamp, now),
                style(Role::Muted),
            )],
        ));
    }
    if rows.len() > 2 {
        rows.push(Row::Blank);
    }
    rows.push(Row::Section(section(branch, overview)));
    rows.push(Row::Blank);
    let text_width = card(Rect::new(0, 0, width, 1)).width;
    for (index, commit) in overview.commits.iter().enumerate() {
        if index > 0 {
            rows.push(Row::Gap);
        }
        for (line, subject) in subject::list_lines(&commit.subject, text_width)
            .into_iter()
            .enumerate()
        {
            rows.push(Row::Subject(index, commit, subject, line == 0));
        }
        rows.push(Row::Metadata(index, commit));
    }
    if overview.total > overview.commits.len() {
        rows.push(Row::Blank);
        rows.push(Row::More(overview.total - overview.commits.len()));
    }
    rows
}

fn upstream_field(branch: &Branch) -> Vec<Span<'static>> {
    let Some(upstream) = &branch.upstream else {
        return vec![Span::styled("none", style(Role::Muted))];
    };
    let mut spans = reference_spans(
        upstream,
        branch.upstream_remote.as_deref(),
        Modifier::empty(),
    );

    let status = if branch.upstream_gone {
        vec![("gone".to_owned(), Role::Warning)]
    } else {
        Vec::new()
    };
    for (text, role) in status {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(text, style(role)));
    }
    spans
}

fn section(branch: &Branch, overview: &BranchOverview) -> Vec<Span<'static>> {
    let title = style(Role::Plain).add_modifier(Modifier::BOLD);
    if overview.total == 0 {
        return vec![Span::styled("Recent commits", title)];
    }
    let noun = if overview.total == 1 {
        "commit"
    } else {
        "commits"
    };
    let mut spans = vec![Span::styled(
        format!("{} {noun} ahead of ", overview.total),
        title,
    )];
    spans.extend(reference_spans(
        &overview.against,
        branch.upstream_remote.as_deref().filter(|_| branch.current),
        Modifier::BOLD,
    ));
    spans
}

fn header_badges(panel: &BranchPanel<'_>, repo: &str) -> Vec<Badge> {
    if let Some(stat) = panel
        .overview
        .and_then(|overview| overview.stat)
        .filter(|stat| stat.files > 0)
    {
        let noun = if stat.files == 1 { "File" } else { "Files" };
        return vec![
            Badge::new(format!("{} {noun}", stat.files), chrome::files_style(repo)),
            Badge::new(format!("+{}", stat.added), style(Role::DiffAdded)),
            Badge::new(format!("-{}", stat.removed), style(Role::DiffRemoved)),
        ];
    }
    if merged(panel.branch) {
        return vec![Badge::dim("Merged".into())];
    }
    Vec::new()
}

fn short(oid: &str) -> String {
    oid.chars().take(7).collect()
}

fn card(row: Rect) -> Rect {
    let x = row.x + INSET + RAIL_GAP + PADDING;
    Rect::new(
        x,
        row.y,
        row.right().saturating_sub(x + MARGIN + 1 + PADDING),
        1,
    )
}

pub(super) fn panel_height(panel: &BranchPanel<'_>, width: u16) -> usize {
    rows(panel, width, now()).len() + 1
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
    repo: &str,
    scroll: Scroll<'_>,
) -> Panel {
    let now = now();
    let rows = rows(panel, area.width, now);
    let commits = panel.overview.map_or(0, |overview| overview.commits.len());
    state.branch_cursor = state.branch_cursor.min(commits.saturating_sub(1));
    let viewport = usize::from(area.height);
    let maximum = rows.len().saturating_sub(viewport);
    let cursor = state.branch_cursor;
    if !scroll.manual
        && scroll.focused
        && let Some(top) = rows
            .iter()
            .position(|row| matches!(row, Row::Subject(index, ..) if *index == cursor))
    {
        let end = rows
            .iter()
            .position(|row| matches!(row, Row::Metadata(index, _) if *index == cursor))
            .map_or(top + 1, |row| row + 1);
        let first = if cursor == 0 {
            rows.iter()
                .position(|row| matches!(row, Row::Section(_)))
                .unwrap_or(top)
        } else {
            top
        };
        *scroll.offset = (*scroll.offset).min(first);
        if end > *scroll.offset + viewport {
            *scroll.offset = end - viewport.min(end);
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
    let rail = identity(label(panel.branch));
    for (line, row) in rows.iter().enumerate().skip(offset).take(viewport) {
        let y = area.y + (line - offset) as u16;
        let line_area = Rect::new(inner.x, y, inner.width, 1);
        let text = card(Rect::new(area.x, y, area.width, 1));
        match row {
            Row::Title => title(
                buffer,
                Rect::new(inner.x, y, area.right().saturating_sub(inner.x + 1), 1),
                panel,
                repo,
            ),
            Row::Field(label, value, aside) => {
                let label_width = LABEL_WIDTH.min(inner.width);
                put(buffer, inner.x, y, label_width, label, style(Role::Muted));
                let aside_width = aside.iter().map(Span::width).sum::<usize>() as u16;
                let aside_width = if aside_width + 2 < inner.width.saturating_sub(label_width) / 2 {
                    render_spans(
                        buffer,
                        Rect::new(inner.right() - aside_width, y, aside_width, 1),
                        aside,
                        Color::Reset,
                    ) + 2
                } else {
                    0
                };
                render_spans(
                    buffer,
                    Rect::new(
                        inner.x + label_width,
                        y,
                        inner.width.saturating_sub(label_width + aside_width),
                        1,
                    ),
                    value,
                    Color::Reset,
                );
            }
            Row::Section(spans) => ruled_title(buffer, line_area, spans),
            Row::More(count) => put(
                buffer,
                text.x,
                y,
                text.width,
                &format!("{count} more"),
                style(Role::Muted),
            ),
            Row::Gap => put(buffer, inner.x, y, 1, "│", rail),
            Row::Subject(index, commit, _, _) | Row::Metadata(index, commit) => {
                let selected = *index == cursor && scroll.focused;
                let appearance = navigator::row_style(selected, selected, false);
                let card_area = Rect::new(text.x - PADDING, y, text.width + PADDING * 2, 1);
                buffer.set_style(card_area, appearance);
                match row {
                    Row::Subject(_, _, subject, true) => {
                        let node = if selected {
                            rail.fg(subject::list_label_color(&commit.subject))
                        } else {
                            rail
                        };
                        put(
                            buffer,
                            inner.x,
                            y,
                            1,
                            "●",
                            node.add_modifier(Modifier::BOLD),
                        );
                        let mut subject = subject.clone();
                        paint(&mut subject, appearance, selected);
                        subject.render(text, buffer);
                    }
                    Row::Subject(_, _, subject, false) => {
                        put(buffer, inner.x, y, 1, "│", rail);
                        let mut subject = subject.clone();
                        paint(&mut subject, appearance, selected);
                        subject.render(text, buffer);
                    }
                    _ => {
                        put(buffer, inner.x, y, 1, "│", rail);
                        history::metadata(
                            buffer,
                            text,
                            &CommitSummary {
                                oid: commit.oid.clone(),
                                parents: Vec::new(),
                                author: commit.author.clone(),
                                timestamp: commit.timestamp,
                            },
                            &[],
                            now,
                            appearance,
                        );
                    }
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

fn title(buffer: &mut Buffer, area: Rect, panel: &BranchPanel<'_>, repo: &str) {
    let branch = panel.branch;
    let name = label(branch);
    let remote = branch.remote_parts().map(|(remote, _)| remote);
    let badges = header_badges(panel, repo);
    let badges_width = badges
        .iter()
        .map(|badge| badge.width() as u16 + 1)
        .sum::<u16>();
    let remote_width = remote.map_or(0, |remote| remote.width() as u16 + 3);
    let room = area
        .width
        .saturating_sub(badges_width + remote_width + 1)
        .max(area.width / 2);
    let mut end = area.x
        + badge(
            buffer,
            Rect::new(area.x, area.y, room, 1),
            &elide_middle(name, usize::from(room.saturating_sub(2))),
            chrome::identity_style(name),
        );
    if let Some(remote) = remote {
        let room = Rect::new(end + 1, area.y, area.right().saturating_sub(end + 1), 1);
        end = room.x + badge(buffer, room, remote, chrome::identity_style(remote));
    }
    chrome::render_badges(
        buffer,
        Rect::new(end + 2, area.y, area.right().saturating_sub(end + 2), 1),
        badges,
        false,
    );
}

fn badge(buffer: &mut Buffer, area: Rect, text: &str, appearance: Style) -> u16 {
    let width = (text.width() as u16 + 2).min(area.width);
    put(
        buffer,
        area.x,
        area.y,
        width,
        &format!(" {text} "),
        appearance,
    );
    width
}
