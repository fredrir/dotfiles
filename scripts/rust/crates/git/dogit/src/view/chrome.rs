use super::*;

#[derive(Clone)]
pub(super) struct Badge {
    pub label: String,
    appearance: Style,
}

impl Badge {
    pub fn new(label: String, appearance: Style) -> Self {
        Self { label, appearance }
    }

    pub fn width(&self) -> usize {
        self.label.width() + 2
    }

    fn count(count: usize, noun: &str, appearance: Style) -> Self {
        Self::new(format!("{count} {noun}"), appearance)
    }
}

pub(super) fn render_tabs(
    frame: &mut Frame,
    area: Rect,
    state: &ViewState,
) -> Vec<(Workspace, Rect)> {
    if area.height < 2 || area.width < 4 {
        return Vec::new();
    }
    let active = state.workspace;
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
        tabs.push((workspace, Rect::new(x, area.y, width - 1, 2)));
        x += width;
    }
    for (workspace, rect) in &tabs {
        let index = Workspace::ALL
            .iter()
            .position(|item| item == workspace)
            .unwrap_or(0);
        let selected = *workspace == active;
        put(
            frame.buffer_mut(),
            rect.x + 1,
            rect.y,
            rect.width - 1,
            &format!("{} {}", index + 1, workspace.label()),
            if selected {
                style(Role::Accent).add_modifier(Modifier::BOLD)
            } else {
                style(Role::Plain)
            },
        );
        if selected {
            put(
                frame.buffer_mut(),
                rect.x,
                rect.y + 1,
                rect.width - 1,
                &"━".repeat(usize::from(rect.width - 1)),
                style(Role::Accent),
            );
        }
    }
    tabs
}

