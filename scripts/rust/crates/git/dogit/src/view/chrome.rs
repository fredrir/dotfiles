use super::*;
use std::time::{Duration, Instant};

const TAB_TRANSITION: Duration = Duration::from_millis(90);

#[derive(Clone, Debug)]
pub struct TabIndicator {
    from: (f64, f64),
    to: (f64, f64),
    emphasis: [f64; Workspace::ALL.len()],
    workspace: Workspace,
    started: Instant,
}

impl TabIndicator {
    pub(super) fn moving(&self) -> bool {
        self.from != self.to || self.emphasis != tab_emphasis(self.workspace)
    }

    fn settled(workspace: Workspace, bounds: (f64, f64), now: Instant) -> Self {
        Self {
            from: bounds,
            to: bounds,
            emphasis: tab_emphasis(workspace),
            workspace,
            started: now,
        }
    }

    fn sample(&self, now: Instant) -> ((f64, f64), [f64; Workspace::ALL.len()]) {
        let progress = (now.saturating_duration_since(self.started).as_secs_f64()
            / TAB_TRANSITION.as_secs_f64())
        .min(1.0);
        let eased = 1.0 - (1.0 - progress).powi(3);
        let target = tab_emphasis(self.workspace);
        let emphasis = std::array::from_fn(|index| {
            self.emphasis[index] + (target[index] - self.emphasis[index]) * eased
        });
        (
            (
                self.from.0 + (self.to.0 - self.from.0) * eased,
                self.from.1 + (self.to.1 - self.from.1) * eased,
            ),
            emphasis,
        )
    }
}

fn tab_emphasis(workspace: Workspace) -> [f64; Workspace::ALL.len()] {
    Workspace::ALL.map(|item| f64::from(u8::from(item == workspace)))
}

pub(super) fn render_tabs(
    frame: &mut Frame,
    area: Rect,
    state: &mut ViewState,
) -> Vec<(Workspace, Rect)> {
    render_tabs_at(
        frame,
        area,
        state,
        Instant::now(),
        ui_terminal::reduced_motion_requested("DOGIT_REDUCED_MOTION"),
    )
}

pub(super) fn render_tabs_at(
    frame: &mut Frame,
    area: Rect,
    state: &mut ViewState,
    now: Instant,
    reduced_motion: bool,
) -> Vec<(Workspace, Rect)> {
    if area.height < 3 || area.width < 4 {
        state.tab_indicator = None;
        return Vec::new();
    }
    let active = state.workspace;
    put(
        frame.buffer_mut(),
        area.x,
        area.y + 2,
        area.width,
        &"─".repeat(usize::from(area.width)),
        style(Role::Border),
    );
    let available = area.width.saturating_sub(4);
    let widths = Workspace::ALL.map(|workspace| workspace.label().width() as u16 + 6);
    let selected = Workspace::ALL
        .iter()
        .position(|workspace| *workspace == active)
        .unwrap_or(0);
    let mut start = 0;
    while start < selected && widths[start..=selected].iter().sum::<u16>() > available {
        start += 1;
    }
    let mut tabs = Vec::new();
    let mut x = area.x + 2;
    for (index, workspace) in Workspace::ALL.into_iter().enumerate().skip(start) {
        let width = widths[index];
        if x + width > area.right().saturating_sub(2) {
            break;
        }
        tabs.push((workspace, Rect::new(x, area.y + 1, width - 1, 2)));
        x += width;
    }
    if let Some((_, target)) = tabs.iter().find(|(workspace, _)| *workspace == active) {
        let bounds = (f64::from(target.x), f64::from(target.right() - 1));
        match &mut state.tab_indicator {
            Some(indicator) if indicator.workspace != active && !reduced_motion => {
                let (from, emphasis) = indicator.sample(now);
                *indicator = TabIndicator {
                    from,
                    to: bounds,
                    emphasis,
                    workspace: active,
                    started: now,
                };
            }
            Some(indicator)
                if indicator.to != bounds
                    || reduced_motion
                    || now.saturating_duration_since(indicator.started) >= TAB_TRANSITION =>
            {
                *indicator = TabIndicator::settled(active, bounds, now);
            }
            None => {
                state.tab_indicator = Some(TabIndicator::settled(active, bounds, now));
            }
            _ => {}
        }
        let ((left, right), emphasis) = state
            .tab_indicator
            .as_ref()
            .map_or((bounds, tab_emphasis(active)), |indicator| {
                indicator.sample(now)
            });
        for (workspace, rect) in &tabs {
            let index = Workspace::ALL
                .iter()
                .position(|item| item == workspace)
                .unwrap_or(0);
            let amount = emphasis[index];
            let mut appearance = style(Role::Plain).fg(blend_color(
                foreground(Role::Plain),
                foreground(Role::Accent),
                amount,
            ));
            if amount >= 0.5 {
                appearance = appearance.add_modifier(Modifier::BOLD);
            }
            put(
                frame.buffer_mut(),
                rect.x + 1,
                rect.y,
                rect.width - 1,
                &format!("{} {}", index + 1, workspace.label()),
                appearance,
            );
        }
        for x in left.floor().max(f64::from(area.x)) as u16..(right.ceil() as u16).min(area.right())
        {
            let coverage = (right.min(f64::from(x + 1)) - left.max(f64::from(x))).clamp(0.0, 1.0);
            let appearance = style(Role::Accent).fg(blend_color(
                foreground(Role::Border),
                foreground(Role::Accent),
                coverage,
            ));
            put(frame.buffer_mut(), x, area.y + 2, 1, "━", appearance);
        }
    } else {
        state.tab_indicator = None;
    }
    tabs
}

