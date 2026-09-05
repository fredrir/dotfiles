use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use super::model::{Mode, PreviewState, ToolbarItem};
use super::{
    AgentFilter, Model, OriginFilter, PickerAction, PickerOptions, PreviewDensity, ScopeFilter,
    SessionEntry, clean,
};

const SIDE_BY_SIDE_WIDTH: u16 = 92;
const FULL_CARD_WIDTH: u16 = 66;
const FULL_CARD_HEIGHT: u16 = 5;
const COMPACT_CARD_HEIGHT: u16 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HitTarget {
    Toolbar(ToolbarItem),
    Session(usize),
    List,
    PreviewText,
    Preview,
    Issues,
    None,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Regions {
    pub(crate) header: Rect,
    pub(crate) list: Rect,
    pub(crate) preview: Rect,
    pub(crate) footer: Rect,
}

pub(crate) fn layout(area: Rect) -> Regions {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4.min(area.height)),
            Constraint::Min(0),
            Constraint::Length(1.min(area.height.saturating_sub(4))),
        ])
        .split(area);
    let mut regions = Regions {
        header: vertical[0],
        list: vertical[1],
        preview: Rect::default(),
        footer: vertical[2],
    };
    let content = vertical[1];
    if content.is_empty() {
        return regions;
    }

    if area.width >= SIDE_BY_SIDE_WIDTH {
        let target = (content.width.saturating_mul(58) / 100)
            .max(40)
            .min(content.width.saturating_sub(32));
        let preview_width = target;
        let gap = u16::from(preview_width > 0 && content.width > preview_width);
        regions.list.width = content.width.saturating_sub(preview_width + gap);
        regions.preview = Rect::new(
            content.right().saturating_sub(preview_width),
            content.y,
            preview_width,
            content.height,
        );
    } else {
        let available = content
            .height
            .saturating_sub(COMPACT_CARD_HEIGHT.saturating_add(1));
        let target = if available >= 3 {
            (content.height.saturating_mul(58) / 100)
                .max(3)
                .min(available)
        } else {
            0
        };
        let gap = u16::from(target > 0 && content.height > target);
        let proposed_list_height = content.height.saturating_sub(target + gap);
        let proposed_list = Rect::new(content.x, content.y, content.width, proposed_list_height);
        regions.list.height = if target > 0 {
            let card_height = card_height(proposed_list) as u16;
            proposed_list_height / card_height * card_height
        } else {
            proposed_list_height
        };
        let preview_height = content
            .height
            .saturating_sub(regions.list.height.saturating_add(gap));
        regions.preview = Rect::new(
            content.x,
            content.bottom().saturating_sub(preview_height),
            content.width,
            preview_height,
        );
    }
    regions
}

fn ease_in_out(progress: f32) -> f32 {
    progress * progress * (3.0 - 2.0 * progress)
}

fn animated_extent(target: u16, progress: f32) -> u16 {
    ((f32::from(target) * progress).round() as u16)
        .max(1)
        .min(target)
}

fn compact_cards(list_area: Rect) -> bool {
    list_area.width < FULL_CARD_WIDTH || list_area.height < FULL_CARD_HEIGHT.saturating_mul(3)
}

fn card_height(list_area: Rect) -> usize {
    if compact_cards(list_area) {
        usize::from(COMPACT_CARD_HEIGHT)
    } else {
        usize::from(FULL_CARD_HEIGHT)
    }
}

pub(crate) fn list_page_size(area: Rect) -> usize {
    let list = layout(area).list;
    usize::from(list.height)
        .checked_div(card_height(list))
        .unwrap_or(0)
        .max(1)
}

pub(crate) fn preview_text_area(model: &Model) -> Rect {
    if model.preview_entry().is_none() || model.preview_progress() <= f32::EPSILON {
        return Rect::default();
    }
    let inner = layout(model.area).preview.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    preview_reveal(inner, ease_in_out(model.preview_progress()))
}

