use super::*;

pub fn nav_horizontal_limit(items: &[NavItem], width: u16, workspace: Workspace) -> usize {
    items
        .iter()
        .zip(tree_statuses(items))
        .map(|(item, status)| {
            if item.file_state.is_some() {
                let layout = TreeLayout::new(item, status, width);
                let label_width = layout.label_width;
                return safe_text(&item.label)
                    .width()
                    .saturating_sub(usize::from(label_width));
            }
            let width = width.saturating_sub((item.depth.min(8) * 2) as u16 + 3);
            let current = item.kind == ItemKind::Worktree && item.status == "current";
            let status_width = if current || matches!(item.status.as_str(), "." | " ") {
                0
            } else {
                item.status.width().min(8) as u16
            };
            let staged_width = if item.staged && width > 22 {
                if item.detail == "mixed" { 6 } else { 7 }
            } else {
                0
            };
            let graph_width = item.graph.width().min(usize::from(width / 3)) as u16;
            let label_width = width.saturating_sub(
                status_width + staged_width + graph_width + u16::from(status_width > 0),
            );
            safe_text(&label_text(item, workspace))
                .width()
                .saturating_sub(usize::from(label_width))
        })
        .max()
        .unwrap_or(0)
}

fn label_text(item: &NavItem, workspace: Workspace) -> String {
    if item.kind == ItemKind::Folder && workspace == Workspace::Changes {
        format!(
            "{}/{}",
            item.label.trim_end_matches('/'),
            if item.detail == "closed" { "…" } else { "" }
        )
    } else {
        item.label.clone()
    }
}

pub(super) fn render_navigator(
    buffer: &mut Buffer,
    area: Rect,
    state: &mut ViewState,
    items: &[NavItem],
) -> Vec<(usize, Rect)> {
    let mut disclosures = Vec::new();
    if area.is_empty() || items.is_empty() {
        return disclosures;
    }
    state.nav_horizontal =
        state
            .nav_horizontal
            .min(nav_horizontal_limit(items, area.width, state.workspace));
    state.selected = state.selected.min(items.len() - 1);
    let height = usize::from(area.height);
    if !state.nav_manual_scroll {
        state.offset = state.offset.min(state.selected);
        if state.selected >= state.offset.saturating_add(height) {
            state.offset = state.selected + 1 - height;
        }
    }
    state.offset = state.offset.min(items.len().saturating_sub(height));
    for (row, (item, status)) in items
        .iter()
        .zip(tree_statuses(items))
        .enumerate()
        .skip(state.offset)
        .take(height)
    {
        let y = area.y + (row - state.offset) as u16;
        let selected = row == state.selected;
        let active = selected && state.focus == Focus::Navigator;
        if let Some(file) = item.file_state {
            let row_style = tree_row_style(selected, active, state.nav_hovered == Some(row));
            let disclosure = render_tree_item(
                buffer,
                Rect::new(area.x, y, area.width, 1),
                item,
                TreePaint {
                    file,
                    status,
                    horizontal: state.nav_horizontal,
                    appearance: row_style,
                    animation: state
                        .folder_animation
                        .filter(|animation| animation.row == row),
                },
            );
            if let Some(disclosure) = disclosure {
                disclosures.push((row, disclosure));
            }
            continue;
        }
        if selected {
            put(
                buffer,
                area.x,
                y,
                1,
                "┃",
                if active {
                    style(Role::Accent)
                } else {
                    style(Role::Border)
                },
            );
        }
        let indent = (item.depth.min(8) * 2) as u16;
        let x = area.x + 2 + indent;
        let width = area.width.saturating_sub(indent + 3);
        let current = item.kind == ItemKind::Worktree && item.status == "current";
        let appearance = if current {
            style(Role::Accent)
        } else {
            style(Role::Plain)
        };
        let status = if current || matches!(item.status.as_str(), "." | " ") {
            ""
        } else {
            &item.status
        };
        let status_width = status.width().min(8) as u16;
        let stage_label = if item.detail == "mixed" {
            "mixed"
        } else {
            "staged"
        };
        let staged_width = if item.staged && width > 22 {
            stage_label.len() as u16 + 1
        } else {
            0
        };
        let graph_width = item.graph.width().min(usize::from(width / 3)) as u16;
        if !item.graph.is_empty() {
            put(
                buffer,
                x,
                y,
                graph_width,
                &item.graph,
                if active {
                    style(Role::Accent)
                } else {
                    style(Role::Plain)
                },
            );
        }
        let label_width = width.saturating_sub(
            status_width + staged_width + graph_width + u16::from(status_width > 0),
        );
        if item.kind == ItemKind::Commit {
            subject::render_list(
                buffer,
                Rect::new(x + graph_width, y, label_width, 1),
                &item.label,
                state.nav_horizontal,
            );
        } else {
            let label = label_text(item, state.workspace);
            let label = if state.nav_horizontal > 0 {
                window(
                    &safe_text(&label),
                    state.nav_horizontal,
                    usize::from(label_width),
                )
            } else if item.kind == ItemKind::Stash {
                elide_end(&label, usize::from(label_width))
            } else {
                elide_middle(&label, usize::from(label_width))
            };
            put(buffer, x + graph_width, y, label_width, &label, appearance);
        }
        if staged_width > 0 {
            put(
                buffer,
                area.right() - status_width - staged_width - 1,
                y,
                staged_width,
                stage_label,
                style(Role::Plain),
            );
        }
        if status_width > 0 {
            let status_style = match status {
                "+" | "A" | "new" => style(Role::DiffAdded),
                "-" | "D" | "deleted" => style(Role::DiffRemoved),
                "!" | "U" | "conflict" => style(Role::Warning),
                _ => style(Role::Plain),
            };
            put(
                buffer,
                area.right() - status_width - 1,
                y,
                status_width,
                status,
                status_style,
            );
        }
    }
    render_directory_guides(buffer, area, state.offset, items);
    if items.len() > height && area.height > 2 {
        let thumb_size = (height * height / items.len()).max(1);
        let thumb_top =
            state.offset * (height - thumb_size) / items.len().saturating_sub(height).max(1);
        for row in thumb_top..thumb_top + thumb_size {
            put(
                buffer,
                area.right() - 1,
                area.y + row as u16,
                1,
                "│",
                style(Role::Accent),
            );
        }
    }
    disclosures
}