fn blend_color(from: Color, to: Color, amount: f64) -> Color {
    match (from, to) {
        (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) => {
            let mix = |from: u8, to: u8| {
                (f64::from(from) + (f64::from(to) - f64::from(from)) * amount).round() as u8
            };
            Color::Rgb(mix(r0, r1), mix(g0, g1), mix(b0, b1))
        }
        _ if amount >= 0.5 => to,
        _ => from,
    }
}

pub(super) fn render_footer(frame: &mut Frame, area: Rect, data: &RenderData<'_>) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let y = area.bottom() - 1;
    let controls = data.mouse_capture == ui_terminal::MouseCapture::Enabled;
    let mode = if area.width >= 80 {
        if controls {
            "Shift+drag selects"
        } else {
            "Native text selection"
        }
    } else if controls {
        "Shift+drag"
    } else {
        "Text select"
    };
    let hint = if area.width >= 100 && data.operation.is_none() {
        format!(" {mode}  1-7 tabs  Tab panes ")
    } else {
        format!(" {mode} ")
    };
    let hint_width = (hint.width() as u16).min(area.width.saturating_sub(2));
    let metadata_x = area.x + 1 + hint_width;
    let available = area.width.saturating_sub(hint_width + 2);
    let repo_width = if data.repo.is_empty() {
        0
    } else {
        (safe_text(data.repo).width() as u16)
            .min(available / 2)
            .saturating_add(2)
            .min(available)
    };
    let badge_x = area.right().saturating_sub(repo_width + 1);
    let remote = match (data.ahead, data.behind) {
        (0, 0) => String::new(),
        (ahead, 0) => format!("  {ahead} ahead"),
        (0, behind) => format!("  {behind} behind"),
        (ahead, behind) => format!("  {ahead} ahead  {behind} behind"),
    };
    let branch = format!("{}{remote}", data.branch);
    let gap = u16::from(repo_width > 0 && !branch.is_empty());
    let branch_room = available.saturating_sub(repo_width + gap);
    let branch_room = if data.operation.is_some() {
        branch_room.min(area.width / 3)
    } else {
        branch_room
    };
    let branch = elide_middle(&branch, usize::from(branch_room));
    let branch_width = branch.width() as u16;
    let branch_x = badge_x.saturating_sub(branch_width + gap);
    if let Some(operation) = data.operation {
        let room = branch_x.saturating_sub(metadata_x + 2);
        render_operation(
            frame.buffer_mut(),
            Rect::new(metadata_x, y, room, 1),
            operation,
        );
    }
    if repo_width > 0 || branch_width > 0 {
        let start = branch_x.saturating_sub(1).max(area.x + 1);
        let width = area.right().saturating_sub(start + 1);
        put(
            frame.buffer_mut(),
            start,
            y,
            width,
            &" ".repeat(usize::from(width)),
            style(Role::Plain),
        );
    }
    put(
        frame.buffer_mut(),
        branch_x,
        y,
        branch_width,
        &branch,
        style(Role::Plain),
    );
    if repo_width > 0 {
        let repo = elide_middle(data.repo, usize::from(repo_width.saturating_sub(2)));
        put(
            frame.buffer_mut(),
            badge_x,
            y,
            repo_width,
            &format!(" {repo} "),
            repository_style(data.repo),
        );
    }
    put(
        frame.buffer_mut(),
        area.x + 1,
        y,
        hint_width,
        &hint,
        style(if controls { Role::Accent } else { Role::Plain }),
    );
}

pub(super) fn repository_style(name: &str) -> Style {
    // Fixed FNV-1a keeps a repository's identity stable across processes and
    // machines. The full name, not its clipped display label, selects the color.
    let hash = name.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    let hue = (hash & 0xffff) as f64 / 65536.0 * 6.0;
    let saturation = 0.55 + ((hash >> 16) & 255) as f64 / 255.0 * 0.25;
    let lightness = 0.32 + ((hash >> 24) & 255) as f64 / 255.0 * 0.26;
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let secondary = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let channels = match hue as u8 {
        0 => [chroma, secondary, 0.0],
        1 => [secondary, chroma, 0.0],
        2 => [0.0, chroma, secondary],
        3 => [0.0, secondary, chroma],
        4 => [secondary, 0.0, chroma],
        _ => [chroma, 0.0, secondary],
    }
    .map(|value| ((value + lightness - chroma / 2.0) * 255.0).round() as u8);
    let background =
        ui_theme::Color::Rgb(channels[0], channels[1], channels[2]).at_depth(color_depth());
    let luminance = |rgb: [u8; 3]| {
        let linear = rgb.map(|channel| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
    };
    let base = luminance(background.rgb().unwrap_or(channels));
    let contrast = |rgb| {
        let value = luminance(rgb);
        (value.max(base) + 0.05) / (value.min(base) + 0.05)
    };
    let tint = [40, 48, 56].map(|shift| ((hash >> shift) & 15) as u8);
    let quantized =
        |rgb: [u8; 3]| ui_theme::Color::Rgb(rgb[0], rgb[1], rgb[2]).at_depth(color_depth());
    let light = quantized(tint.map(|channel| 240 + channel));
    let dark = quantized(tint.map(|channel| 8 + channel));
    let mut foreground =
        if contrast(light.rgb().unwrap_or([255; 3])) > contrast(dark.rgb().unwrap_or([0; 3])) {
            light
        } else {
            dark
        };
    if contrast(foreground.rgb().unwrap_or([255; 3])) < 4.5 {
        foreground = ui_theme::Color::Ansi(if base > 0.179 { 0 } else { 15 });
    }
    Style::default()
        .bg(background.ratatui())
        .fg(foreground.ratatui())
        .add_modifier(Modifier::BOLD)
}
