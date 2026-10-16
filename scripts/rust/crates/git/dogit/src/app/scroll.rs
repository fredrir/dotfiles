use super::*;
use std::collections::VecDeque;

pub(super) const FRAME: Duration = Duration::from_millis(16);
const RATE_WINDOW: Duration = Duration::from_millis(100);
const PRECISE_RATE: f64 = 30.0;
const RATE_SPAN: f64 = 25.0;
const MAX_SPEED: f64 = 6.0;
const COLUMNS_PER_ROW: f64 = 2.0;
const EASE: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Region {
    Navigator,
    NavigatorColumns,
    Canvas,
    CanvasColumns,
    CommitDetails,
    Dialog,
}

impl Region {
    fn horizontal(self) -> bool {
        matches!(self, Self::NavigatorColumns | Self::CanvasColumns)
    }
}

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
        // WezTerm reports one event per trackpad delta, so event density stands in for velocity
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
            glide.advance(now, instant);
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
    pub(super) fn wheel(&mut self, event: MouseEvent) {
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
        match region {
            Region::Navigator => self.view.nav_manual_scroll = true,
            Region::Canvas => self.view.canvas_manual_scroll = true,
            Region::CommitDetails => self.view.commit_manual_scroll = true,
            _ => {}
        }
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
        let maximum = match &self.dialog {
            Some(Dialog::Inspect { message, .. }) => {
                view::inspection_scroll_max(message, geometry.overlay_bounds)
            }
            Some(Dialog::Confirm { detail, .. }) => {
                view::confirmation_scroll_max(detail, geometry.overlay_bounds)
            }
            Some(_) => return None,
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
                    view::nav_horizontal_limit(
                        &self.nav_items,
                        geometry.nav.width,
                        self.view.workspace,
                    ),
                )
            } else {
                (
                    Region::Navigator,
                    self.entries
                        .len()
                        .saturating_sub(usize::from(geometry.nav.height)),
                )
            });
        }
        if geometry.canvas.contains(point) {
            return Some(if horizontal {
                (Region::CanvasColumns, self.canvas_horizontal_limit())
            } else {
                (
                    Region::Canvas,
                    self.row_map
                        .len()
                        .max(self.content_line_count)
                        .saturating_sub(usize::from(geometry.canvas.height)),
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