fn render_directory_guides(buffer: &mut Buffer, area: Rect, offset: usize, items: &[NavItem]) {
    struct Siblings {
        first: usize,
        last: usize,
        has_nested_directories: bool,
    }

    let mut parents: Vec<usize> = Vec::new();
    let mut siblings: Vec<_> = (0..items.len())
        .map(|row| Siblings {
            first: row,
            last: row,
            has_nested_directories: false,
        })
        .collect();
    for (row, item) in items.iter().enumerate() {
        while parents
            .last()
            .is_some_and(|parent| items[*parent].depth >= item.depth)
        {
            parents.pop();
        }
        if item.kind == ItemKind::Folder && item.file_state.is_some() {
            if let Some(parent) = parents.last() {
                let group = &mut siblings[*parent];
                if group.first == *parent {
                    group.first = row;
                }
                group.last = row;
            }
            if let Some(grandparent) = parents.iter().rev().nth(1) {
                siblings[*grandparent].has_nested_directories = true;
            }
            parents.push(row);
        }
    }
    for (parent, group) in siblings.into_iter().enumerate() {
        if group.first == group.last || !group.has_nested_directories {
            continue;
        }
        let start = (group.first + 1).max(offset);
        let end = group.last.min(offset + usize::from(area.height));
        if start >= end {
            continue;
        }
        let parent_indent = TreeLayout::indent(items[parent].depth, area.width);
        let child_indent = TreeLayout::indent(items[group.first].depth, area.width);
        if child_indent <= parent_indent {
            continue;
        }
        let indent = child_indent;
        for (row, item) in items.iter().enumerate().take(end).skip(start) {
            // Narrow panes can compress several levels into one column.
            if indent >= TreeLayout::indent(item.depth, area.width) {
                continue;
            }
            let x = area.x + indent;
            let y = area.y + (row - offset) as u16;
            // Inset each end equally from the neighboring folder's icon row.
            let below_icon = TreeLayout::indent(items[row - 1].depth, area.width) <= indent;
            let above_icon = TreeLayout::indent(items[row + 1].depth, area.width) <= indent;
            let symbol = match (below_icon, above_icon) {
                (true, false) => "╷",
                (false, true) => "╵",
                _ => "│",
            };
            put(
                buffer,
                x,
                y,
                1,
                symbol,
                directory_guide_style(buffer[(x, y)].bg),
            );
        }
    }
}

fn directory_guide_style(background: Color) -> Style {
    let appearance = style(Role::Border).bg(background);
    if color_depth() == ColorDepth::Ansi16 {
        return appearance;
    }
    let backdrop = match background {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        Color::Indexed(index) => ui_theme::Color::Ansi(index).rgb(),
        _ => Palette::current().background(Role::Background).rgb(),
    }
    .unwrap_or([0; 3]);
    let border = palette().color(Role::Border).rgb().unwrap_or([128; 3]);
    let channels: [u8; 3] = std::array::from_fn(|index| {
        (f64::from(backdrop[index]) * 0.55 + f64::from(border[index]) * 0.45).round() as u8
    });
    appearance.fg(ui_theme::Color::Rgb(channels[0], channels[1], channels[2])
        .at_depth(color_depth())
        .ratatui())
}