pub(crate) fn hit_test(model: &Model, point: ratatui::layout::Position) -> HitTarget {
    let regions = layout(model.area);
    if let Some(columns) = toolbar_columns(toolbar_area(regions.header))
        && let Some((index, _)) = columns
            .iter()
            .enumerate()
            .find(|(_, area)| area.contains(point))
    {
        return HitTarget::Toolbar(
            [
                ToolbarItem::Search,
                ToolbarItem::Origin,
                ToolbarItem::Agent,
                ToolbarItem::Scope,
            ][index],
        );
    }
    if regions.list.contains(point) {
        let height = card_height(regions.list).max(1);
        let row = usize::from(point.y.saturating_sub(regions.list.y)) / height;
        let position = model.list_offset.saturating_add(row);
        let visible_rows = usize::from(regions.list.height)
            .checked_div(height)
            .unwrap_or(0)
            .max(1);
        if row < visible_rows && position < model.filtered.len() {
            return HitTarget::Session(position);
        }
        return HitTarget::List;
    }
    if preview_text_area(model).contains(point) {
        return HitTarget::PreviewText;
    }
    if regions.preview.contains(point) {
        return HitTarget::Preview;
    }
    if !model.warnings.is_empty()
        && regions.footer.contains(point)
        && point.x >= regions.footer.right().saturating_sub(8)
    {
        return HitTarget::Issues;
    }
    HitTarget::None
}

pub(crate) fn render(frame: &mut Frame<'_>, model: &Model, options: PickerOptions) {
    let area = frame.area();
    let regions = layout(area);
    frame.render_widget(Block::default().style(base(options)), area);
    render_header(frame, regions.header, model, options);
    if regions.list.width > 0 && regions.list.height > 0 {
        render_list(frame, regions.list, model, options);
    }
    if regions.preview.width > 0 && regions.preview.height > 0 {
        render_preview(frame, regions.preview, model, options);
    }
    render_text_selection(frame, model, options);
    render_footer(frame, regions.footer, model, options);
    match model.mode {
        Mode::Help => render_help(frame, area, model, options),
        Mode::Diagnostics => render_diagnostics(frame, area, model, options),
        Mode::Review => render_review(frame, area, model, options),
        _ => {}
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    if area.is_empty() {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let heading = format!(
        "Sessions  {} / {}",
        if model.filtered.is_empty() {
            0
        } else {
            model.selected + 1
        },
        model.filtered.len()
    );
    let mut spans = vec![Span::styled(
        format!(" {heading} "),
        accent(options).add_modifier(Modifier::BOLD),
    )];
    if let Some(error) = &model.fatal_error {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "Error: ",
            danger(options).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(clean(error), normal(options)));
        spans.push(Span::styled("  [r] retry", muted(options)));
    } else {
        if model.loading {
            spans.push(Span::raw("  "));
            spans.push(Span::styled("Updating…", muted(options)));
        } else if let Some(status) = &model.status {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(clean(status), muted(options)));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[0]);
    render_toolbar(frame, rows[1], model, options);
}

fn render_toolbar(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    if area.is_empty() {
        return;
    }
    let active = model.pane == super::model::Pane::List && model.mode == Mode::Browse;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if active && model.toolbar_focus.is_some() {
            accent(options)
        } else {
            dim(options)
        })
        .title(Span::styled(
            " Find sessions ",
            if active && model.toolbar_focus == Some(ToolbarItem::Search) {
                accent(options).add_modifier(Modifier::BOLD)
            } else {
                muted(options)
            },
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    let compact = inner.width < 86;
    let Some(columns) = toolbar_columns(area) else {
        return;
    };
    let search_focused = active && model.toolbar_focus == Some(ToolbarItem::Search);
    let search_text = if model.query.is_empty() && !search_focused {
        "Type to search".to_owned()
    } else {
        clean(&model.query)
    };
    let cursor = if search_focused { "▏" } else { "" };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("⌕ ", accent(options)),
            Span::styled(
                truncate_tail(
                    &search_text,
                    usize::from(columns[0].width).saturating_sub(2 + cursor.chars().count()),
                ),
                if model.query.is_empty() {
                    muted(options)
                } else {
                    normal(options)
                },
            ),
            Span::styled(cursor, accent(options)),
        ]))
        .style(if search_focused {
            selected_field(options)
        } else {
            normal(options)
        }),
        columns[0],
    );

    let filters = [
        (
            ToolbarItem::Origin,
            filter_label(
                if compact { "O" } else { "Origin" },
                origin_filter_label(model.origin_filter),
            ),
        ),
        (
            ToolbarItem::Agent,
            filter_label(
                if compact { "A" } else { "Agent" },
                agent_filter_label(model.agent_filter),
            ),
        ),
        (
            ToolbarItem::Scope,
            filter_label(
                if compact { "S" } else { "Scope" },
                scope_filter_label(model.scope_filter),
            ),
        ),
    ];
    for ((item, label), area) in filters.into_iter().zip(columns[1..].iter().copied()) {
        let focused = active && model.toolbar_focus == Some(item);
        frame.render_widget(
            Paragraph::new(label)
                .alignment(Alignment::Center)
                .style(if focused {
                    selected(options)
                } else {
                    muted(options)
                }),
            area,
        );
    }
}

