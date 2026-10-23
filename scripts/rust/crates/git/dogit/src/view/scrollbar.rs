use super::*;

const GUTTER_WIDTH: u16 = 3;
const SUBROWS: usize = 2;

#[derive(Clone, Copy, Debug)]
pub struct Scrollbar {
    pub gutter: Rect,
    pub thumb: Rect,
    pub maximum: usize,
    pub visible: usize,
    start: usize,
    length: usize,
}

impl Scrollbar {
    pub(super) fn new(
        area: Rect,
        total: usize,
        visible: usize,
        offset: usize,
        thumb_limit: u16,
    ) -> Option<Self> {
        let maximum = total.saturating_sub(visible);
        if maximum == 0 || area.height < 2 || area.width <= GUTTER_WIDTH {
            return None;
        }
        let gutter = Rect::new(
            area.right() - GUTTER_WIDTH,
            area.y,
            GUTTER_WIDTH,
            area.height,
        );
        let span = usize::from(area.height) * SUBROWS;
        let length = scale(visible, span, total)
            .min(usize::from(thumb_limit) * SUBROWS)
            .max(2 * SUBROWS)
            .min(span - SUBROWS);
        let offset = offset.min(maximum);
        let start = scale(offset, span - length, maximum);
        let top = (start / SUBROWS) as u16;
        let height = (start % SUBROWS + length).div_ceil(SUBROWS) as u16;
        Some(Self {
            gutter,
            thumb: Rect::new(gutter.x + 1, gutter.y + top, 1, height),
            maximum,
            visible,
            start,
            length,
        })
    }

    pub fn content_width(self, area: Rect) -> u16 {
        self.gutter.x.saturating_sub(area.x)
    }

    pub fn grabs_thumb(self, row: u16) -> bool {
        row >= self.thumb.y && row < self.thumb.bottom()
    }

    pub fn page(self, row: u16, offset: usize) -> usize {
        let page = self.visible.saturating_sub(1).max(1);
        if row < self.thumb.y {
            offset.saturating_sub(page)
        } else {
            offset.saturating_add(page).min(self.maximum)
        }
    }

    pub fn drag(self, row: u16, origin_row: u16, origin_offset: usize) -> usize {
        let origin_offset = origin_offset.min(self.maximum);
        let delta = (i128::from(row) - i128::from(origin_row)) * SUBROWS as i128;
        if delta == 0 {
            return origin_offset;
        }
        let travel = usize::from(self.gutter.height) * SUBROWS - self.length;
        let top = scale(origin_offset, travel, self.maximum) as i128 + delta;
        if top <= 0 {
            return 0;
        }
        if top >= travel as i128 {
            return self.maximum;
        }
        let distance =
            (delta.unsigned_abs() * self.maximum as u128 + travel as u128 / 2) / travel as u128;
        (origin_offset as i128 + delta.signum() * distance as i128).clamp(0, self.maximum as i128)
            as usize
    }

    pub(super) fn render(self, buffer: &mut Buffer, state: &ViewState) {
        let interacting = state.overlay.is_none()
            && (state.nav_scrollbar_hovered || state.nav_scrollbar_drag.is_some());
        let appearance = style(if interacting {
            Role::Accent
        } else if state.focus == Focus::Navigator {
            Role::Muted
        } else {
            Role::Border
        });
        for y in self.gutter.y..self.gutter.bottom() {
            put(
                buffer,
                self.gutter.x,
                y,
                self.gutter.width,
                "   ",
                style(Role::Plain),
            );
        }
        for y in self.thumb.y..self.thumb.bottom() {
            let row = usize::from(y - self.gutter.y) * SUBROWS;
            let start = self.start.saturating_sub(row).min(SUBROWS);
            let end = (self.start + self.length).saturating_sub(row).min(SUBROWS);
            let (symbol, appearance) = if start > 0 {
                ("▄", appearance)
            } else if end < SUBROWS {
                ("▀", appearance)
            } else {
                ("█", appearance.bg(appearance.fg.unwrap_or(Color::Reset)))
            };
            put(buffer, self.thumb.x, y, 1, symbol, appearance);
        }
    }
}

fn scale(value: usize, span: usize, maximum: usize) -> usize {
    ((value as u128 * span as u128 + maximum as u128 / 2) / maximum as u128) as usize
}
