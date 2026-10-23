use super::*;
use std::collections::VecDeque;

pub(super) const FRAME: Duration = Duration::from_millis(16);
const RATE_WINDOW: Duration = Duration::from_millis(100);
const PRECISE_RATE: f64 = 30.0;
const RATE_SPAN: f64 = 25.0;
const MAX_SPEED: f64 = 6.0;
const COLUMNS_PER_ROW: f64 = 2.0;
const EASE: Duration = Duration::from_millis(40);

use view::ScrollRegion as Region;

#[derive(Default)]
struct Wheel {
    events: VecDeque<Instant>,
    direction: isize,
    remainder: f64,
}

impl Wheel {
    fn distance(&mut self, direction: isize, unit: f64, now: Instant) -> usize {
        if direction != self.direction {
            self.events.clear();
            self.remainder = 0.0;
            self.direction = direction;
        }
        while self
            .events
            .front()
            .is_some_and(|event| now.duration_since(*event) > RATE_WINDOW)
        {
            self.events.pop_front();
        }
        self.events.push_back(now);
        let rate = self.events.len() as f64 / RATE_WINDOW.as_secs_f64();
        let speed = 1.0 + ((rate - PRECISE_RATE).max(0.0) / RATE_SPAN).powf(1.5);
        let distance = speed.min(MAX_SPEED) * unit + self.remainder;
        self.remainder = distance.fract();
        distance.trunc() as usize
    }
}

struct Glide {
    region: Region,
    position: f64,
    target: f64,
    shown: usize,
    last: Instant,
}

impl Glide {
    fn advance(&mut self, now: Instant, instant: bool) {
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.position = if instant {
            self.target
        } else {
            let progress = 1.0 - (-elapsed / EASE.as_secs_f64()).exp();
            self.position + (self.target - self.position) * progress
        };
        self.shown = if self.target >= self.position {
            self.position.ceil()
        } else {
            self.position.floor()
        } as usize;
        if self.shown == self.target as usize {
            self.position = self.target;
        }
    }

    fn settled(&self) -> bool {
        self.position == self.target
    }
}

#[derive(Default)]
pub(super) struct Scroll {
    rows: Wheel,
    columns: Wheel,
    glides: Vec<Glide>,
}

impl Scroll {
    fn push(
        &mut self,
        region: Region,
        current: usize,
        direction: isize,
        maximum: usize,
        now: Instant,
    ) {
        let distance = if region.horizontal() {
            self.columns.distance(direction, COLUMNS_PER_ROW, now)
        } else {
            self.rows.distance(direction, 1.0, now)
        };
        let index = match self
            .glides
            .iter()
            .position(|glide| glide.region == region && glide.shown == current)
        {
            Some(index) => index,
            None => {
                self.glides.retain(|glide| glide.region != region);
                self.glides.push(Glide {
                    region,
                    position: current as f64,
                    target: current as f64,
                    shown: current,
                    last: now,
                });
                self.glides.len() - 1
            }
        };
        let glide = &mut self.glides[index];
        let travel = glide.target - glide.shown as f64;
        if travel != 0.0 && (travel > 0.0) != (direction > 0) {
            glide.position = glide.shown as f64;
            glide.target = glide.position;
        }
        let base = glide.target as usize;
        glide.target = if direction > 0 {
            base.saturating_add(distance).min(maximum)
        } else {
            base.saturating_sub(distance)
        } as f64;
    }

    fn advance(
        &mut self,
        now: Instant,
        instant: bool,
        mut apply: impl FnMut(Region, usize, usize) -> bool,
    ) {
        self.glides.retain_mut(|glide| {
            let from = glide.shown;
            glide.advance(now, instant || !glide.region.horizontal());
            apply(glide.region, from, glide.shown) && !glide.settled()
        });
    }

    pub(super) fn stop(&mut self) {
        self.glides.clear();
    }

    pub(super) fn active(&self) -> bool {
        !self.glides.is_empty()
    }
}