fn tree_row_style(selected: bool, active: bool, hovered: bool) -> Style {
    let role = if active {
        Role::Selection
    } else if selected || hovered {
        Role::Surface
    } else {
        return style(Role::Plain);
    };
    let background = if color_depth() == ColorDepth::Ansi16 {
        if active { Color::Blue } else { Color::DarkGray }
    } else {
        palette().background(role).ratatui()
    };
    style(Role::Plain).bg(background)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TreeStatus {
    #[default]
    None,
    Staged,
    Unstaged,
    Mixed,
    Conflict,
}

impl TreeStatus {
    fn width(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Mixed => 3,
            _ => 1,
        }
    }
}

fn tree_statuses(items: &[NavItem]) -> impl Iterator<Item = TreeStatus> + '_ {
    let mut parents: Vec<(usize, FileState)> = Vec::new();
    items.iter().map(move |item| {
        while parents
            .last()
            .is_some_and(|(depth, _)| *depth >= item.depth)
        {
            parents.pop();
        }
        let Some(file) = item.file_state else {
            return TreeStatus::None;
        };
        let directory = item.kind == ItemKind::Folder;
        let show_file_status = parents
            .last()
            .is_none_or(|(_, parent)| parent.staged && parent.unstaged);
        if directory {
            parents.push((item.depth, file));
        }
        if file.conflicted {
            TreeStatus::Conflict
        } else if directory {
            if !file.expanded && file.staged && file.unstaged {
                TreeStatus::Mixed
            } else {
                TreeStatus::None
            }
        } else if show_file_status {
            match (file.staged, file.unstaged) {
                (true, true) => TreeStatus::Mixed,
                (true, false) => TreeStatus::Staged,
                (false, true) => TreeStatus::Unstaged,
                _ => TreeStatus::None,
            }
        } else {
            TreeStatus::None
        }
    })
}

struct TreePaint {
    file: FileState,
    status: TreeStatus,
    horizontal: usize,
    appearance: Style,
    animation: Option<FolderAnimation>,
}

fn render_tree_item(
    buffer: &mut Buffer,
    area: Rect,
    item: &NavItem,
    paint: TreePaint,
) -> Option<Rect> {
    let TreePaint {
        file,
        status,
        horizontal,
        appearance,
        animation,
    } = paint;
    buffer.set_style(area, appearance);
    let layout = TreeLayout::new(item, status, area.width);
    let x = area.x + layout.indent;
    let directory = item.kind == ItemKind::Folder;
    let label_style = if directory {
        let directory_style = if file.conflicted {
            style(Role::DiffRemoved)
        } else {
            match (file.staged, file.unstaged) {
                (true, false) => file_side_style(DiffSide::Index),
                (false, true) => file_side_style(DiffSide::Worktree),
                _ => style(Role::Plain),
            }
        };
        appearance
            .fg(directory_style.fg.unwrap_or(Color::Reset))
            .add_modifier(Modifier::BOLD)
    } else {
        appearance
    };
    if layout.decoration > 0 {
        let (icon, icon_style) = if directory {
            folder::appearance(
                file.expanded,
                animation,
                label_style.remove_modifier(Modifier::BOLD),
            )
        } else {
            (
                file_icons::for_path(&item.label),
                appearance.fg(foreground(Role::Muted)),
            )
        };
        put(buffer, x, area.y, layout.decoration, icon, icon_style);
    }
    let disclosure =
        (directory && layout.decoration > 0).then_some(Rect::new(x, area.y, layout.decoration, 1));
    let horizontal = horizontal.min(
        safe_text(&item.label)
            .width()
            .saturating_sub(usize::from(layout.label_width)),
    );
    let label = if horizontal > 0 {
        window(
            &safe_text(&item.label),
            horizontal,
            usize::from(layout.label_width),
        )
    } else if item.depth == 0 && directory {
        elide_root_path(&item.label, usize::from(layout.label_width))
    } else {
        elide_middle(&item.label, usize::from(layout.label_width))
    };
    put(
        buffer,
        x + layout.decoration,
        area.y,
        layout.label_width,
        &label,
        label_style,
    );
    if layout.status_width > 0 {
        let right = area.right() - 2;
        if status == TreeStatus::Mixed {
            put(
                buffer,
                right - 2,
                area.y,
                1,
                "●",
                appearance.fg(file_side_style(DiffSide::Index).fg.unwrap_or(Color::Reset)),
            );
        }
        let (marker, foreground) = match status {
            TreeStatus::Staged => ("●", file_side_style(DiffSide::Index).fg),
            TreeStatus::Mixed | TreeStatus::Unstaged => {
                ("○", file_side_style(DiffSide::Worktree).fg)
            }
            TreeStatus::Conflict => ("!", style(Role::DiffRemoved).fg),
            TreeStatus::None => ("", None),
        };
        put(
            buffer,
            right,
            area.y,
            1,
            marker,
            appearance.fg(foreground.unwrap_or(Color::Reset)),
        );
    }
    disclosure
}

