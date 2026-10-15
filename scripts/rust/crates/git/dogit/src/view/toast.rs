use std::time::Duration;

use ratatui::widgets::{Block, BorderType};

use super::*;

const ENTER: Duration = Duration::from_millis(180);
const EXIT: Duration = Duration::from_millis(140);
const LIFETIME: Duration = Duration::from_secs(5);
const FRAME: Duration = Duration::from_millis(16);

pub struct ToastView<'a> {
    pub title: &'a str,
    pub detail: &'a str,
    pub failed: bool,
    pub elapsed: Duration,
    pub reduced_motion: bool,
}

impl ToastView<'_> {
    pub fn expired(&self) -> bool {
        !self.failed && self.elapsed >= LIFETIME
    }

    pub fn animating(&self) -> bool {
        !self.reduced_motion
            && !self.expired()
            && (self.elapsed < ENTER || (!self.failed && self.elapsed >= LIFETIME - EXIT))
    }

    pub fn next_transition(&self) -> Duration {
        if self.animating() {
            FRAME
        } else if self.failed {
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
        } else if !self.failed && self.elapsed > LIFETIME - EXIT {
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
    let detail = safe_text_multiline(toast.detail);
    let natural_width = (title.width() + 8).max(if toast.failed {
        detail
            .lines()
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0)
            + 4
    } else {
        0
    });
    let width = natural_width.clamp(22, 64).min(usize::from(room.width)) as u16;
    let inner_width = width.saturating_sub(4).max(1);
    let detail_rows = if toast.failed && !detail.is_empty() {
        detail
            .lines()
            .map(|line| line.width().div_ceil(usize::from(inner_width)).max(1))
            .sum::<usize>()
            .min(3) as u16
    } else {
        0
    };
    let height = (3 + detail_rows + u16::from(toast.failed)).min(room.height);
    let card = Rect::new(0, 0, width, height);
    let appearance = chrome::badge_style(
        style(if toast.failed {
            Role::DiffRemoved
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
    let label = format!("{} {}", if toast.failed { "×" } else { "✓" }, title);
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
    if height >= 5 && detail_rows > 0 {
        Paragraph::new(detail)
            .style(appearance)
            .wrap(Wrap { trim: true })
            .render(
                Rect::new(2, 2, inner_width, detail_rows.min(height - 4)),
                &mut painted,
            );
    }
    if toast.failed && height >= 4 {
        put(
            &mut painted,
            2,
            height - 2,
            inner_width,
            "Esc dismiss",
            appearance,
        );
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
