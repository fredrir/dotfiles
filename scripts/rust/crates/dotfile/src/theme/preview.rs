use super::{
    Result,
    cli::Command,
    emitters::Target,
    model::{Repository, Theme, table},
    selection::{self, Selection},
};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal},
    time::Duration,
};
use ui_theme::{ColorMode, Role, ThemeHandle};
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}
fn swatch(t: &Theme, name: &str) -> Result<String> {
    let c = t.color(name)?;
    Ok(
        if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
            format!("\x1b[{}m██\x1b[0m {name:16} {c}", c.ansi())
        } else {
            format!("{name:16} {c}")
        },
    )
}
pub fn show(t: &Theme, selection: &Selection, targets: &[Target]) -> Result<()> {
    println!(
        "\n  {}   {}   {}\n",
        t.profile,
        t.name,
        if t.dark { "dark" } else { "light" }
    );
    println!("  source  theme/profiles/{}.toml", t.profile);
    println!(
        "  fonts   {} / {}\n  sizes   {} / {}\n",
        t.font("general")?,
        t.font("nerd")?,
        t.size("terminal")?,
        t.size("interface")?
    );
    print_card(t)?;
    println!("  COMPONENTS\n");
    let palette = super::emitters::ui::palette(t)?;
    let colored = ColorMode::Auto.enabled(io::stdout().is_terminal());
    for line in ui_gallery::preview(
        &palette,
        workstation::terminal_width().unwrap_or(80),
        colored,
    ) {
        println!("{line}");
    }
    println!("  PALETTE\n");
    for name in Theme::palette_names() {
        println!("  {}", swatch(t, &name)?);
    }
    println!("\n  ROLES\n");
    for (name, _) in table(&t.data.roles["roles"])? {
        println!("  {name:20} {}", t.role(name)?);
    }
    println!("\n  STAMPED INTO\n");
    let owned = targets
        .iter()
        .filter(|target| selection.for_path(&target.path) == t.profile)
        .collect::<Vec<_>>();
    let scopes = owned
        .iter()
        .map(|target| selection.scope_of(&target.path))
        .collect::<std::collections::BTreeSet<_>>();
    if owned.is_empty() {
        println!("  no group is assigned to this profile");
    } else {
        println!(
            "  {}      {} files",
            scopes.into_iter().collect::<Vec<_>>().join("  |  "),
            owned.len()
        );
    }
    println!();
    Ok(())
}
pub fn status(
    repo: &Repository,
    selection: &Selection,
    targets: &[Target],
    changes: &[super::plan::Change],
) -> Result<()> {
    let mut names = repo.names();
    names.sort_by_key(|name| (name != selection.default(), name.clone()));
    println!();
    for name in names {
        let t = repo.theme(&name)?;
        let owned = targets
            .iter()
            .filter(|target| selection.for_path(&target.path) == name)
            .collect::<Vec<_>>();
        let scopes = owned
            .iter()
            .map(|target| selection.scope_of(&target.path))
            .collect::<std::collections::BTreeSet<_>>();
        println!(
            "  {}  {}   {}   {}   {}",
            if owned.is_empty() { "○" } else { "●" },
            t.profile,
            t.name,
            if t.dark { "dark" } else { "light" },
            if owned.is_empty() {
                "unassigned".into()
            } else {
                format!("{} files", owned.len())
            }
        );
        if !scopes.is_empty() {
            println!(
                "     {}",
                scopes.into_iter().collect::<Vec<_>>().join("  |  ")
            );
        }
        println!();
    }
    if changes.is_empty() {
        println!("  every generated file matches its profile");
    } else {
        println!(
            "  {} generated files would change      dotfile theme sync",
            changes.len()
        );
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct Pick {
    pub kind: &'static str,
    pub option: String,
    pub index: usize,
}
#[derive(Clone, Debug)]
pub struct Column {
    pub kind: &'static str,
    pub options: Vec<String>,
    pub details: Vec<String>,
    pub index: usize,
}
impl Column {
    fn new(kind: &'static str, options: Vec<String>, details: Vec<String>) -> Self {
        Self {
            kind,
            options,
            details,
            index: 0,
        }
    }

    fn move_by(&mut self, amount: isize) {
        let mut viewport = ui_widgets::Viewport {
            cursor: self.index,
            offset: 0,
        };
        viewport.move_by(amount, self.options.len(), ui_widgets::Navigation::Clamp);
        self.index = viewport.cursor;
    }
}
fn profile_column(repo: &Repository, default: &str) -> Column {
    let names = repo.names();
    let details = names
        .iter()
        .map(|n| {
            let t = &repo.themes[n];
            format!("{}   {}", t.name, if t.dark { "dark" } else { "light" })
        })
        .collect();
    let index = names.iter().position(|n| n == default).unwrap_or(0);
    Column {
        kind: "profile",
        options: names,
        details,
        index,
    }
}
pub fn next(
    repo: &Repository,
    selection: &Selection,
    groups: &BTreeMap<String, Vec<String>>,
    flow: &str,
    picks: &[Pick],
) -> Option<Column> {
    if picks.is_empty() && flow.is_empty() {
        return Some(Column::new(
            "menu",
            [
                "sync", "switch", "status", "preview", "gallery", "dry", "check",
            ]
            .map(str::to_string)
            .to_vec(),
            [
                "regenerate every config",
                "assign a profile to a scope",
                "resolved profiles, and drift",
                "look at a profile in full",
                "browse shared components",
                "what sync would change",
                "validate every profile and application pair",
            ]
            .map(str::to_string)
            .to_vec(),
        ));
    }
    let command = if flow.is_empty() {
        picks.first().map(|p| p.option.as_str()).unwrap_or("")
    } else {
        flow
    };
    let last = picks.last();
    if (matches!(command, "preview" | "gallery")
        && (last.is_none() || last.is_some_and(|p| p.kind == "menu")))
        || (command == "switch" && last.is_some_and(|p| p.kind == "scope" && p.index == 0))
    {
        return Some(profile_column(repo, selection.default()));
    }
    if command != "switch" {
        return None;
    }
    if last.is_none() || last.is_some_and(|p| p.kind == "menu") {
        let mut options = vec!["global".into()];
        options.extend(groups.keys().cloned());
        let mut details = vec![selection.default().into()];
        details.extend(groups.iter().map(|(group, packages)| {
            format!(
                "{}   {}",
                selection.current(group, "theme"),
                packages.join(", ")
            )
        }));
        return Some(Column::new("scope", options, details));
    }
    if let Some(pick) = last.filter(|p| p.kind == "scope") {
        let packages = groups.get(&pick.option)?;
        if packages.len() < 2 {
            return Some(profile_column(
                repo,
                selection.current(&pick.option, "theme"),
            ));
        }
        let mut options = vec!["group".into()];
        options.extend(packages.iter().cloned());
        let mut details = vec![format!(
            "every file in {}, now {}",
            pick.option,
            selection.current(&pick.option, "theme")
        )];
        details.extend(
            packages
                .iter()
                .map(|p| selection.current(&pick.option, p).into()),
        );
        return Some(Column::new("package", options, details));
    }
    if let Some(pick) = last.filter(|p| p.kind == "package") {
        let group = &picks.iter().find(|p| p.kind == "scope")?.option;
        return Some(profile_column(
            repo,
            selection.current(
                group,
                if pick.index == 0 {
                    "theme"
                } else {
                    &pick.option
                },
            ),
        ));
    }
    None
}
fn command(flow: &str, picks: &[Pick]) -> Result<Command> {
    let name = if flow.is_empty() {
        picks.first().map(|p| p.option.as_str()).unwrap_or("")
    } else {
        flow
    };
    Ok(match name {
        "sync" => Command::Sync,
        "dry" => Command::Dry,
        "check" => Command::Check,
        "status" => Command::Status,
        "preview" => Command::Preview {
            profile: picks.last().map(|p| p.option.clone()),
        },
        "gallery" => Command::Gallery {
            profile: picks.last().map(|p| p.option.clone()),
        },
        "switch" => {
            let scope = picks
                .iter()
                .find(|p| p.kind == "scope")
                .map(|p| p.option.clone())
                .unwrap_or_else(|| "shared".into());
            let package = picks.iter().find(|p| p.kind == "package" && p.index != 0);
            let scope = package.map_or(scope.clone(), |p| format!("{scope}/{}", p.option));
            Command::Switch {
                profile: picks.last().map(|p| p.option.clone()),
                scope: Some(scope),
            }
        }
        _ => return Err("unknown theme action".into()),
    })
}
pub fn choose(
    repo: &Repository,
    selection: &Selection,
    targets: &[Target],
    flow: &str,
) -> Result<Option<Command>> {
    let groups = selection::inventory(targets);
    let mut picks = Vec::new();
    let mut columns = vec![next(repo, selection, &groups, flow, &picks).ok_or("no theme choices")?];
    if columns[0].options.is_empty() {
        return Err("no profiles in theme/profiles".into());
    }
    let _signals = ui_terminal::SignalGuard::with_options(ui_terminal::SignalOptions {
        cancellation: Some(crate::cancel::flag()),
        reraise_on_drop: false,
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    let mut surface = ui_terminal::Alternate::new(ui_terminal::MouseCapture::Disabled)
        .map_err(|e| e.to_string())?;
    let mut theme = ThemeHandle::from_path(repo.root.join("config/theme/theme.json"));
    let mode = if ColorMode::Auto.enabled(true) {
        ColorMode::Always
    } else {
        ColorMode::Never
    };
    loop {
        if ui_terminal::termination_requested() {
            return Err("interrupted".into());
        }
        theme.poll();
        let palette = theme.palette();
        surface
            .terminal()
            .draw(|frame| {
                let area = frame.area();
                frame
                    .buffer_mut()
                    .set_style(area, palette.ratatui(mode, true, Role::Background));
                let vertical = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Min(7),
                        Constraint::Length(7),
                        Constraint::Length(1),
                    ])
                    .split(frame.area());
                frame.render_widget(
                    Paragraph::new("dotfile theme").style(palette.ratatui(
                        mode,
                        true,
                        Role::Strong,
                    )),
                    vertical[0],
                );
                let constraints = vec![Constraint::Ratio(1, columns.len() as u32); columns.len()];
                let areas = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints(constraints)
                    .split(vertical[1]);
                for (i, column) in columns.iter().enumerate() {
                    let items = column
                        .options
                        .iter()
                        .enumerate()
                        .map(|(j, name)| {
                            ListItem::new(vec![
                                Line::from(name.clone()),
                                Line::from(Span::styled(
                                    column.details.get(j).cloned().unwrap_or_default(),
                                    palette.ratatui(mode, true, Role::Muted),
                                )),
                            ])
                        })
                        .collect::<Vec<_>>();
                    let list = List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(column.kind)
                                .border_style(palette.ratatui(mode, true, Role::Border)),
                        )
                        .highlight_style(palette.ratatui(mode, true, Role::Selection))
                        .highlight_symbol("› ");
                    let mut state = ListState::default();
                    state.select(Some(column.index));
                    frame.render_stateful_widget(list, areas[i], &mut state);
                }
                if let Some(column) = columns.last().filter(|c| c.kind == "profile")
                    && let Some(t) = column
                        .options
                        .get(column.index)
                        .and_then(|name| repo.themes.get(name))
                {
                    let lines = if mode != ColorMode::Never {
                        terminal_card(t).unwrap_or_default()
                    } else {
                        vec![
                            Line::from(format!(
                                "{}  ·  {}",
                                t.name,
                                if t.dark { "dark" } else { "light" }
                            )),
                            Line::from(format!(
                                "{}  /  {}",
                                t.font("general").unwrap_or(""),
                                t.font("nerd").unwrap_or("")
                            )),
                        ]
                    };
                    frame.render_widget(
                        Paragraph::new(lines).wrap(Wrap { trim: false }),
                        vertical[2],
                    );
                }
                frame.render_widget(
                    Paragraph::new(Line::from(
                        ui_widgets::hints(&[
                            ui_widgets::KeyHint::new("↑/↓", "select"),
                            ui_widgets::KeyHint::new("→/enter", "open"),
                            ui_widgets::KeyHint::new("←", "back"),
                            ui_widgets::KeyHint::new("esc", "cancel"),
                        ])
                        .spans
                        .into_iter()
                        .map(|span| Span::styled(span.text, palette.ratatui(mode, true, span.role)))
                        .collect::<Vec<_>>(),
                    )),
                    vertical[3],
                );
            })
            .map_err(|e| e.to_string())?;
        if !event::poll(Duration::from_millis(50)).map_err(|e| e.to_string())? {
            continue;
        }
        let Event::Key(key) = event::read().map_err(|e| e.to_string())? else {
            continue;
        };
        if key.kind == event::KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            crate::cancel::request();
            return Err("interrupted".into());
        }
        let current = columns.last_mut().ok_or("no active column")?;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => current.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => current.move_by(1),
            KeyCode::Home => current.index = 0,
            KeyCode::End => current.index = current.options.len().saturating_sub(1),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                if columns.len() > 1 {
                    columns.pop();
                    picks.pop();
                }
            }
            KeyCode::Right | KeyCode::Enter | KeyCode::Char('l') => {
                let Some(option) = current.options.get(current.index).cloned() else {
                    continue;
                };
                picks.push(Pick {
                    kind: current.kind,
                    option,
                    index: current.index,
                });
                if let Some(column) = next(repo, selection, &groups, flow, &picks) {
                    columns.push(column);
                } else {
                    return command(flow, &picks).map(Some);
                }
            }
            _ => {}
        }
    }
}
pub fn confirm(prompt: &str) -> Result<bool> {
    let answer = ui_cli::prompt(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
        &format!("  {prompt} [Y/n] "),
        false,
    )
    .map_err(|error| error.to_string())?;
    Ok(answer == Some(ui_cli::Answer::Yes))
}

