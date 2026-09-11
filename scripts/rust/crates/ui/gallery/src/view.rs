use std::io::{self, IsTerminal};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ui_diff_view::{DiffDocument, DiffView, ViewState};
use ui_picker::{Item, Mode, SelectionState};
use ui_progress::{PhaseState, Progress, ProgressBar};
use ui_theme::{ColorMode, Palette, Role, ThemeHandle};

struct Gallery {
    selection: SelectionState<usize>,
    diff: DiffDocument,
    diff_state: ViewState,
    diff_active: bool,
    diff_height: u16,
    diff_width: u16,
    searching: bool,
    query: String,
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            selection: SelectionState::new(
                [
                    "Local files",
                    "Remote snapshot",
                    "Recovery point",
                    "Unavailable source",
                ]
                .into_iter()
                .enumerate()
                .map(|(id, label)| Item::new(id, label).disabled(id == 3))
                .collect(),
                Mode::Multiple,
            ),
            diff: DiffDocument::new(
                "theme = mocha\npreview = false\nworkers = 2\n",
                "theme = sexy-purple\npreview = true\nworkers = 2\n",
            ),
            diff_state: ViewState::default(),
            diff_active: false,
            diff_height: 0,
            diff_width: 0,
            searching: false,
            query: String::new(),
        }
    }
}

fn rich(line: ui_widgets::Line, palette: &Palette, color: bool) -> Line<'static> {
    let mode = if color {
        ColorMode::Always
    } else {
        ColorMode::Never
    };
    Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.text, palette.ratatui(mode, true, span.role)))
            .collect::<Vec<_>>(),
    )
}

fn panel(
    area: Rect,
    title: &str,
    palette: &Palette,
    color: bool,
    focused: bool,
    buffer: &mut Buffer,
) -> Rect {
    let mode = if color {
        ColorMode::Always
    } else {
        ColorMode::Never
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(palette.ratatui(
            mode,
            true,
            if focused { Role::Focus } else { Role::Border },
        ));
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}

fn render(
    gallery: &mut Gallery,
    area: Rect,
    buffer: &mut Buffer,
    palette: &Palette,
    color: bool,
    frame: u64,
) {
    if area.is_empty() {
        return;
    }
    let mode = if color {
        ColorMode::Always
    } else {
        ColorMode::Never
    };
    buffer.set_style(area, palette.ratatui(mode, true, Role::Background));
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);
    Paragraph::new(format!("UI components · {}", palette.profile))
        .style(palette.ratatui(mode, true, Role::Strong))
        .render(rows[0], buffer);
    let panels = if area.width >= 80 {
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(rows[1])
    } else {
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).split(rows[1])
    };
    let controls = panel(
        panels[0],
        "Picker / progress",
        palette,
        color,
        !gallery.diff_active,
        buffer,
    );
    let diff = panel(
        panels[1],
        "Comparison",
        palette,
        color,
        gallery.diff_active,
        buffer,
    );
    gallery.diff_height = diff.height;
    gallery.diff_width = diff.width;
    gallery
        .diff_state
        .fit_width(&gallery.diff, diff.width, diff.height);
    let content = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .split(controls);
    Paragraph::new(format!(
        "Search: {}  · {} selected",
        gallery.query,
        gallery.selection.selected_count()
    ))
    .style(palette.ratatui(mode, true, Role::Muted))
    .render(content[0], buffer);
    let visible = gallery.selection.rows().len();
    gallery
        .selection
        .viewport
        .settle(visible, usize::from(content[1].height));
    let items = gallery
        .selection
        .viewport
        .visible(visible, usize::from(content[1].height))
        .map(|position| {
            let index = gallery.selection.rows()[position];
            let item = &gallery.selection.items()[index];
            let mut line = ui_widgets::choice(
                &item.label,
                position == gallery.selection.viewport.cursor,
                gallery.selection.checked(index),
            );
            if item.disabled {
                line.spans
                    .push(ui_widgets::Span::new(" (unavailable)", Role::Warning));
            }
            rich(line, palette, color)
        })
        .collect::<Vec<_>>();
    Paragraph::new(if items.is_empty() {
        vec![rich(
            ui_widgets::Line::styled("No matches", Role::Muted),
            palette,
            color,
        )]
    } else {
        items
    })
    .render(content[1], buffer);
    Paragraph::new(ui_progress::phase_track(
        &[
            ("Load", PhaseState::Complete),
            ("Review", PhaseState::Active),
            ("Apply", PhaseState::Pending),
        ],
        palette,
        color,
    ))
    .render(content[2], buffer);
    ProgressBar {
        progress: Progress {
            completed: 7,
            total: Some(12),
        },
        frame,
        palette,
        color,
    }
    .render(content[3], buffer);
    let statuses = ui_widgets::Line::from_spans([
        ui_widgets::Span::new("✓ Ready  ", Role::Success),
        ui_widgets::Span::new("! Warning  ", Role::Warning),
        ui_widgets::Span::new("× Error", Role::Danger),
    ]);
    Paragraph::new(vec![
        rich(statuses, palette, color),
        rich(
            ui_widgets::detail("Preview", "Local and remote sources"),
            palette,
            color,
        ),
    ])
    .render(content[4], buffer);
    DiffView {
        document: &gallery.diff,
        state: &gallery.diff_state,
        palette,
        color,
        left_label: "repo",
        right_label: "live",
    }
    .render(diff, buffer);
    use ui_widgets::KeyHint;
    let hints = if gallery.searching {
        vec![
            KeyHint::new("Type", "search"),
            KeyHint::new("Esc", "leave search"),
        ]
    } else if gallery.diff_active {
        vec![
            KeyHint::new("Tab", "picker"),
            KeyHint::new("j/k", "scroll"),
            KeyHint::new("[/]", "hunk"),
            KeyHint::new("v", "view"),
            KeyHint::new("q", "quit"),
        ]
    } else {
        vec![
            KeyHint::new("Tab", "diff"),
            KeyHint::new("Space", "check"),
            KeyHint::new("a", "all matches"),
            KeyHint::new("/", "search"),
            KeyHint::new("q", "quit"),
        ]
    };
    Paragraph::new(rich(ui_widgets::hints(&hints), palette, color)).render(rows[2], buffer);
}

