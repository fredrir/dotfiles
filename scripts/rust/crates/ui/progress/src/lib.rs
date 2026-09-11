#[cfg(feature = "ratatui")]
use ratatui::buffer::Buffer;
#[cfg(feature = "ratatui")]
use ratatui::layout::Rect;
#[cfg(feature = "ratatui")]
use ratatui::style::Modifier;
#[cfg(feature = "ratatui")]
use ratatui::text::{Line, Span};
#[cfg(feature = "ratatui")]
use ratatui::widgets::{Gauge, Paragraph, Widget};
#[cfg(feature = "ratatui")]
use ui_theme::{ColorMode, Palette, Role};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    pub completed: u64,
    pub total: Option<u64>,
}

impl Progress {
    pub fn fraction(self) -> Option<f64> {
        self.total.map(|total| {
            if total == 0 {
                f64::from(self.completed > 0)
            } else {
                (self.completed as f64 / total as f64).clamp(0.0, 1.0)
            }
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Spinner {
    #[default]
    Braille,
    Quarter,
}

impl Spinner {
    pub fn frame(self, index: u64, motion: bool) -> &'static str {
        if !motion {
            return "◌";
        }
        let frames: &[&str] = match self {
            Self::Braille => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            Self::Quarter => &["◐", "◓", "◑", "◒"],
        };
        frames[index as usize % frames.len()]
    }
}

#[cfg(feature = "ratatui")]
pub struct ProgressBar<'a> {
    pub progress: Progress,
    pub frame: u64,
    pub palette: &'a Palette,
    pub color: bool,
}

#[cfg(feature = "ratatui")]
impl Widget for ProgressBar<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let style = |role| {
            self.palette.ratatui(
                if self.color {
                    ColorMode::Always
                } else {
                    ColorMode::Never
                },
                true,
                role,
            )
        };
        if let Some(ratio) = self.progress.fraction() {
            Gauge::default()
                .gauge_style(
                    style(Role::Background)
                        .patch(style(Role::Accent))
                        .add_modifier(Modifier::BOLD),
                )
                .ratio(ratio)
                .label(format!(
                    "{} / {}  |  {:.0}%",
                    self.progress.completed,
                    self.progress.total.unwrap_or_default(),
                    ratio * 100.0
                ))
                .render(area, buffer);
        } else {
            let position = self.frame as usize % usize::from(area.width);
            let spans = (0..usize::from(area.width)).map(|index| {
                let (symbol, role) = if index == position {
                    ("━", Role::Accent)
                } else if index.abs_diff(position) == 1 {
                    ("━", Role::Muted)
                } else {
                    ("─", Role::Border)
                };
                Span::styled(symbol, style(role))
            });
            Paragraph::new(Line::from(spans.collect::<Vec<_>>())).render(area, buffer);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseState {
    Pending,
    Active,
    Complete,
}

#[cfg(feature = "ratatui")]
pub fn phase_track(phases: &[(&str, PhaseState)], palette: &Palette, color: bool) -> Line<'static> {
    let mut spans = Vec::with_capacity(phases.len().saturating_mul(2));
    let mode = if color {
        ColorMode::Always
    } else {
        ColorMode::Never
    };
    for (index, (label, state)) in phases.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(
                " ━━━━━ ",
                palette.ratatui(mode, true, Role::Border),
            ));
        }
        let (symbol, role) = match state {
            PhaseState::Pending => ("○", Role::Muted),
            PhaseState::Active => ("◉", Role::Info),
            PhaseState::Complete => ("●", Role::Success),
        };
        spans.push(Span::styled(
            format!("{symbol} {label}"),
            palette.ratatui(mode, true, role),
        ));
    }
    Line::from(spans)
}

#[cfg(feature = "ratatui")]
pub fn activity(
    label: &str,
    detail: &str,
    frame: u64,
    palette: &Palette,
    color: bool,
) -> Line<'static> {
    let mode = if color {
        ColorMode::Always
    } else {
        ColorMode::Never
    };
    Line::from(vec![
        Span::styled(
            format!("{} {label}", Spinner::Quarter.frame(frame, true)),
            palette
                .ratatui(mode, true, Role::Info)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(detail.to_owned(), palette.ratatui(mode, true, Role::Muted)),
    ])
}

pub fn transfer(files: u64, formatted_bytes: &str) -> String {
    format!(
        "▸ {files} {} {formatted_bytes}",
        if files == 1 { "file" } else { "files" }
    )
}

pub fn transfer_styled(style: &ui_theme::Style, files: u64, formatted_bytes: &str) -> String {
    format!(
        "  {} {} {}",
        style.dim("▸"),
        style.bold(&format!(
            "{files} {}",
            if files == 1 { "file" } else { "files" }
        )),
        style.dim(formatted_bytes),
    )
}

pub struct Reporter<W: std::io::Write> {
    output: W,
    last_draw: Option<std::time::Instant>,
    visible: bool,
}

impl<W: std::io::Write> Reporter<W> {
    pub fn new(output: W) -> Self {
        Self {
            output,
            last_draw: None,
            visible: false,
        }
    }

    pub fn update(&mut self, line: &str) -> std::io::Result<()> {
        let now = std::time::Instant::now();
        if self
            .last_draw
            .is_some_and(|last| now.duration_since(last) < std::time::Duration::from_millis(80))
        {
            return Ok(());
        }
        self.visible = true;
        write!(self.output, "\r\x1b[2K{line}")?;
        self.output.flush()?;
        self.last_draw = Some(now);
        Ok(())
    }

    pub fn finish(&mut self) -> std::io::Result<()> {
        if self.visible {
            write!(self.output, "\r\x1b[2K")?;
            self.output.flush()?;
            self.visible = false;
        }
        Ok(())
    }
}

impl<W: std::io::Write> Drop for Reporter<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