fn toolbar_columns(header: Rect) -> Option<[Rect; 4]> {
    let inner = header.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    if inner.is_empty() {
        return None;
    }
    let filter_width = if inner.width < 86 { 11 } else { 17 };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(filter_width),
            Constraint::Length(filter_width),
            Constraint::Length(filter_width),
        ])
        .split(inner);
    Some([columns[0], columns[1], columns[2], columns[3]])
}

fn toolbar_area(header: Rect) -> Rect {
    Rect::new(
        header.x,
        header.y.saturating_add(1),
        header.width,
        header.height.saturating_sub(1),
    )
}

fn filter_label(name: &str, value: &str) -> String {
    format!("[{name} {value}]")
}

fn origin_filter_label(filter: OriginFilter) -> &'static str {
    match filter {
        OriginFilter::All => "All",
        OriginFilter::Local => "Local",
        OriginFilter::Remote => "Remote",
    }
}

fn agent_filter_label(filter: AgentFilter) -> &'static str {
    match filter {
        AgentFilter::All => "All",
        AgentFilter::Codex => "Codex",
        AgentFilter::Claude => "Claude",
    }
}

fn scope_filter_label(filter: ScopeFilter) -> &'static str {
    match filter {
        ScopeFilter::All => "All",
        ScopeFilter::CurrentProject => "Project",
        ScopeFilter::Favorites => "Starred",
    }
}