pub(super) fn render_footer(
    frame: &mut Frame,
    area: Rect,
    data: &RenderData<'_>,
    badges: Vec<Badge>,
    detail: Option<&str>,
) -> Rect {
    if area.width < 4 || area.height == 0 {
        return Rect::default();
    }
    let y = area.bottom() - 1;
    let metadata_x = area.x;
    let available = area.width.saturating_sub(2);
    let repo_label = identity_label(data.repo);
    let repo_width = if repo_label.is_empty() {
        0
    } else {
        (repo_label.width() as u16)
            .min(available / 2)
            .saturating_add(2)
            .min(available)
    };
    let badge_x = area.right().saturating_sub(repo_width + 1);
    let branch = identity_label(data.branch);
    let gap = u16::from(repo_width > 0 && !branch.is_empty());
    let mut tracking_room =
        available.saturating_sub(repo_width + gap + u16::from(!branch.is_empty()) * 3);
    let mut tracking_width = 0;
    let tracking = [
        (data.behind, "", Role::DiffRemoved),
        (data.ahead, "", Role::DiffAdded),
    ]
    .into_iter()
    .filter(|(count, _, _)| *count > 0)
    .filter_map(|(count, arrow, role)| {
        let badge = Badge::new(format!("{count} {arrow}"), style(role));
        let width = badge.label.width() as u16 + 3;
        if width > tracking_room {
            return None;
        }
        tracking_room -= width;
        tracking_width += width;
        Some(badge)
    })
    .collect::<Vec<_>>();
    let branch_room = available.saturating_sub(repo_width + gap + tracking_width);
    let branch_room = if data.operation.is_some() || !badges.is_empty() {
        branch_room.min(area.width / 3)
    } else {
        branch_room
    };
    let branch = elide_middle(&branch, usize::from(branch_room.saturating_sub(2)));
    let branch_width = if branch.is_empty() {
        0
    } else {
        branch.width() as u16 + 2
    };
    let branch_x = badge_x.saturating_sub(branch_width + gap);
    let tracking_x = branch_x.saturating_sub(tracking_width);
    let counts = render_badges(
        frame.buffer_mut(),
        Rect::new(metadata_x, y, tracking_x.saturating_sub(metadata_x + 1), 1),
        badges,
        true,
    );
    let detail_x = counts.right() + u16::from(counts.width > 0);
    let detail_area = Rect::new(detail_x, y, tracking_x.saturating_sub(detail_x + 1), 1);
    if let Some(operation) = data.operation {
        render_operation(frame.buffer_mut(), detail_area, operation);
    } else {
        put(
            frame.buffer_mut(),
            detail_area.x,
            y,
            detail_area.width,
            &elide_end(detail.unwrap_or_default(), usize::from(detail_area.width)),
            style(Role::Plain),
        );
    }
    if repo_width > 0 || branch_width > 0 || tracking_width > 0 {
        let start = tracking_x.saturating_sub(1).max(area.x + 1);
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
    let mut x = tracking_x;
    for badge in tracking {
        let width = badge.label.width() as u16 + 2;
        render_badges(
            frame.buffer_mut(),
            Rect::new(x, y, width, 1),
            vec![badge],
            true,
        );
        x += width + 1;
    }
    put(
        frame.buffer_mut(),
        branch_x,
        y,
        branch_width,
        &format!(" {branch} "),
        identity_style(data.branch),
    );
    if repo_width > 0 {
        let repo = elide_middle(&repo_label, usize::from(repo_width.saturating_sub(2)));
        put(
            frame.buffer_mut(),
            badge_x,
            y,
            repo_width,
            &format!(" {repo} "),
            identity_style(data.repo),
        );
    }
    counts
}

pub(super) fn nav_badges(
    workspace: Workspace,
    summary: NavSummary,
    repo: &str,
    changes_filter: Option<DiffSide>,
) -> Vec<Badge> {
    let mut counts = Vec::new();
    if workspace == Workspace::Changes {
        if summary.items > 0
            && (summary.staged > 0 && summary.unstaged > 0
                || summary.items > summary.staged.max(summary.unstaged))
        {
            counts.push(Badge::count(
                summary.items,
                if summary.items == 1 { "File" } else { "Files" },
                files_style(repo),
            ));
        }
        if summary.staged > 0 || changes_filter == Some(DiffSide::Index) {
            counts.push(Badge::new(
                format!("{} Staged", summary.staged),
                file_side_style(DiffSide::Index),
            ));
        }
        if summary.unstaged > 0 || changes_filter == Some(DiffSide::Worktree) {
            counts.push(Badge::new(
                format!("{} Unstaged", summary.unstaged),
                file_side_style(DiffSide::Worktree),
            ));
        }
    } else if summary.items > 0 {
        let (singular, plural) = match workspace {
            Workspace::History if summary.has_commits => ("Commit", "Commits"),
            Workspace::Branches if summary.has_branches => ("Branch", "Branches"),
            Workspace::Stashes => ("Stash", "Stashes"),
            Workspace::Worktrees => ("Worktree", "Worktrees"),
            _ => ("File", "Files"),
        };
        let noun = if summary.items == 1 { singular } else { plural };
        let appearance = if singular == "File" {
            files_style(repo)
        } else {
            style(Role::Plain)
        };
        counts.push(Badge::count(summary.items, noun, appearance));
        if workspace == Workspace::Branches && summary.changed > 0 {
            counts.push(Badge::count(
                summary.changed,
                "Changed",
                file_side_style(DiffSide::Worktree),
            ));
        }
    }
    counts
}

fn files_style(repo: &str) -> Style {
    if repo.is_empty() {
        style(Role::Plain)
    } else {
        identity_color(repo)
    }
}

pub(super) fn render_pane_footer(
    buffer: &mut Buffer,
    area: Rect,
    badges: Vec<Badge>,
    detail: &str,
) -> Rect {
    let rendered = render_badges(buffer, area, badges, false);
    let detail_x = area.x + 1;
    let detail_width = rendered.x.saturating_sub(detail_x + 1);
    put(
        buffer,
        detail_x,
        area.y,
        detail_width,
        &elide_end(detail, usize::from(detail_width)),
        style(Role::Plain),
    );
    rendered
}

pub(super) fn render_badges(
    buffer: &mut Buffer,
    area: Rect,
    mut badges: Vec<Badge>,
    left: bool,
) -> Rect {
    if area.is_empty() {
        return area;
    }
    let natural_width = |badges: &[Badge]| {
        badges
            .iter()
            .map(|badge| badge.width() + 1)
            .sum::<usize>()
            .saturating_sub(1)
    };
    if badges.len() == 3 && natural_width(&badges) > usize::from(area.width) {
        badges.remove(0);
    }
    let budget = usize::from(area.width);
    let fits = natural_width(&badges) <= budget;
    let mut remaining = budget;
    let count = badges.len();
    let mut sized = Vec::new();
    for (index, mut badge) in badges.into_iter().enumerate() {
        let width = if fits {
            badge.width()
        } else {
            badge.width().min(remaining / (count - index))
        };
        if width >= 3 {
            badge.label = elide_end(&badge.label, width - 2);
            sized.push((badge, width as u16));
            remaining = remaining.saturating_sub(width + 1);
        }
    }
    let width = sized
        .iter()
        .map(|(_, width)| *width + 1)
        .sum::<u16>()
        .saturating_sub(1);
    let mut x = if left {
        area.x
    } else {
        area.right().saturating_sub(width)
    };
    let rendered = Rect::new(x, area.y, width, 1);
    for (badge, width) in sized {
        let appearance = badge_style(badge.appearance, color_depth()).add_modifier(Modifier::BOLD);
        put(
            buffer,
            x,
            area.y,
            width,
            &format!(" {} ", badge.label),
            appearance,
        );
        x += width + 1;
    }
    rendered
}

pub(super) fn identity_style(name: &str) -> Style {
    badge_style(identity_color(name), color_depth())
}

pub(super) fn identity_text_style(name: &str) -> Style {
    style(Role::Plain)
        .fg(identity_style(name).bg.unwrap_or(Color::Reset))
        .add_modifier(Modifier::BOLD)
}

fn identity_color(name: &str) -> Style {
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
    Style::default()
        .fg(Color::Rgb(channels[0], channels[1], channels[2]))
        .add_modifier(Modifier::BOLD)
}

fn identity_label(name: &str) -> String {
    let mut label = safe_text(name);
    if let Some((index, letter)) = label
        .char_indices()
        .find(|(_, letter)| letter.is_alphabetic())
    {
        label.replace_range(
            index..index + letter.len_utf8(),
            &letter.to_uppercase().to_string(),
        );
    }
    label
}

pub(super) fn badge_style(appearance: Style, depth: ColorDepth) -> Style {
    let source = match appearance.fg.unwrap_or(Color::Gray) {
        Color::Rgb(red, green, blue) => [red, green, blue],
        color => {
            let index = match color {
                Color::Black => 0,
                Color::Red => 1,
                Color::Green => 2,
                Color::Yellow => 3,
                Color::Blue => 4,
                Color::Magenta => 5,
                Color::Cyan => 6,
                Color::Gray => 7,
                Color::DarkGray => 8,
                Color::LightRed => 9,
                Color::LightGreen => 10,
                Color::LightYellow => 11,
                Color::LightBlue => 12,
                Color::LightMagenta => 13,
                Color::LightCyan => 14,
                Color::White => 15,
                Color::Indexed(index) => index,
                _ => 7,
            };
            ui_theme::Color::Ansi(index).rgb().unwrap_or([192; 3])
        }
    };
    let quantized = |channels: [u8; 3]| {
        let color = ui_theme::Color::Rgb(channels[0], channels[1], channels[2]);
        let Some(hue) = badge_hue(channels).filter(|_| depth != ColorDepth::TrueColor) else {
            return color.at_depth(depth);
        };
        let range = if depth == ColorDepth::Ansi16 {
            0..16
        } else {
            16..256
        };
        range
            .filter_map(|index| {
                let candidate = ui_theme::Color::Ansi(index as u8);
                let rgb = candidate.rgb()?;
                let distance = (badge_hue(rgb)? - hue).abs();
                if distance.min(6.0 - distance) > 0.5 {
                    return None;
                }
                let error = rgb
                    .into_iter()
                    .zip(channels)
                    .map(|(left, right)| (i32::from(left) - i32::from(right)).pow(2))
                    .sum::<i32>();
                Some((candidate, error))
            })
            .min_by_key(|(_, error)| *error)
            .map_or_else(|| color.at_depth(depth), |(candidate, _)| candidate)
    };
    let tint = |amount: f64| {
        quantized(source.map(|channel| {
            (f64::from(channel) + (255.0 - f64::from(channel)) * amount).round() as u8
        }))
    };
    let shade =
        |amount: f64| quantized(source.map(|channel| (f64::from(channel) * amount).round() as u8));
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
    let contrast = |background: ui_theme::Color, foreground: ui_theme::Color| {
        (luminance(background.rgb().unwrap_or([255; 3])) + 0.05)
            / (luminance(foreground.rgb().unwrap_or([0; 3])) + 0.05)
    };
    let mut background = tint(0.65);
    let mut foreground = shade(0.35);
    for amount in [0.75, 0.85, 0.95, 1.0] {
        if contrast(background, foreground) >= 4.5 {
            break;
        }
        background = tint(amount);
    }
    for amount in [0.30, 0.25, 0.20, 0.15, 0.10, 0.0] {
        if contrast(background, foreground) >= 4.5 {
            break;
        }
        foreground = shade(amount);
    }
    Style::default()
        .bg(background.ratatui())
        .fg(foreground.ratatui())
        .add_modifier(appearance.add_modifier)
}

fn badge_hue([red, green, blue]: [u8; 3]) -> Option<f64> {
    let maximum = red.max(green).max(blue);
    let chroma = maximum - red.min(green).min(blue);
    if chroma == 0 || u16::from(chroma) * 10 < u16::from(maximum) {
        return None;
    }
    let [r, g, b] = [red, green, blue].map(f64::from);
    let chroma = f64::from(chroma);
    Some(if maximum == red {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if maximum == green {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    })
}