struct TreeLayout {
    indent: u16,
    decoration: u16,
    label_width: u16,
    status_width: u16,
}

impl TreeLayout {
    fn indent(depth: usize, width: u16) -> u16 {
        let step = if width >= 28 { 2 } else { 1 };
        // Keep equal-depth icons aligned across different icon and status widths.
        1 + ((depth.min(8) * step) as u16).min(width.saturating_sub(16))
    }

    fn new(item: &NavItem, status: TreeStatus, width: u16) -> Self {
        let available = width.saturating_sub(2);
        let status_width = if available >= 12 { status.width() } else { 0 };
        let content = available.saturating_sub(status_width + u16::from(status_width > 0));
        let decoration = if content >= 3 { 2 } else { 0 };
        let indent = Self::indent(item.depth, width);
        Self {
            indent,
            decoration,
            label_width: content.saturating_sub(indent - 1 + decoration),
            status_width,
        }
    }
}

fn elide_root_path(path: &str, width: usize) -> String {
    let clean = safe_text(path);
    if clean.width() <= width {
        return clean;
    }
    if width == 0 {
        return String::new();
    }
    let parts: Vec<_> = clean.split('/').collect();
    let keep_first =
        parts.len() > 2 && parts[0].width() + parts[parts.len() - 1].width() + 3 <= width;
    let prefix = if keep_first {
        format!("{}/…", parts[0])
    } else {
        "…".into()
    };
    let mut used = prefix.width();
    let mut start = parts.len();
    while start > if keep_first { 2 } else { 1 } {
        let next_width = parts[start - 1].width() + 1;
        if used + next_width > width {
            break;
        }
        used += next_width;
        start -= 1;
    }
    if start < parts.len() {
        format!("{prefix}/{}", parts[start..].join("/"))
    } else {
        prefix
    }
}

pub fn graph_rows(commits: &[crate::git::Commit]) -> Vec<String> {
    let mut lanes = Vec::<Option<String>>::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let lane = lanes
            .iter()
            .position(|oid| oid.as_ref() == Some(&commit.oid))
            .unwrap_or_else(|| {
                let lane = lanes
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(lanes.len());
                if lane == lanes.len() {
                    lanes.push(None);
                }
                lanes[lane] = Some(commit.oid.clone());
                lane
            });
        let occupied = lanes.iter().map(Option::is_some).collect::<Vec<_>>();
        lanes[lane] = None;
        let mut targets = Vec::new();
        for parent in &commit.parents {
            let target = lanes
                .iter()
                .position(|oid| oid.as_ref() == Some(parent))
                .unwrap_or_else(|| {
                    let target = if lanes[lane].is_none() {
                        lane
                    } else {
                        lanes
                            .iter()
                            .enumerate()
                            .skip(lane + 1)
                            .find_map(|(index, value)| value.is_none().then_some(index))
                            .unwrap_or(lanes.len())
                    };
                    if target == lanes.len() {
                        lanes.push(None);
                    }
                    lanes[target] = Some(parent.clone());
                    target
                });
            targets.push(target);
        }
        let width = occupied.len().max(lanes.len()).min(6);
        let mut row = vec![' '; width * 2];
        for (position, present) in occupied.iter().take(width).enumerate() {
            if *present {
                row[position * 2] = '│';
            }
        }
        for target in targets.into_iter().filter(|target| *target != lane) {
            if lane >= width || target >= width {
                continue;
            }
            for (column, cell) in row
                .iter_mut()
                .enumerate()
                .take(lane.max(target) * 2)
                .skip(lane.min(target) * 2 + 1)
            {
                *cell = if column % 2 == 0 && *cell == '│' {
                    '┼'
                } else {
                    '─'
                };
            }
            row[target * 2] = match (target < lane, occupied.get(target) == Some(&true)) {
                (true, true) => '├',
                (false, true) => '┤',
                (true, false) => '┌',
                (false, false) => '┐',
            };
        }
        if lane < width {
            row[lane * 2] = '●';
        }
        let mut row = row.into_iter().collect::<String>();
        if occupied.len().max(lanes.len()) > 6 {
            row.push_str("… ");
        }
        rows.push(row);
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
    }
    rows
}

#[cfg(test)]
#[path = "../../tests/unit/navigator.rs"]
mod tests;
