use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::model::{Mode, Pane, PreviewState};
use super::{Model, PickerAction, PickerOptions, PreviewDensity, SessionEntry, clean};

const WIDE: u16 = 110;
const TINY: u16 = 70;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Regions {
    pub(crate) header: Rect,
    pub(crate) banner: Rect,
    pub(crate) list: Rect,
    pub(crate) preview: Rect,
    pub(crate) footer: Rect,
}

pub(crate) fn layout(area: Rect, focused: Pane) -> Regions {
    let banner_height = 2.min(area.height.saturating_sub(1));
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3.min(area.height)),
            Constraint::Length(banner_height),
            Constraint::Min(3),
            Constraint::Length(2.min(area.height)),
        ])
        .split(area);
    let mut regions = Regions {
        header: vertical[0],
        banner: vertical[1],
        list: Rect::default(),
        preview: Rect::default(),
        footer: vertical[3],
    };
    if area.width < TINY || area.height < 16 {
        match focused {
            Pane::List => regions.list = vertical[2],
            Pane::Preview => regions.preview = vertical[2],
        }
    } else {
        let percentage = if area.width >= WIDE { 42 } else { 48 };
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(percentage),
                Constraint::Percentage(100 - percentage),
            ])
            .split(vertical[2]);
        regions.list = horizontal[0];
        regions.preview = horizontal[1];
    }
    regions
}

