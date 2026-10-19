use std::time::Duration;

use ratatui::widgets::{Block, BorderType};

use super::*;

const ENTER: Duration = Duration::from_millis(180);
const EXIT: Duration = Duration::from_millis(140);
const LIFETIME: Duration = Duration::from_secs(5);
const FRAME: Duration = Duration::from_millis(16);

pub struct ToastView<'a> {
    pub title: &'a str,
    pub loading: bool,
    pub elapsed: Duration,
    pub reduced_motion: bool,
}

impl ToastView<'_> {
    pub fn expired(&self) -> bool {
        !self.loading && self.elapsed >= LIFETIME
    }

    pub fn animating(&self) -> bool {
        !self.reduced_motion
            && !self.expired()
            && (self.loading || self.elapsed < ENTER || self.elapsed >= LIFETIME - EXIT)
    }

    pub fn next_transition(&self) -> Duration {
        if self.animating() {
            if self.loading && self.elapsed >= ENTER {
                Duration::from_millis(80)
            } else {
                FRAME
            }
        } else if self.loading {
            Duration::MAX
        } else {
            let next = if self.reduced_motion {
                LIFETIME
            } else {
                LIFETIME - EXIT
            };
            next.saturating_sub(self.elapsed)
        }
    }

    fn visibility(&self) -> f64 {
        if self.expired() {
            0.0
        } else if self.reduced_motion {
            1.0
        } else if self.elapsed < ENTER {
            let progress = self.elapsed.as_secs_f64() / ENTER.as_secs_f64();
            1.0 - (1.0 - progress).powi(3)
        } else if !self.loading && self.elapsed > LIFETIME - EXIT {
            let progress = (self.elapsed - (LIFETIME - EXIT)).as_secs_f64() / EXIT.as_secs_f64();
            1.0 - progress.powi(3)
        } else {
            1.0
        }
    }
}

pub(super) fn render(buffer: &mut Buffer, area: Rect, toast: &ToastView<'_>) {
    if area.width < 4 || area.height < 4 || toast.expired() {
        return;
    }
    let margin = u16::from(area.width >= 24) * 2;
    let room = Rect::new(
        area.x + margin,
        area.y,
        area.width - margin * 2,
        area.height - 1,
    );
    let title = safe_text(toast.title);
    let width = (title.width() + 10)
        .clamp(26, 64)
        .min(usize::from(room.width)) as u16;
    let inner_width = width.saturating_sub(4).max(1);
    let height = (if toast.loading { 5 } else { 3 }).min(room.height);
    let card = Rect::new(0, 0, width, height);
    let appearance = chrome::badge_style(
        style(if toast.loading {
            Role::Accent
        } else {
            Role::DiffAdded
        }),
        color_depth(),
    );
    let mut painted = Buffer::empty(card);
    Block::bordered()
        .border_type(BorderType::Rounded)
        .style(appearance)
        .render(card, &mut painted);
    let phase = (toast.elapsed.as_millis() / 80) as u64;
    let symbol = if toast.loading {
        ui_progress::Spinner::Braille.frame(phase, !toast.reduced_motion)
    } else {
        "✓"
    };
    let label = format!("{symbol} {title}{}", if toast.loading { "…" } else { "" });
    let (x, y, text_width) = if height >= 3 {
        (2, 1, inner_width)
    } else {
        (0, 0, width)
    };
    put(
        &mut painted,
        x,
        y,
        text_width,
        &elide_end(&label, usize::from(text_width)),
        appearance.add_modifier(Modifier::BOLD),
    );
    if toast.loading && height >= 5 {
        put(&mut painted, 2, 2, inner_width, "Working", appearance);
        let span = usize::from(inner_width);
        let position = if toast.reduced_motion {
            span / 2
        } else {
            let travel = span.saturating_sub(1).max(1);
            let step = phase as usize % (travel * 2);
            step.min(travel * 2 - step)
        };
        for column in 0..span {
            let distance = column.abs_diff(position);
            let symbol = if distance == 0 { "━" } else { "─" };
            let paint = if distance <= 1 {
                appearance.add_modifier(Modifier::BOLD)
            } else {
                appearance.add_modifier(Modifier::DIM)
            };
            put(&mut painted, 2 + column as u16, 3, 1, symbol, paint);
        }
    }
    let offset = (f64::from(height) * (1.0 - toast.visibility())).round() as u16;
    let left = room.x + (room.width - width) / 2;
    for row in offset..height {
        for column in 0..width {
            let cell = &painted[(column, row)];
            if cell.symbol().width() <= usize::from(width - column) {
                buffer[(left + column, room.y + row - offset)] = cell.clone();
            }
        }
    }
}