fn terminal_card(t: &Theme) -> Result<Vec<Line<'static>>> {
    let bg = t.app("terminal", "background")?;
    let rgb = |c: super::color::Color| {
        let [r, g, b] = c.0;
        Color::Rgb(r, g, b)
    };
    let span = |text: String, fg: super::color::Color| {
        Span::styled(text, Style::default().fg(rgb(fg)).bg(rgb(bg)))
    };
    let padded = |spans: Vec<Span<'static>>| {
        let mut line = Line::from(spans).style(Style::default().bg(rgb(bg)));
        line.spans.push(Span::styled(
            " ".repeat(56usize.saturating_sub(line.width())),
            Style::default().bg(rgb(bg)),
        ));
        line
    };
    let mut lines = vec![padded(vec![span(
        format!("  {}    {}", t.name, t.profile),
        t.app("terminal", "foreground")?,
    )])];
    let mut chips = vec![span("  ".into(), bg)];
    for name in super::model::ANSI {
        chips.push(span("███ ".into(), t.color(name)?));
    }
    lines.push(padded(chips));
    let mut prompt = vec![span("  ".into(), bg)];
    for (content, role) in [
        ("~/dotfiles   ", "prompt_dir"),
        ("main   ", "prompt_git"),
        ("3.13   ", "prompt_python"),
        ("1.2s", "prompt_duration"),
    ] {
        prompt.push(span(content.into(), t.role(role)?));
    }
    lines.push(padded(prompt));
    lines.push(padded(vec![
        span("  ❯ ".into(), t.role("prompt_char")?),
        span("eza".into(), t.app("terminal", "foreground")?),
        span("█".into(), t.app("terminal", "cursor")?),
    ]));
    let mut files = vec![span("  ".into(), bg)];
    for (name, key) in [
        ("scripts", "di"),
        ("theme", "di"),
        ("starship.toml", "*.toml"),
        ("setup.sh", "*.sh"),
    ] {
        let expression = t.data.roles["eza"][key]
            .as_str()
            .or_else(|| t.data.roles["eza"]["fi"].as_str())
            .ok_or("missing eza file color")?;
        files.push(span(format!("{name}  "), t.color(expression)?));
    }
    lines.push(padded(files));
    let mut pills = vec![span("  ".into(), bg)];
    for (label, fg, fill) in [
        (
            "selection",
            t.app("terminal", "selection_foreground")?,
            t.app("terminal", "selection_background")?,
        ),
        ("accent", bg, t.app("kde", "accent")?),
    ] {
        pills.push(Span::styled(
            format!(" {label} "),
            Style::default().fg(rgb(fg)).bg(rgb(fill)),
        ));
        pills.push(span("  ".into(), bg));
    }
    let tabs = &t.data.roles["terminal"]["tabs"];
    if let Some(fill) = tabs["active_background"].as_str() {
        let fg = t.color(super::model::text(&tabs["active_foreground"]))?;
        pills.push(Span::styled(
            " tab ",
            Style::default().fg(rgb(fg)).bg(rgb(t.color(fill)?)),
        ));
    }
    lines.push(padded(pills));
    Ok(lines)
}
fn print_card(t: &Theme) -> Result<()> {
    if !io::stdout().is_terminal() || std::env::var_os("NO_COLOR").is_some() {
        return Ok(());
    }
    for line in terminal_card(t)? {
        print!("  ");
        for span in line.spans {
            if let Some(Color::Rgb(r, g, b)) = span.style.fg {
                print!("\x1b[38;2;{r};{g};{b}m");
            }
            if let Some(Color::Rgb(r, g, b)) = span.style.bg {
                print!("\x1b[48;2;{r};{g};{b}m");
            }
            print!("{}", span.content);
        }
        println!("\x1b[0m");
    }
    println!();
    Ok(())
}