impl App {
    pub(super) fn reset_scrollbar(&mut self) {
        if self.view.scrollbar_hovered.take().is_some() || self.view.scrollbar_drag.is_some() {
            self.view.scrollbar_drag = None;
            self.dirty = true;
        }
    }

    pub(super) fn scrollbar_mouse(&mut self, event: MouseEvent) -> bool {
        let point = ratatui::layout::Position::new(event.column, event.row);
        let drag = self.view.scrollbar_drag;
        let bar = self.geometry.as_ref().and_then(|geometry| {
            geometry.scrollbars.iter().copied().find(|(region, bar)| {
                self.dialog.is_some() == region.overlay()
                    && if let Some(drag) = drag {
                        drag.region == *region
                    } else {
                        bar.offset > 0 && bar.track.contains(point)
                    }
            })
        });
        let Some((region, bar)) = bar else {
            self.reset_scrollbar();
            return false;
        };
        let hovered = (bar.track.contains(point) && !self.dragging).then_some(region);
        if self.view.scrollbar_hovered != hovered {
            self.view.scrollbar_hovered = hovered;
            self.dirty = true;
        }
        if let Some(drag) = drag {
            match event.kind {
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
                    self.set_scroll_offset(region, bar.drag(event.row, drag.row, drag.offset));
                    if event.kind == MouseEventKind::Up(MouseButton::Left) {
                        self.view.scrollbar_drag = None;
                    }
                    self.dirty = true;
                    return true;
                }
                MouseEventKind::Moved => {
                    self.view.scrollbar_drag = None;
                    self.dirty = true;
                }
                _ => {}
            }
        }
        if hovered.is_none() {
            return false;
        }
        self.view.nav_hovered = None;
        self.view.divider_hovered = false;
        self.view.divider_hover_row = None;
        if event.kind == MouseEventKind::Down(MouseButton::Left) {
            match region {
                Region::Navigator => self.view.focus = Focus::Navigator,
                Region::Canvas => self.view.focus = Focus::Canvas,
                Region::CommitDetails => self.view.focus = Focus::CommitDetails,
                _ => {}
            }
            self.manual_scroll(region);
            let offset = self.scroll_offset(region).unwrap_or(bar.offset);
            if bar.grabs_thumb(event.row) {
                self.view.scrollbar_drag = Some(view::ScrollbarDrag {
                    region,
                    row: event.row,
                    offset,
                });
            } else {
                self.set_scroll_offset(region, bar.page(event.row, offset));
            }
            self.dirty = true;
        }
        true
    }

    fn manual_scroll(&mut self, region: Region) {
        match region {
            Region::Navigator => self.view.nav_manual_scroll = true,
            Region::Canvas => self.view.canvas_manual_scroll = true,
            Region::CommitDetails => self.view.commit_manual_scroll = true,
            Region::Editor => self.view.overlay_scroll.editor_manual = true,
            Region::Commands => self.view.overlay_scroll.commands_manual = true,
            _ => {}
        }
    }

    pub(super) fn wheel(&mut self, event: MouseEvent) {
        self.reset_scrollbar();
        if self.view.nav_hovered.take().is_some() {
            self.dirty = true;
        }
        let horizontal = matches!(
            event.kind,
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
        ) || event.modifiers.contains(KeyModifiers::SHIFT);
        let direction = if matches!(
            event.kind,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollRight
        ) {
            1
        } else {
            -1
        };
        let point = ratatui::layout::Position::new(event.column, event.row);
        let Some((region, maximum)) = self.wheel_region(point, horizontal) else {
            return;
        };
        let Some(current) = self.scroll_offset(region) else {
            return;
        };
        self.manual_scroll(region);
        self.scroll
            .push(region, current, direction, maximum, Instant::now());
        self.dirty = true;
    }

    pub(super) fn animate_scroll(&mut self, reduced_motion: bool) -> bool {
        if !self.scroll.active() {
            return false;
        }
        let mut scroll = std::mem::take(&mut self.scroll);
        scroll.advance(Instant::now(), reduced_motion, |region, from, to| {
            let valid = self.scroll_offset(region) == Some(from);
            if valid && from != to {
                self.set_scroll_offset(region, to);
                self.dirty = true;
            }
            valid
        });
        self.scroll = scroll;
        self.scroll.active()
    }

    fn wheel_region(
        &self,
        point: ratatui::layout::Position,
        horizontal: bool,
    ) -> Option<(Region, usize)> {
        let geometry = self.geometry.as_ref()?;
        if !horizontal
            && let Some((region, bar)) = geometry.scrollbars.iter().find(|(region, bar)| {
                self.dialog.is_some() == region.overlay() && bar.track.contains(point)
            })
        {
            return Some((*region, bar.maximum));
        }
        let maximum = match &self.dialog {
            Some(Dialog::Inspect { message, .. }) => {
                view::inspection_scroll_max(message, geometry.overlay_bounds)
            }
            Some(Dialog::Confirm { detail, .. }) => {
                view::confirmation_scroll_max(detail, geometry.overlay_bounds)
            }
            Some(Dialog::Error { .. } | Dialog::Compose { .. } | Dialog::Input { .. }) => {
                return (!horizontal)
                    .then(|| geometry.scrollbar(Region::Editor))
                    .flatten()
                    .map(|bar| (Region::Editor, bar.maximum));
            }
            Some(Dialog::Actions { .. }) => {
                return (!horizontal)
                    .then(|| geometry.scrollbar(Region::Commands))
                    .flatten()
                    .map(|bar| (Region::Commands, bar.maximum));
            }
            None => 0,
        };
        if self.dialog.is_some() {
            return (!horizontal).then_some((Region::Dialog, usize::from(maximum)));
        }
        if geometry.commit_details.contains(point) {
            return (!horizontal).then_some((Region::CommitDetails, geometry.commit_scroll_max));
        }
        if geometry.nav.contains(point) {
            return Some(if horizontal {
                (
                    Region::NavigatorColumns,
                    self.nav_layout.horizontal_limit(
                        &self.nav_items,
                        geometry.nav.width,
                        self.view.workspace,
                    ),
                )
            } else {
                (Region::Navigator, geometry.nav_scroll_max)
            });
        }
        if geometry.canvas.contains(point) {
            return Some(if horizontal {
                (Region::CanvasColumns, self.canvas_horizontal_limit())
            } else {
                (
                    Region::Canvas,
                    geometry.scrollbar(Region::Canvas).map_or_else(
                        || {
                            self.row_map
                                .len()
                                .max(self.content_line_count)
                                .saturating_sub(usize::from(geometry.canvas.height))
                        },
                        |bar| bar.maximum,
                    ),
                )
            });
        }
        None
    }

    fn scroll_offset(&self, region: Region) -> Option<usize> {
        Some(match region {
            Region::Navigator => self.view.offset,
            Region::NavigatorColumns => self.view.nav_horizontal,
            Region::Canvas => self.view.diff_offset,
            Region::CanvasColumns => self.view.horizontal,
            Region::CommitDetails => self.view.commit_offset,
            Region::Editor => self.view.overlay_scroll.editor,
            Region::Commands => self.view.overlay_scroll.commands,
            Region::Dialog => match &self.dialog {
                Some(Dialog::Inspect { scroll, .. } | Dialog::Confirm { scroll, .. }) => {
                    usize::from(*scroll)
                }
                _ => return None,
            },
        })
    }

    fn set_scroll_offset(&mut self, region: Region, offset: usize) {
        match region {
            Region::Navigator => self.view.offset = offset,
            Region::NavigatorColumns => self.view.nav_horizontal = offset,
            Region::Canvas => self.view.diff_offset = offset,
            Region::CanvasColumns => self.view.horizontal = offset,
            Region::CommitDetails => self.view.commit_offset = offset,
            Region::Editor => self.view.overlay_scroll.editor = offset,
            Region::Commands => self.view.overlay_scroll.commands = offset,
            Region::Dialog => {
                if let Some(Dialog::Inspect { scroll, .. } | Dialog::Confirm { scroll, .. }) =
                    &mut self.dialog
                {
                    *scroll = u16::try_from(offset).unwrap_or(u16::MAX);
                }
            }
        }
    }
}