fn input(gallery: &mut Gallery, key: KeyEvent, height: u16) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return true;
    }
    if gallery.searching {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => gallery.searching = false,
            KeyCode::Backspace => {
                gallery.query.pop();
            }
            KeyCode::Char(character) if !character.is_control() => gallery.query.push(character),
            _ => {}
        }
        gallery.selection.set_query(&gallery.query);
        return false;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return true,
        KeyCode::Tab | KeyCode::BackTab => gallery.diff_active = !gallery.diff_active,
        _ if gallery.diff_active => {
            gallery
                .diff_state
                .fit_width(&gallery.diff, gallery.diff_width, gallery.diff_height);
            gallery.diff_state.handle_key(
                key,
                &gallery.diff,
                if gallery.diff_height > 0 {
                    gallery.diff_height
                } else {
                    height
                },
            );
        }
        KeyCode::Up | KeyCode::Char('k') => gallery.selection.move_by(-1),
        KeyCode::Down | KeyCode::Char('j') => gallery.selection.move_by(1),
        KeyCode::Char(' ') => gallery.selection.toggle(),
        KeyCode::Char('a') => gallery.selection.toggle_visible(),
        KeyCode::Char('/') => gallery.searching = true,
        _ => {}
    }
    false
}

pub fn run(palette: Option<Palette>) -> io::Result<()> {
    let fixed = palette.map(Arc::new);
    let mut theme = ThemeHandle::discover();
    if !ui_terminal::capable(
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
        std::env::var("CI").ok().as_deref(),
    ) {
        for line in preview(fixed.as_deref().unwrap_or(theme.palette()), 100, false) {
            println!("{line}");
        }
        return Ok(());
    }
    let color = ColorMode::Auto.enabled(true);
    let _signals = ui_terminal::SignalGuard::new()?;
    let mut terminal = ui_terminal::Alternate::new(ui_terminal::MouseCapture::Disabled)?;
    let mut gallery = Gallery::default();
    let start = Instant::now();
    let mut dirty = true;
    while !ui_terminal::termination_requested() {
        if fixed.is_none() {
            dirty |= theme.poll();
        }
        if dirty {
            let palette = fixed.as_deref().unwrap_or(theme.palette());
            terminal.terminal().draw(|frame| {
                render(
                    &mut gallery,
                    frame.area(),
                    frame.buffer_mut(),
                    palette,
                    color,
                    (start.elapsed().as_millis() / 120) as u64,
                );
            })?;
            dirty = false;
        }
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    if input(&mut gallery, key, terminal.terminal().size()?.height) {
                        break;
                    }
                    dirty = true;
                }
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
        }
    }
    Ok(())
}

fn ansi(color: Color, background: bool) -> String {
    use ui_theme::Color as Shared;
    let color = match color {
        Color::Reset => Shared::Terminal,
        Color::Rgb(r, g, b) => Shared::Rgb(r, g, b),
        Color::Indexed(index) => Shared::Ansi(index),
        Color::Black => Shared::Ansi(0),
        Color::Red => Shared::Ansi(1),
        Color::Green => Shared::Ansi(2),
        Color::Yellow => Shared::Ansi(3),
        Color::Blue => Shared::Ansi(4),
        Color::Magenta => Shared::Ansi(5),
        Color::Cyan => Shared::Ansi(6),
        Color::Gray => Shared::Ansi(7),
        Color::DarkGray => Shared::Ansi(8),
        Color::LightRed => Shared::Ansi(9),
        Color::LightGreen => Shared::Ansi(10),
        Color::LightYellow => Shared::Ansi(11),
        Color::LightBlue => Shared::Ansi(12),
        Color::LightMagenta => Shared::Ansi(13),
        Color::LightCyan => Shared::Ansi(14),
        Color::White => Shared::Ansi(15),
    };
    format!("\x1b[{}m", color.sgr(background))
}

pub fn preview(palette: &Palette, width: usize, colored: bool) -> Vec<String> {
    let area = Rect::new(
        0,
        0,
        width.clamp(1, 240) as u16,
        if width >= 80 { 17 } else { 27 },
    );
    let mut buffer = Buffer::empty(area);
    render(
        &mut Gallery::default(),
        area,
        &mut buffer,
        palette,
        colored,
        0,
    );
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            let mut style = None;
            while x < area.width {
                let cell = &buffer[(x, y)];
                let next_style = (cell.fg, cell.bg, cell.modifier);
                if colored && style != Some(next_style) {
                    line.push_str("\x1b[0m");
                    line.push_str(&ansi(cell.fg, false));
                    line.push_str(&ansi(cell.bg, true));
                    if cell.modifier.contains(Modifier::BOLD) {
                        line.push_str("\x1b[1m");
                    }
                    if cell.modifier.contains(Modifier::REVERSED) {
                        line.push_str("\x1b[7m");
                    }
                    style = Some(next_style);
                }
                line.push_str(cell.symbol());
                x = x.saturating_add(ui_terminal::text::width(cell.symbol()).max(1) as u16);
            }
            if colored {
                line.push_str("\x1b[0m");
            }
            line.trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/unit/gallery.rs"]
mod tests;