fn render_list(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    if model.loading && model.entries.is_empty() {
        frame.render_widget(
            Paragraph::new("Scanning session indexes…").style(muted(options)),
            area,
        );
        return;
    }
    if model.filtered.is_empty() {
        let text = if model.entries.is_empty() {
            "No sessions found. Press r to refresh."
        } else {
            "No sessions match. Press x to reset search and filters."
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(muted(options))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let compact = compact_cards(area);
    let height = card_height(area);
    let page_size = usize::from(area.height)
        .checked_div(height)
        .unwrap_or(0)
        .max(1);
    let end = model
        .list_offset
        .saturating_add(page_size)
        .min(model.filtered.len());
    let list_focused = model.mode == Mode::Browse
        && model.pane == super::model::Pane::List
        && model.toolbar_focus.is_none();
    for (visible, index) in model.filtered[model.list_offset..end].iter().enumerate() {
        let y = area.y.saturating_add((visible * height) as u16);
        let visible_height = area.bottom().saturating_sub(y).min(height as u16);
        let card_area = Rect::new(area.x, y, area.width, visible_height);
        render_session_card(
            frame,
            card_area,
            &model.entries[*index],
            model.list_offset + visible == model.selected,
            list_focused && model.list_offset + visible == model.selected,
            compact,
            options,
        );
    }
}

fn render_session_card(
    frame: &mut Frame<'_>,
    area: Rect,
    entry: &SessionEntry,
    is_highlighted: bool,
    is_focused: bool,
    compact: bool,
    options: PickerOptions,
) {
    if area.is_empty() {
        return;
    }
    let title_style = if is_focused {
        accent(options).add_modifier(Modifier::BOLD)
    } else {
        muted(options)
    };
    let mut card_title = vec![Span::styled("   ", title_style)];
    card_title.push(badge(
        agent_label(entry.agent.name()),
        agent_badge(entry.agent.name(), options),
        options,
    ));
    card_title.push(Span::raw("  "));
    card_title.push(Span::styled(clean(&entry.updated), muted(options)));
    if entry.favorite {
        card_title.push(Span::raw(" "));
        card_title.push(Span::styled("★", warning(options)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(if is_highlighted {
            selected_card(options)
        } else {
            normal(options)
        })
        .border_style(if is_focused {
            accent(options)
        } else {
            dim(options)
        })
        .title(Line::from(card_title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let cursor = if is_highlighted { "› " } else { "  " };
    let available = usize::from(inner.width).saturating_sub(cursor.chars().count());
    let title = truncate(&clean(&entry.title), available.max(1));
    let title_style = if entry.disabled_reason.is_some() {
        dim(options)
    } else if is_highlighted {
        normal(options).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        normal(options).add_modifier(Modifier::BOLD)
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(cursor, accent(options)),
        Span::styled(title, title_style),
    ])];
    let host = entry.host.as_deref().unwrap_or("This host");
    if compact {
        let location = format!(
            "  {}  {}  {}",
            clean(&entry.project),
            clean(host),
            clean(&entry.workspace),
        );
        lines.push(Line::from(vec![
            Span::styled("  ", muted(options)),
            Span::styled(
                truncate(&location[2..], usize::from(inner.width).saturating_sub(2)),
                muted(options),
            ),
        ]));
    } else {
        lines.push(labelled_line(
            "PROJECT",
            &clean(&entry.project),
            inner.width,
            options,
        ));
        lines.push(labelled_line(
            "LOCATION",
            &format!("{}  {}", clean(host), clean(&entry.workspace)),
            inner.width,
            options,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn labelled_line(
    label: &'static str,
    value: &str,
    width: u16,
    options: PickerOptions,
) -> Line<'static> {
    let prefix = format!("  {label:<10}");
    let available = usize::from(width).saturating_sub(prefix.chars().count());
    Line::from(vec![
        Span::styled(prefix, dim(options).add_modifier(Modifier::BOLD)),
        Span::styled(truncate(value, available), muted(options)),
    ])
}

fn badge(label: impl Into<String>, style: Style, options: PickerOptions) -> Span<'static> {
    let label = label.into();
    if options.color {
        Span::styled(format!(" {label} "), style.add_modifier(Modifier::BOLD))
    } else {
        Span::styled(format!("[{label}]"), style.add_modifier(Modifier::BOLD))
    }
}

fn agent_badge(name: &str, options: PickerOptions) -> Style {
    if !options.color {
        return Style::default().add_modifier(Modifier::BOLD);
    }
    match name {
        "codex" => Style::default()
            .fg(Color::Rgb(216, 180, 254))
            .bg(Color::Rgb(76, 29, 149)),
        "claude" => Style::default()
            .fg(Color::Rgb(254, 215, 170))
            .bg(Color::Rgb(124, 45, 18)),
        _ => Style::default()
            .fg(Color::Rgb(203, 213, 225))
            .bg(Color::Rgb(51, 65, 85)),
    }
}

fn agent_label(name: &str) -> &'static str {
    match name {
        "codex" => "Codex",
        "claude" => "Claude",
        _ => "Agent",
    }
}

fn density_label(density: PreviewDensity) -> &'static str {
    match density {
        PreviewDensity::Conversation => "Conversation",
        PreviewDensity::Compact => "Compact conversation",
        PreviewDensity::Metadata => "Details only",
    }
}

fn render_preview(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    let selected_session = model.preview_entry().is_some();
    let preview_focused =
        model.mode == Mode::Browse && model.pane == super::model::Pane::Preview && selected_session;
    let mut title = vec![Span::styled(
        " Preview ",
        if preview_focused {
            accent(options).add_modifier(Modifier::BOLD)
        } else {
            muted(options).add_modifier(Modifier::BOLD)
        },
    )];
    if selected_session {
        title.push(Span::styled(
            density_label(model.preview_density),
            muted(options),
        ));
        title.push(Span::raw(" "));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if preview_focused {
            accent(options)
        } else {
            dim(options)
        })
        .title(Line::from(title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let Some(entry) = model.preview_entry() else {
        let message_area = centered(inner, inner.width.saturating_sub(2), 3.min(inner.height));
        frame.render_widget(
            Paragraph::new(vec![Line::from(Span::styled(
                if model.loading {
                    "Scanning for a session to preview…"
                } else {
                    "No session is available to preview."
                },
                normal(options).add_modifier(Modifier::BOLD),
            ))])
            .alignment(Alignment::Center),
            message_area,
        );
        return;
    };
    let reveal = preview_reveal(inner, ease_in_out(model.preview_progress()));
    if reveal.is_empty() {
        return;
    }
    let host = entry.host.as_deref().unwrap_or("This host");
    let mut lines = vec![
        Line::from(Span::styled(
            clean(&entry.title),
            normal(options).add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        Line::from(vec![
            badge(
                agent_label(entry.agent.name()),
                agent_badge(entry.agent.name(), options),
                options,
            ),
            Span::raw("  "),
            badge(clean(&entry.updated), muted(options), options),
            Span::raw("  "),
            badge(clean(&entry.project), muted(options), options),
        ]),
        Line::default(),
        Line::from(vec![
            Span::styled("LOCATION   ", dim(options).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{}  {}", clean(host), clean(&entry.workspace)),
                normal(options),
            ),
        ]),
        Line::from(vec![
            Span::styled("SESSION    ", dim(options).add_modifier(Modifier::BOLD)),
            Span::styled(clean(&entry.id), dim(options)),
        ]),
        Line::default(),
    ];
    if let Some(reason) = &entry.disabled_reason {
        lines.push(Line::from(vec![
            Span::styled(
                "Unavailable: ",
                danger(options).add_modifier(Modifier::BOLD),
            ),
            Span::styled(clean(reason), normal(options)),
        ]));
        lines.push(Line::default());
    }
    if let Some(warning_text) = &entry.warning {
        lines.push(Line::from(vec![
            Span::styled("! ", warning(options).add_modifier(Modifier::BOLD)),
            Span::styled(clean(warning_text), muted(options)),
        ]));
        lines.push(Line::default());
    }
    if model.preview_density == PreviewDensity::Metadata {
        lines.push(Line::from(Span::styled(
            "Press v to show conversation text.",
            muted(options),
        )));
    } else {
        render_conversation(&mut lines, model, options);
    }
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let maximum = paragraph
        .line_count(reveal.width)
        .saturating_sub(usize::from(reveal.height));
    let scroll = usize::from(model.preview_scroll)
        .min(maximum)
        .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(paragraph.scroll((scroll, 0)), reveal);
}

fn preview_reveal(area: Rect, progress: f32) -> Rect {
    if area.is_empty() || progress <= f32::EPSILON {
        return Rect::default();
    }
    let width = animated_extent(area.width, progress);
    Rect::new(
        area.right().saturating_sub(width),
        area.y,
        width,
        area.height,
    )
}

fn render_text_selection(frame: &mut Frame<'_>, model: &Model, options: PickerOptions) {
    let Some(selection) = model.text_selection else {
        return;
    };
    let area = preview_text_area(model);
    if area.is_empty() {
        return;
    }
    let style = if options.color {
        Style::default()
            .fg(Color::Rgb(15, 23, 42))
            .bg(Color::Rgb(196, 181, 253))
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    };
    let buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let point = ratatui::layout::Position::new(x, y);
            if selection.contains(point, area)
                && let Some(cell) = buffer.cell_mut(point)
            {
                cell.set_style(style);
            }
        }
    }
}

fn render_conversation(lines: &mut Vec<Line<'static>>, model: &Model, options: PickerOptions) {
    match model.selected_preview() {
        Some(PreviewState::Loading) | None => lines.push(Line::from(Span::styled(
            "Loading transcript preview…",
            muted(options),
        ))),
        Some(PreviewState::Error(error)) => lines.push(Line::from(vec![
            Span::styled("Preview unavailable: ", warning(options)),
            Span::styled(clean(error), muted(options)),
        ])),
        Some(PreviewState::Ready(preview)) => {
            if let Some(preview_warning) = &preview.warning {
                lines.push(Line::from(vec![
                    Span::styled("! ", warning(options)),
                    Span::styled(clean(preview_warning), muted(options)),
                ]));
                lines.push(Line::default());
            }
            if preview.lines.is_empty() {
                lines.push(Line::from(Span::styled(
                    "No conversational text in this session.",
                    muted(options),
                )));
                return;
            }
            if preview.truncated {
                lines.push(Line::from(Span::styled(
                    "Showing the latest meaningful exchange",
                    muted(options),
                )));
                lines.push(Line::default());
            }
            let visible = if model.preview_density == PreviewDensity::Compact {
                preview.lines.len().saturating_sub(2)..preview.lines.len()
            } else {
                0..preview.lines.len()
            };
            for item in &preview.lines[visible] {
                let role_style = match item.role {
                    super::PreviewRole::User => success(options),
                    super::PreviewRole::Assistant => accent(options),
                };
                lines.push(Line::from(Span::styled(
                    format!("{}:", item.role.label()),
                    role_style.add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(clean(&item.text), normal(options))));
                lines.push(Line::default());
            }
        }
    }
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    if area.is_empty() {
        return;
    }
    let hints = if model.preview_actions_enabled() {
        "[←→] sessions   [Space] actions   [?] shortcuts"
    } else if model.toolbar_focus == Some(ToolbarItem::Search) {
        "Search active   [←→] filters   [↓] sessions"
    } else if model.toolbar_focus.is_some() {
        "[←→] choose filter   [Enter] change   [↓] sessions"
    } else {
        "[/] search   [←→] preview   [Enter] select"
    };
    let issue_width = if model.warnings.is_empty() { 0 } else { 8 };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(issue_width)])
        .split(area);
    frame.render_widget(Paragraph::new(hints).style(muted(options)), columns[0]);
    if issue_width > 0 {
        frame.render_widget(
            Paragraph::new(format!("! {}", model.warnings.len()))
                .style(warning(options))
                .alignment(Alignment::Right),
            columns[1],
        );
    }
}

fn render_help(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    let popup = centered(
        area,
        72.min(area.width.saturating_sub(2)),
        24.min(area.height.saturating_sub(2)),
    );
    frame.render_widget(Clear, popup);
    let text = Text::from(vec![
        Line::from(Span::styled(
            "Search and filter",
            accent(options).add_modifier(Modifier::BOLD),
        )),
        Line::from("  /                     focus search from anywhere"),
        Line::from("  Type                  filter after focusing search"),
        Line::from("  ←/→ or click; Enter/Space changes a filter"),
        Line::from("  ↓                     enter the session list"),
        Line::default(),
        Line::from(Span::styled(
            "Browse sessions",
            accent(options).add_modifier(Modifier::BOLD),
        )),
        Line::from("  ↑/↓, j/k, click      choose a session"),
        Line::from("  PageUp/Down, Home/End page or first/last"),
        Line::from("  ←/→                   move focus between list and preview"),
        Line::from("  Enter                 select highlighted session for preview"),
        Line::from("  r                     refresh local + remote"),
        Line::default(),
        Line::from(Span::styled(
            "Selected session preview",
            accent(options).add_modifier(Modifier::BOLD),
        )),
        Line::from("  Space                 show transfer actions"),
        Line::from("  f favorite   y copy complete session details   v detail"),
        Line::from("  ↑/↓, j/k scroll       ←/→ focus the session list"),
        Line::from("  ! / w                 inspect actionable issues"),
        Line::from("  Ctrl-C                cancel from anywhere"),
        Line::default(),
        Line::from(Span::styled(
            "Drag inside Preview to select and copy only that text.",
            muted(options),
        )),
    ]);
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(accent(options)),
        )
        .wrap(Wrap { trim: false });
    let scroll = overlay_scroll(&paragraph, popup, model.overlay_scroll);
    frame.render_widget(paragraph.scroll((scroll, 0)), popup);
}

fn render_diagnostics(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    let popup = centered(
        area,
        82.min(area.width.saturating_sub(2)),
        22.min(area.height.saturating_sub(2)),
    );
    frame.render_widget(Clear, popup);
    let mut lines = vec![Line::from(Span::styled(
        "Actionable catalog issues",
        warning(options).add_modifier(Modifier::BOLD),
    ))];
    for (index, item) in model.warnings.iter().enumerate() {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled(format!("{}. ", index + 1), muted(options)),
            Span::styled(clean(item), normal(options)),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "[↑↓] scroll   [!] [w] [Enter] [Esc] close",
        muted(options),
    )));
    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Issues ")
                .borders(Borders::ALL)
                .border_style(warning(options)),
        )
        .wrap(Wrap { trim: false });
    let scroll = overlay_scroll(&paragraph, popup, model.overlay_scroll);
    frame.render_widget(paragraph.scroll((scroll, 0)), popup);
}

fn overlay_scroll(paragraph: &Paragraph<'_>, area: Rect, requested: u16) -> u16 {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    // `line_count` accounts for the block's vertical borders, but wrapping
    // still needs the width actually available inside its horizontal borders.
    let maximum = paragraph
        .line_count(inner.width)
        .saturating_sub(usize::from(area.height));
    usize::from(requested)
        .min(maximum)
        .min(usize::from(u16::MAX)) as u16
}

fn render_review(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    let popup = review_area(area);
    frame.render_widget(Clear, popup);
    let Some(entry) = model.preview_entry() else {
        return;
    };
    let block = Block::default()
        .title(" Transfer actions ")
        .borders(Borders::ALL)
        .border_style(accent(options));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if !review_can_apply(area) {
        frame.render_widget(
            Paragraph::new("Terminal too small to apply\nResize or press Esc")
                .style(warning(options))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    if popup.height < 10 {
        let action = PickerAction::ALL[model.review_action].label();
        let lines = if inner.height >= 3 {
            vec![
                Line::from(Span::styled(
                    format!("Apply: {action}"),
                    selected(options).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    PickerAction::ALL[model.review_action].description(),
                    muted(options),
                )),
                Line::from("[Space] opened actions   [←→] choose   [Enter] apply   [Esc] back"),
            ]
        } else if inner.height >= 2 {
            vec![
                Line::from(Span::styled(
                    format!("Apply: {action}"),
                    selected(options).add_modifier(Modifier::BOLD),
                )),
                Line::from("[←→] choose   [Enter] apply   [Esc] back"),
            ]
        } else {
            vec![Line::from(Span::styled(
                format!("Enter: {action}"),
                selected(options).add_modifier(Modifier::BOLD),
            ))]
        };
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                clean(&entry.title),
                normal(options).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!(
                    "{}   {}   {}",
                    agent_label(entry.agent.name()),
                    entry.origin.label(),
                    clean(&entry.project)
                ),
                muted(options),
            )),
        ]),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            PickerAction::ALL[model.review_action].description(),
            muted(options),
        )),
        rows[1],
    );
    let buttons = review_buttons(rows[2]);
    for (index, button) in buttons.into_iter().enumerate() {
        let label = PickerAction::ALL[index].label();
        frame.render_widget(
            Paragraph::new(label)
                .alignment(Alignment::Center)
                .style(if index == model.review_action {
                    selected(options)
                } else {
                    normal(options)
                })
                .block(Block::default().borders(Borders::ALL)),
            button,
        );
    }
    frame.render_widget(
        Paragraph::new("[Space] opened actions   [←→] choose   [Enter] apply   [Esc] back")
            .style(muted(options)),
        rows[3],
    );
}

