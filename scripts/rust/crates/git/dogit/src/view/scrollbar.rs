use super::*;

const SUBROWS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollRegion {
    Navigator,
    NavigatorColumns,
    Canvas,
    CanvasColumns,
    CommitDetails,
    Dialog,
    Editor,
    Commands,
}

impl ScrollRegion {
    pub fn horizontal(self) -> bool {
        matches!(self, Self::NavigatorColumns | Self::CanvasColumns)
    }

    pub fn overlay(self) -> bool {
        matches!(self, Self::Dialog | Self::Editor | Self::Commands)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollbarDrag {
    pub region: ScrollRegion,
    pub row: u16,
    pub offset: usize,
}

#[derive(Clone, Debug, Default)]
pub struct OverlayScroll {
    pub editor: usize,
    pub editor_manual: bool,
    pub commands: usize,
    pub commands_manual: bool,
}

impl Geometry {
    pub fn scrollbar(&self, region: ScrollRegion) -> Option<Scrollbar> {
        self.scrollbars
            .iter()
            .find(|(key, _)| *key == region)
            .map(|(_, bar)| *bar)
    }

    pub(super) fn add_scrollbar(
        &mut self,
        region: ScrollRegion,
        area: Rect,
        total: usize,
        offset: usize,
    ) {
        if let Some(bar) = Scrollbar::new(area, total, usize::from(area.height), offset, 3) {
            self.scrollbars.push((region, bar));
        }
    }

    pub(super) fn render_scrollbars(&self, buffer: &mut Buffer, state: &ViewState) {
        for &(region, bar) in &self.scrollbars {
            if state.overlay.is_some() == region.overlay() && bar.offset > 0 {
                bar.render(buffer, state, region);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Scrollbar {
    pub track: Rect,
    pub thumb: Rect,
    pub maximum: usize,
    pub visible: usize,
    pub offset: usize,
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
        if maximum == 0 || area.height < 2 || area.width == 0 {
            return None;
        }
        let track = Rect::new(area.right() - 1, area.y, 1, area.height);
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
            track,
            thumb: Rect::new(track.x, track.y + top, 1, height),
            maximum,
            visible,
            offset,
            start,
            length,
        })
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
        let travel = usize::from(self.track.height) * SUBROWS - self.length;
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

    pub(super) fn render(self, buffer: &mut Buffer, state: &ViewState, region: ScrollRegion) {
        let interacting = state.scrollbar_hovered == Some(region)
            || state
                .scrollbar_drag
                .is_some_and(|drag| drag.region == region);
        let focused = match region {
            ScrollRegion::Navigator => state.focus == Focus::Navigator,
            ScrollRegion::Canvas => state.focus == Focus::Canvas,
            ScrollRegion::CommitDetails => state.focus == Focus::CommitDetails,
            _ => region.overlay(),
        };
        let appearance = style(if interacting {
            Role::Accent
        } else if focused {
            Role::Muted
        } else {
            Role::Border
        });
        for y in self.thumb.y..self.thumb.bottom() {
            // Clear a wide glyph that crosses the thumb column so it cannot hide the overlay.
            if self.thumb.x > buffer.area.x {
                let previous = &mut buffer[(self.thumb.x - 1, y)];
                if previous.symbol().width() > 1 {
                    previous.set_symbol(" ");
                }
            }
            let appearance = appearance.bg(buffer[(self.thumb.x, y)].bg);
            let row = usize::from(y - self.track.y) * SUBROWS;
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