pub(crate) fn render(frame: &mut Frame<'_>, model: &Model, options: PickerOptions) {
    let area = frame.area();
    let regions = layout(area, model.pane);
    frame.render_widget(Block::default().style(base(options)), area);
    render_header(frame, regions.header, model, options);
    render_banner(frame, regions.banner, model, options);
    if regions.list.width > 0 && regions.list.height > 0 {
        render_list(frame, regions.list, model, options);
    }
    if regions.preview.width > 0 && regions.preview.height > 0 {
        render_preview(frame, regions.preview, model, options);
    }
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
    let title = Line::from(vec![
        Span::styled(" agent-hop ", accent(options).add_modifier(Modifier::BOLD)),
        Span::styled("session switcher", muted(options)),
    ]);
    let query = if model.mode == Mode::Search {
        format!("/{}▏", clean(&model.query))
    } else if model.query.is_empty() {
        "/ search".into()
    } else {
        format!("/{}", clean(&model.query))
    };
    let chips = format!(
        "[1 {}] [2 {}] [3 {}]",
        model.origin_filter.label(),
        model.agent_filter.label(),
        model.scope_filter.label()
    );
    let width = usize::from(area.width);
    let line = if width >= 88 {
        Line::from(vec![
            Span::styled(
                query,
                if model.mode == Mode::Search {
                    selected(options)
                } else {
                    normal(options)
                },
            ),
            Span::raw("  "),
            Span::styled(chips, muted(options)),
        ])
    } else if model.mode == Mode::Search {
        Line::from(Span::styled(query, normal(options)))
    } else if width >= 54 {
        Line::from(Span::styled(chips, muted(options)))
    } else {
        Line::from(Span::styled(query, normal(options)))
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    frame.render_widget(Paragraph::new(title), rows[0]);
    frame.render_widget(Paragraph::new(line), rows[1]);
}

fn render_banner(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    if area.is_empty() {
        return;
    }
    let line = if model.loading {
        let loading = if options.reduced_motion {
            " LOADING ".to_string()
        } else {
            const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
            let frame = usize::try_from((model.animation_frame / 4) % 8).unwrap_or(0);
            format!(" {} LOADING ", SPINNER[frame])
        };
        Line::from(vec![
            Span::styled(loading, accent(options).add_modifier(Modifier::BOLD)),
            Span::styled(
                if model.entries.is_empty() {
                    "Discovering local sessions…"
                } else {
                    "Local sessions ready; fetching remote sessions…"
                },
                normal(options),
            ),
        ])
    } else if let Some(error) = &model.fatal_error {
        Line::from(vec![
            Span::styled(" ERROR ", danger(options).add_modifier(Modifier::BOLD)),
            Span::styled(clean(error), normal(options)),
            Span::styled("  r retry", muted(options)),
        ])
    } else if let Some(status) = &model.status {
        Line::from(vec![
            Span::styled(" NOTE ", accent(options).add_modifier(Modifier::BOLD)),
            Span::styled(clean(status), normal(options)),
        ])
    } else if !model.warnings.is_empty() {
        let more = if model.warnings.len() > 1 {
            format!(" (+{} more)", model.warnings.len() - 1)
        } else {
            String::new()
        };
        Line::from(vec![
            Span::styled(" WARNING ", warning(options).add_modifier(Modifier::BOLD)),
            Span::styled(clean(&model.warnings[0]), normal(options)),
            Span::styled(more, muted(options)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" READY ", success(options).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    "{} session{} · {} shown",
                    model.entries.len(),
                    if model.entries.len() == 1 { "" } else { "s" },
                    model.filtered.len()
                ),
                muted(options),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_list(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    let focused = model.pane == Pane::List;
    let border = if focused {
        accent(options)
    } else {
        dim(options)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(format!(
            " Sessions {}/{} ",
            if model.filtered.is_empty() {
                0
            } else {
                model.selected + 1
            },
            model.filtered.len()
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if model.loading && model.entries.is_empty() {
        frame.render_widget(
            Paragraph::new("Scanning session indexes…").style(muted(options)),
            inner,
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
            inner,
        );
        return;
    }
    let end = model
        .list_offset
        .saturating_add(usize::from(inner.height))
        .min(model.filtered.len());
    let lines = model.filtered[model.list_offset..end]
        .iter()
        .enumerate()
        .map(|(visible, index)| {
            session_line(
                &model.entries[*index],
                model.list_offset + visible == model.selected,
                inner.width,
                options,
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn session_line(
    entry: &SessionEntry,
    is_selected: bool,
    width: u16,
    options: PickerOptions,
) -> Line<'static> {
    let cursor = if is_selected { "›" } else { " " };
    let agent = match entry.agent.name() {
        "codex" => "C",
        _ => "A",
    };
    let favorite = if entry.favorite { "★" } else { " " };
    let state = if entry.disabled_reason.is_some() {
        "×"
    } else if entry.warning.is_some() {
        "!"
    } else if entry.current_project {
        "●"
    } else {
        " "
    };
    let prefix = format!(
        "{cursor} [{agent}{}] {favorite}{state} ",
        entry.origin.short_label()
    );
    let suffix = format!("  {}", clean(&entry.updated));
    let available = usize::from(width)
        .saturating_sub(prefix.chars().count())
        .saturating_sub(suffix.chars().count());
    let title = truncate(&clean(&entry.title), available.max(1));
    let line_style = if is_selected {
        selected(options)
    } else if entry.disabled_reason.is_some() {
        dim(options)
    } else {
        normal(options)
    };
    Line::from(vec![
        Span::styled(prefix, line_style.add_modifier(Modifier::BOLD)),
        Span::styled(title, line_style),
        Span::styled(suffix, line_style),
    ])
}

fn render_preview(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    let focused = model.pane == Pane::Preview;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if focused {
            accent(options)
        } else {
            dim(options)
        })
        .title(format!(" Preview · {} ", model.preview_density.label()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(entry) = model.selected_entry() else {
        frame.render_widget(
            Paragraph::new("Select a session to preview it.")
                .style(muted(options))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    };
    let host = entry.host.as_deref().unwrap_or("this host");
    let mut lines = vec![
        Line::from(Span::styled(
            clean(&entry.title),
            accent(options).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                format!("{} · {} · ", entry.agent.name(), entry.origin.label()),
                muted(options),
            ),
            Span::styled(clean(host), normal(options)),
        ]),
        Line::from(Span::styled(clean(&entry.workspace), muted(options))),
        Line::from(Span::styled(
            format!("id {}", clean(&entry.id)),
            dim(options),
        )),
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
            Span::styled("Warning: ", warning(options).add_modifier(Modifier::BOLD)),
            Span::styled(clean(warning_text), normal(options)),
        ]));
        lines.push(Line::default());
    }
    if model.preview_density == PreviewDensity::Metadata {
        lines.extend([
            Line::from(vec![
                Span::styled("project  ", muted(options)),
                Span::styled(clean(&entry.project), normal(options)),
            ]),
            Line::from(vec![
                Span::styled("updated  ", muted(options)),
                Span::styled(clean(&entry.updated), normal(options)),
            ]),
            Line::from(vec![
                Span::styled("favorite ", muted(options)),
                Span::styled(if entry.favorite { "yes" } else { "no" }, normal(options)),
            ]),
            Line::default(),
            Line::from(Span::styled(
                "Press v for a transcript preview.",
                muted(options),
            )),
        ]);
    } else {
        render_conversation(&mut lines, model, options);
    }
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let maximum = paragraph
        .line_count(inner.width)
        .saturating_sub(usize::from(inner.height));
    let scroll = usize::from(model.preview_scroll)
        .min(maximum)
        .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(paragraph.scroll((scroll, 0)), inner);
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
                    Span::styled("Preview warning: ", warning(options)),
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
    let compact = area.width < 72;
    let hints = if model.mode == Mode::Search {
        "type to filter · Enter done · Ctrl-U clear · Esc done"
    } else if compact {
        "↕ move · Enter review · x reset · ! warnings · ? help"
    } else {
        "↑↓/jk move · Tab pane · Enter review · v preview · f favorite · y copy ID · ! warnings · r refresh · ? help · q quit"
    };
    let pane = match model.pane {
        Pane::List => "LIST",
        Pane::Preview => "PREVIEW",
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {pane} "),
                selected(options).add_modifier(Modifier::BOLD),
            ),
            Span::styled(hints, muted(options)),
        ])),
        area,
    );
}

fn render_help(frame: &mut Frame<'_>, area: Rect, model: &Model, options: PickerOptions) {
    let popup = centered(
        area,
        72.min(area.width.saturating_sub(2)),
        21.min(area.height.saturating_sub(2)),
    );
    frame.render_widget(Clear, popup);
    let text = Text::from(vec![
        Line::from(Span::styled(
            "Move",
            accent(options).add_modifier(Modifier::BOLD),
        )),
        Line::from("  ↑/↓, j/k, Ctrl-N/P   one row"),
        Line::from("  PageUp/Down, Home/End page or edge"),
        Line::from("  Tab                   switch list/preview"),
        Line::default(),
        Line::from(Span::styled(
            "Find and filter",
            accent(options).add_modifier(Modifier::BOLD),
        )),
        Line::from("  /                     fuzzy token search"),
        Line::from("  1/o, 2/a, 3/s         origin, agent, scope"),
        Line::from("  v                     preview density"),
        Line::from("  x                     reset search + filters"),
        Line::default(),
        Line::from(Span::styled(
            "Act",
            accent(options).add_modifier(Modifier::BOLD),
        )),
        Line::from("  Enter                 review, then apply"),
        Line::from("  f                     favorite/unfavorite"),
        Line::from("  y                     copy session ID"),
        Line::from("  r                     refresh local + remote"),
        Line::from("  ! / w                 inspect all warnings"),
        Line::from("  q / Esc / Ctrl-C      cancel safely"),
        Line::default(),
        Line::from(Span::styled(
            "Click rows or scroll with the mouse.  ? closes help.",
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
        "All catalog warnings",
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
        "↑/↓ scroll · !, w, Enter, or Esc closes",
        muted(options),
    )));
    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Diagnostics ")
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
    let Some(entry) = model.selected_entry() else {
        return;
    };
    let block = Block::default()
        .title(" Review transfer ")
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
                Line::from("←/→ choose · Enter apply · Esc back"),
            ]
        } else if inner.height >= 2 {
            vec![
                Line::from(Span::styled(
                    format!("Apply: {action}"),
                    selected(options).add_modifier(Modifier::BOLD),
                )),
                Line::from("←/→ choose · Enter apply · Esc back"),
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
                    "{} · {} · {}",
                    entry.agent.name(),
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
        Paragraph::new("←/→ choose · Enter apply · Esc back").style(muted(options)),
        rows[3],
    );
}

pub(crate) fn review_hit(area: Rect, column: u16, row: u16) -> Option<usize> {
    if !review_can_apply(area) {
        return None;
    }
    let popup = review_area(area);
    if popup.height < 10 {
        return None;
    }
    let inner = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(inner);
    review_buttons(rows[2])
        .into_iter()
        .position(|button| button.contains((column, row).into()))
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

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::cli::Agent;
    use crate::tui::{CatalogSnapshot, Origin};

    fn model() -> Model {
        let mut model = Model::new();
        model.load(
            CatalogSnapshot {
                sessions: vec![SessionEntry {
                    key: "remote:one".into(),
                    id: "01999999-1111-7222-8333-444444444444".into(),
                    agent: Agent::Claude,
                    origin: Origin::Remote,
                    host: Some("archie".into()),
                    project: "dotfiles".into(),
                    workspace: "/Users/me/dotfiles".into(),
                    title: "Make the session picker beautiful".into(),
                    updated: "8m".into(),
                    current_project: true,
                    favorite: true,
                    disabled_reason: None,
                    warning: Some("preview is partial".into()),
                    sort_timestamp: 0,
                }],
                warnings: vec!["remote host took longer than expected".into()],
            },
            true,
        );
        model
    }

    fn rendered(width: u16, height: u16, mut model: Model) -> String {
        model.area = Rect::new(0, 0, width, height);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &model,
                    PickerOptions {
                        color: false,
                        reduced_motion: true,
                        initial_action: PickerAction::HopAndOpen,
                        initial_view: super::super::PickerView::default(),
                    },
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn wide_layout_renders_list_preview_diagnostics_and_explicit_labels() {
        let text = rendered(120, 28, model());
        assert!(text.contains("agent-hop"), "{text:?}");
        assert!(text.contains("Sessions 1/1"), "{text:?}");
        assert!(text.contains("Preview"), "{text:?}");
        assert!(text.contains("WARNING"), "{text:?}");
        assert!(text.contains("[AR]"), "{text:?}");
    }

    #[test]
    fn tiny_layout_shows_only_the_focused_pane() {
        let mut list = model();
        list.pane = Pane::List;
        let list_text = rendered(48, 14, list);
        assert!(list_text.contains("Sessions"));
        assert!(!list_text.contains("Preview"));

        let mut preview = model();
        preview.pane = Pane::Preview;
        let preview_text = rendered(48, 14, preview);
        assert!(preview_text.contains("Preview"));
        assert!(!preview_text.contains("Sessions 1/1"));
    }

    #[test]
    fn review_and_help_are_rendered_as_modal_overlays() {
        let mut review = model();
        review.mode = Mode::Review;
        let review_text = rendered(100, 26, review);
        assert!(review_text.contains("Review transfer"));
        assert!(review_text.contains("Hop & open"));
        assert!(review_text.contains("Copy only"));
        assert!(review_text.contains("Dry run"));

        let mut help = model();
        help.mode = Mode::Help;
        let help_text = rendered(100, 26, help);
        assert!(help_text.contains("fuzzy token search"));
    }

    #[test]
    fn wrapped_preview_can_scroll_all_the_way_to_its_tail() {
        let mut model = model();
        model.pane = Pane::Preview;
        model.preview_scroll = u16::MAX;
        model.previews.insert(
            "remote:one".into(),
            PreviewState::Ready(super::super::Preview {
                lines: vec![super::super::PreviewLine {
                    role: super::super::PreviewRole::Assistant,
                    text: format!("{} TAIL_MARKER", "wrapped words ".repeat(80)),
                }],
                truncated: false,
                warning: None,
            }),
        );
        let text = rendered(48, 14, model);
        assert!(text.contains("TAIL_MARKER"), "{text:?}");
    }

    #[test]
    fn diagnostics_and_short_review_keep_controls_visible() {
        let mut diagnostics = model();
        diagnostics.mode = Mode::Diagnostics;
        let diagnostic_text = rendered(64, 14, diagnostics);
        assert!(diagnostic_text.contains("Diagnostics"));
        assert!(diagnostic_text.contains("remote host"));

        let mut review = model();
        review.mode = Mode::Review;
        let review_text = rendered(64, 9, review);
        assert!(review_text.contains("Hop & open"));
        assert!(review_text.contains("Enter apply"));

        let mut very_short = model();
        very_short.mode = Mode::Review;
        let very_short_text = rendered(30, 6, very_short);
        assert!(
            very_short_text.contains("Hop & open"),
            "{very_short_text:?}"
        );
        assert!(
            very_short_text.contains("Enter apply"),
            "{very_short_text:?}"
        );
    }

    #[test]
    fn wrapped_overlays_scroll_to_their_last_content() {
        let mut diagnostics = model();
        diagnostics.mode = Mode::Diagnostics;
        diagnostics.overlay_scroll = u16::MAX;
        diagnostics.warnings = vec![format!("{} DIAGNOSTIC_TAIL", "wrapped warning ".repeat(40))];
        let diagnostic_text = rendered(36, 9, diagnostics);
        assert!(
            diagnostic_text.contains("DIAGNOSTIC_TAIL"),
            "{diagnostic_text:?}"
        );

        let mut help = model();
        help.mode = Mode::Help;
        help.overlay_scroll = u16::MAX;
        let help_text = rendered(36, 9, help);
        assert!(help_text.contains("closes help"), "{help_text:?}");
    }
}