pub(crate) fn review_can_apply(area: Rect) -> bool {
    area.width >= 18 && area.height >= 5
}

fn review_buttons(area: Rect) -> [Rect; 3] {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);
    [columns[0], columns[1], columns[2]]
}

fn review_area(area: Rect) -> Rect {
    centered(
        area,
        76.min(area.width.saturating_sub(2)),
        13.min(area.height.saturating_sub(2)),
    )
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.max(1).min(area.width);
    let height = height.max(1).min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn truncate(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let mut result = value.chars().take(width - 1).collect::<String>();
    result.push('…');
    result
}

fn truncate_tail(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let tail = value
        .chars()
        .skip(count.saturating_sub(width - 1))
        .collect::<String>();
    format!("…{tail}")
}

fn base(options: PickerOptions) -> Style {
    if options.color {
        Style::default()
            .bg(Color::Rgb(8, 12, 22))
            .fg(Color::Rgb(226, 232, 240))
    } else {
        Style::default()
    }
}

fn normal(options: PickerOptions) -> Style {
    if options.color {
        Style::default().fg(Color::Rgb(226, 232, 240))
    } else {
        Style::default()
    }
}

fn muted(options: PickerOptions) -> Style {
    if options.color {
        Style::default().fg(Color::Rgb(148, 163, 184))
    } else {
        Style::default().add_modifier(Modifier::DIM)
    }
}

fn dim(options: PickerOptions) -> Style {
    if options.color {
        Style::default().fg(Color::Rgb(71, 85, 105))
    } else {
        Style::default().add_modifier(Modifier::DIM)
    }
}

fn accent(options: PickerOptions) -> Style {
    if options.color {
        Style::default().fg(Color::Rgb(167, 139, 250))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn success(options: PickerOptions) -> Style {
    if options.color {
        Style::default().fg(Color::Rgb(52, 211, 153))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn warning(options: PickerOptions) -> Style {
    if options.color {
        Style::default().fg(Color::Rgb(250, 204, 21))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn danger(options: PickerOptions) -> Style {
    if options.color {
        Style::default().fg(Color::Rgb(248, 113, 113))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn selected(options: PickerOptions) -> Style {
    if options.color {
        Style::default()
            .fg(Color::Rgb(15, 23, 42))
            .bg(Color::Rgb(196, 181, 253))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
}

fn selected_field(options: PickerOptions) -> Style {
    if options.color {
        Style::default().bg(Color::Rgb(21, 28, 48))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn selected_card(options: PickerOptions) -> Style {
    if options.color {
        Style::default()
            .fg(Color::Rgb(241, 245, 249))
            .bg(Color::Rgb(21, 28, 48))
    } else {
        Style::default()
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tui/view_tests.rs"]
mod tests;
