use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::style::Modifier;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tachyonfx::{CellFilter, Effect, EffectRenderer, Interpolation, fx};
use ui_progress::Spinner;
use ui_terminal::{Inline, SignalGuard, Teardown, termination_requested};
use ui_theme::{Palette, Role, ThemeHandle};

use super::model::Snapshot;
use super::{ColorMode, Options, collect, render};

const FRAME: Duration = Duration::from_millis(33);
const IDLE_POLL: Duration = Duration::from_millis(100);

pub fn capable() -> bool {
    ui_terminal::capable(
        io::stdin().is_terminal(),
        io::stdout().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
        std::env::var("CI").ok().as_deref(),
    )
}

pub fn run(options: Options) -> Result<(), String> {
    let color = options.color.enabled(true);
    let motion = !ui_terminal::reduced_motion_requested("HWIRE_REDUCED_MOTION") && color;
    let mut theme = ThemeHandle::discover();
    let mut height = workstation::terminal_height()
        .unwrap_or(24)
        .saturating_sub(1)
        .max(6) as u16;
    let signals =
        SignalGuard::new().map_err(|error| format!("unable to watch terminal signals: {error}"))?;
    let mut inline = Inline::with_signals(io::stdout(), height, Teardown::KeepViewport, signals)
        .map_err(|error| format!("unable to open verbose terminal: {error}"))?;
    let mut snapshot = None;
    let mut text = None;
    let mut width = inline
        .terminal()
        .size()
        .map_err(|error| error.to_string())?
        .width;
    let mut content_height = 0;
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
        if termination_requested() {
            return Ok(());
        }
        if theme.poll() {
            text = snapshot.as_ref().map(|snapshot| {
                Paragraph::new(styled_text(snapshot, theme.palette(), color))
                    .wrap(Wrap { trim: false })
            });
            content_height = text
                .as_ref()
                .map_or(0, |text| measured_height(text, width.saturating_sub(2)));
            dirty = true;
        }
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
                        bell(inline.terminal())?;
                    }
                    failure = next.failure();
                    previous_preferred = Some(primary_route);
                    previous_fingerprint = Some(fingerprint);
                    text = Some(
                        Paragraph::new(styled_text(&next, theme.palette(), color))
                            .wrap(Wrap { trim: false }),
                    );
                    content_height =
                        measured_height(text.as_ref().unwrap(), width.saturating_sub(2));
                    snapshot = Some(next);
                    scroll = scroll.min(scroll_limit(content_height, height));
                    worker = None;
                    if motion && changed {
                        effect = Some(reveal_effect(theme.palette(), color));
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

        let max_scroll = scroll_limit(content_height, height);
        match input()? {
            Input::None => {}
            Input::Resize(next_width, next_height) => {
                width = next_width;
                height = next_height.saturating_sub(1).max(1);
                inline
                    .resize_viewport(io::stdout(), height)
                    .map_err(|error| error.to_string())?;
                content_height = text
                    .as_ref()
                    .map_or(0, |text| measured_height(text, width.saturating_sub(2)));
                scroll = scroll.min(scroll_limit(content_height, height));
                dirty = true;
            }
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
            text = text.take().map(|paragraph| paragraph.scroll((scroll, 0)));
            draw(
                inline.terminal(),
                theme.palette(),
                DrawState {
                    text: text.as_ref(),
                    frame_index,
                    color,
                    probing: worker.is_some(),
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

fn reveal_effect(palette: &Palette, color: bool) -> Effect {
    let foreground = palette
        .ratatui(
            if color {
                ColorMode::Always
            } else {
                ColorMode::Never
            },
            true,
            Role::Border,
        )
        .fg
        .unwrap_or_default();
    fx::fade_from_fg(foreground, (180, Interpolation::CubicOut)).with_filter(CellFilter::Text)
}

fn measured_height(text: &Paragraph<'_>, width: u16) -> usize {
    text.line_count(width.max(1))
}

fn scroll_limit(content_height: usize, viewport_height: u16) -> u16 {
    content_height
        .saturating_sub(usize::from(viewport_height.saturating_sub(2)))
        .min(usize::from(u16::MAX)) as u16
}

struct DrawState<'a> {
    text: Option<&'a Paragraph<'static>>,
    frame_index: u64,
    color: bool,
    probing: bool,
    tick: Duration,
}

fn draw(
    terminal: &mut Terminal<ui_terminal::Backend<Stdout>>,
    palette: &Palette,
    state: DrawState<'_>,
    effect: Option<&mut Effect>,
) -> Result<(), String> {
    let DrawState {
        text,
        frame_index,
        color,
        probing,
        tick,
    } = state;
    terminal
        .draw(|frame| {
            let area = frame.area();
            let title = if probing {
                format!(" {} probing ", Spinner::Braille.frame(frame_index, true))
            } else {
                " hwire info ".to_string()
            };
            let block = Block::default()
                .style(palette.ratatui(
                    if color {
                        ColorMode::Always
                    } else {
                        ColorMode::Never
                    },
                    true,
                    Role::Background,
                ))
                .borders(Borders::ALL)
                .title(format!("{title} | ↑/↓ scroll | q quit "))
                .border_style(
                    palette
                        .ratatui(
                            if color {
                                ColorMode::Always
                            } else {
                                ColorMode::Never
                            },
                            true,
                            Role::Accent,
                        )
                        .add_modifier(Modifier::BOLD),
                );
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let placeholder = Paragraph::new(Line::styled(
                "Discovering routes…",
                palette.ratatui(
                    if color {
                        ColorMode::Always
                    } else {
                        ColorMode::Never
                    },
                    true,
                    Role::Muted,
                ),
            ));
            let text = text.unwrap_or(&placeholder);
            frame.render_widget(text, inner);
            if let Some(effect) = effect {
                frame.render_effect(effect, area, tachyonfx::Duration::from(tick));
            }
        })
        .map(|_| ())
        .map_err(|error| format!("unable to render verbose information: {error}"))
}

fn input() -> Result<Input, String> {
    while event::poll(Duration::ZERO)
        .map_err(|error| format!("unable to poll terminal input: {error}"))?
    {
        let key = match event::read()
            .map_err(|error| format!("unable to read terminal input: {error}"))?
        {
            Event::Key(key) => key,
            Event::Resize(width, height) => return Ok(Input::Resize(width, height)),
            _ => continue,
        };
        if key.kind == KeyEventKind::Press
            && (key.code == KeyCode::Char('q')
                || key.code == KeyCode::Esc
                || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
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

fn bell(terminal: &mut Terminal<ui_terminal::Backend<Stdout>>) -> Result<(), String> {
    terminal
        .backend_mut()
        .write_all(b"\x07")
        .and_then(|_| terminal.backend_mut().flush())
        .map_err(|error| format!("unable to ring route-change bell: {error}"))
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
    Resize(u16, u16),
}

fn styled_text(snapshot: &Snapshot, palette: &Palette, color: bool) -> Text<'static> {
    let plain = render::verbose(snapshot, ColorMode::Never, false);
    let mode = if color {
        ColorMode::Always
    } else {
        ColorMode::Never
    };
    Text::from(
        plain
            .lines()
            .map(|line| {
                let role = if line.starts_with("hwire info") {
                    Role::Strong
                } else if line.contains("up  ") {
                    Role::Success
                } else if line.contains("down") {
                    Role::Danger
                } else if line.trim_start().starts_with('!') {
                    Role::Warning
                } else if matches!(line, "routes" | "ssh resolution") {
                    Role::Accent
                } else {
                    Role::Plain
                };
                Line::styled(line.to_owned(), palette.ratatui(mode, true, role))
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
#[path = "../../tests/unit/info/tui_tests.rs"]
mod tests;
