use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tachyonfx::{CellFilter, Effect, EffectRenderer, Interpolation, fx};

use super::model::Snapshot;
use super::{ColorMode, Options, collect, render};

const FRAME: Duration = Duration::from_millis(33);
const IDLE_POLL: Duration = Duration::from_millis(100);

pub fn capable() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var("TERM")
            .ok()
            .is_none_or(|term| !term.eq_ignore_ascii_case("dumb"))
        && std::env::var("CI").ok().is_none_or(|value| !flag(&value))
}

pub fn run(options: Options) -> Result<(), String> {
    let color = options.color.enabled(true);
    let motion = motion_enabled() && color;
    let height = workstation::terminal_height()
        .unwrap_or(24)
        .saturating_sub(1)
        .max(6) as u16;
    let mut terminal = InlineTerminal::new(height)
        .map_err(|error| format!("unable to open verbose terminal: {error}"))?;
    let mut snapshot = None;
    let mut previous_fingerprint = None;
    let mut previous_preferred: Option<Option<hostkit::Route>> = None;
    let mut worker = None;
    let mut next_probe = Instant::now();
    let mut frame_index = 0u64;
    let mut last_draw = Instant::now();
    let mut effect: Option<Effect> = None;
    let mut scroll = 0u16;
    let mut dirty = true;
    let mut failure = None;

    loop {
        if worker.is_none() && (snapshot.is_none() || options.watch && Instant::now() >= next_probe)
        {
            let (sender, receiver) = mpsc::channel();
            let request = options.clone();
            thread::spawn(move || {
                let _ = sender.send(collect::snapshot(&request));
            });
            worker = Some(receiver);
            dirty = true;
        }

        if let Some(receiver) = &worker {
            match receiver.try_recv() {
                Ok(Ok(next)) => {
                    let fingerprint = next.fingerprint();
                    let changed = previous_fingerprint
                        .as_ref()
                        .is_none_or(|previous| previous != &fingerprint);
                    let primary_route = next.primary_route();
                    let route_changed =
                        previous_preferred.is_some_and(|previous| previous != primary_route);
                    if changed && route_changed && options.notify {
                        terminal.bell()?;
                    }
                    failure = next.failure();
                    previous_preferred = Some(primary_route);
                    previous_fingerprint = Some(fingerprint);
                    snapshot = Some(next);
                    scroll = scroll.min(scroll_limit(snapshot.as_ref(), height));
                    worker = None;
                    if motion && changed {
                        effect = Some(reveal_effect(color));
                    }
                    dirty = true;
                    if options.watch {
                        next_probe = Instant::now() + options.interval;
                    }
                }
                Ok(Err(error)) => return Err(error),
                Err(TryRecvError::Disconnected) => {
                    return Err("information worker stopped without a result".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        let max_scroll = scroll_limit(snapshot.as_ref(), height);
        match terminal.input()? {
            Input::None => {}
            Input::Quit => {
                return match failure {
                    Some(error) => Err(error),
                    None => Ok(()),
                };
            }
            Input::Up => {
                scroll = scroll.saturating_sub(1);
                dirty = true;
            }
            Input::Down => {
                scroll = scroll.saturating_add(1).min(max_scroll);
                dirty = true;
            }
            Input::PageUp => {
                scroll = scroll.saturating_sub(height.saturating_sub(3));
                dirty = true;
            }
            Input::PageDown => {
                scroll = scroll
                    .saturating_add(height.saturating_sub(3))
                    .min(max_scroll);
                dirty = true;
            }
            Input::Home => {
                scroll = 0;
                dirty = true;
            }
        }
        let now = Instant::now();
        let animating =
            motion && (worker.is_some() || effect.as_ref().is_some_and(|effect| !effect.done()));
        if dirty || animating && now.duration_since(last_draw) >= FRAME {
            if animating {
                frame_index = frame_index.wrapping_add(1);
            }
            terminal.draw(
                DrawState {
                    snapshot: snapshot.as_ref(),
                    frame_index,
                    color,
                    probing: worker.is_some(),
                    scroll,
                    tick: now.duration_since(last_draw),
                },
                effect.as_mut(),
            )?;
            last_draw = now;
            dirty = false;
            if effect.as_ref().is_some_and(Effect::done) {
                effect = None;
            }
        }
        let sleep = if animating {
            Duration::from_millis(10)
        } else if worker.is_some() {
            Duration::from_millis(20)
        } else if options.watch {
            next_probe
                .saturating_duration_since(Instant::now())
                .min(IDLE_POLL)
        } else {
            IDLE_POLL
        };
        if !sleep.is_zero() {
            thread::sleep(sleep);
        }
    }
}

fn reveal_effect(color: bool) -> Effect {
    let foreground = if color {
        Color::Rgb(51, 65, 85)
    } else {
        Color::DarkGray
    };
    fx::fade_from_fg(foreground, (180, Interpolation::CubicOut)).with_filter(CellFilter::Text)
}

fn motion_enabled() -> bool {
    ![
        "HWIRE_REDUCED_MOTION",
        "PREFERS_REDUCED_MOTION",
        "REDUCE_MOTION",
        "REDUCED_MOTION",
    ]
    .into_iter()
    .any(|name| std::env::var(name).ok().is_some_and(|value| flag(&value)))
}

fn flag(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

fn scroll_limit(snapshot: Option<&Snapshot>, viewport_height: u16) -> u16 {
    let Some(snapshot) = snapshot else {
        return 0;
    };
    let width = workstation::terminal_width()
        .unwrap_or(80)
        .saturating_sub(2)
        .max(1);
    let content_height = render::verbose(snapshot, ColorMode::Never, false)
        .lines()
        .map(|line| line.chars().count().max(1).div_ceil(width))
        .sum::<usize>();
    content_height
        .saturating_sub(usize::from(viewport_height.saturating_sub(2)))
        .min(usize::from(u16::MAX)) as u16
}

struct InlineTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    raw: bool,
}

struct DrawState<'a> {
    snapshot: Option<&'a Snapshot>,
    frame_index: u64,
    color: bool,
    probing: bool,
    scroll: u16,
    tick: Duration,
}

impl InlineTerminal {
    fn new(height: u16) -> io::Result<Self> {
        enable_raw_mode()?;
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = match Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        ) {
            Ok(mut terminal) => {
                if let Err(error) = terminal.hide_cursor() {
                    let _ = disable_raw_mode();
                    return Err(error);
                }
                terminal
            }
            Err(error) => {
                let _ = disable_raw_mode();
                return Err(error);
            }
        };
        Ok(Self {
            terminal,
            raw: true,
        })
    }

    fn draw(&mut self, state: DrawState<'_>, effect: Option<&mut Effect>) -> Result<(), String> {
        let DrawState {
            snapshot,
            frame_index,
            color,
            probing,
            scroll,
            tick,
        } = state;
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                let title = if probing {
                    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    format!(
                        " {} probing ",
                        spinner[frame_index as usize % spinner.len()]
                    )
                } else {
                    " hwire info ".to_string()
                };
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(format!("{title} | ↑/↓ scroll | q quit "))
                    .border_style(ui_style(color, Color::Rgb(124, 58, 237), Modifier::BOLD));
                let inner = block.inner(area);
                frame.render_widget(block, area);
                let text = match snapshot {
                    Some(snapshot) => styled_text(snapshot, color),
                    None => Text::from(Line::styled(
                        "Discovering routes…",
                        ui_style(color, Color::Rgb(148, 163, 184), Modifier::empty()),
                    )),
                };
                frame.render_widget(
                    Paragraph::new(text)
                        .wrap(Wrap { trim: false })
                        .scroll((scroll, 0)),
                    inner,
                );
                if let Some(effect) = effect {
                    frame.render_effect(effect, area, tachyonfx::Duration::from(tick));
                }
            })
            .map(|_| ())
            .map_err(|error| format!("unable to render verbose information: {error}"))
    }

    fn input(&self) -> Result<Input, String> {
        while event::poll(Duration::ZERO)
            .map_err(|error| format!("unable to poll terminal input: {error}"))?
        {
            let Event::Key(key) =
                event::read().map_err(|error| format!("unable to read terminal input: {error}"))?
            else {
                continue;
            };
            if key.kind == KeyEventKind::Press
                && (key.code == KeyCode::Char('q')
                    || key.code == KeyCode::Esc
                    || key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                return Ok(Input::Quit);
            }
            if key.kind == KeyEventKind::Press {
                return Ok(match key.code {
                    KeyCode::Up | KeyCode::Char('k') => Input::Up,
                    KeyCode::Down | KeyCode::Char('j') => Input::Down,
                    KeyCode::PageUp => Input::PageUp,
                    KeyCode::PageDown => Input::PageDown,
                    KeyCode::Home | KeyCode::Char('g') => Input::Home,
                    _ => Input::None,
                });
            }
        }
        Ok(Input::None)
    }

    fn bell(&mut self) -> Result<(), String> {
        self.terminal
            .backend_mut()
            .write_all(b"\x07")
            .and_then(|_| self.terminal.backend_mut().flush())
            .map_err(|error| format!("unable to ring route-change bell: {error}"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Input {
    None,
    Quit,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
}

impl Drop for InlineTerminal {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        if self.raw {
            let _ = disable_raw_mode();
            self.raw = false;
        }
    }
}

fn styled_text(snapshot: &Snapshot, color: bool) -> Text<'static> {
    let plain = render::verbose(snapshot, ColorMode::Never, false);
    let lines = plain
        .lines()
        .map(|line| {
            let style = if line.starts_with("hwire info") {
                ui_style(color, Color::Rgb(196, 181, 253), Modifier::BOLD)
            } else if line.contains("up  ") {
                ui_style(color, Color::Rgb(52, 211, 153), Modifier::empty())
            } else if line.contains("down") {
                ui_style(color, Color::Rgb(248, 113, 113), Modifier::empty())
            } else if line.trim_start().starts_with('!') {
                ui_style(color, Color::Rgb(250, 204, 21), Modifier::empty())
            } else if matches!(line, "routes" | "ssh resolution") {
                ui_style(color, Color::Rgb(167, 139, 250), Modifier::BOLD)
            } else {
                ui_style(color, Color::Rgb(203, 213, 225), Modifier::empty())
            };
            Line::styled(line.to_string(), style)
        })
        .collect::<Vec<_>>();
    Text::from(lines)
}

fn ui_style(color: bool, foreground: Color, modifier: Modifier) -> Style {
    let style = if color {
        Style::default().fg(foreground)
    } else {
        Style::default()
    };
    style.add_modifier(modifier)
}

#[cfg(test)]
#[path = "../../tests/unit/info/tui_tests.rs"]
mod tests;
