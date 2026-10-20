use super::*;

pub fn nav_horizontal_limit(items: &[NavItem], width: u16, workspace: Workspace) -> usize {
    items
        .iter()
        .zip(tree_decorations(items))
        .map(|(item, decoration)| {
            if item.file_state.is_some() {
                let status = if workspace == Workspace::Changes {
                    decoration.status
                } else {
                    TreeStatus::None
                };
                let layout = TreeLayout::new(item, status, width);
                let label_width = layout.label_width;
                return files::display_path(&item.label)
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
            let mut label_width = width.saturating_sub(
                status_width + staged_width + graph_width + u16::from(status_width > 0),
            );
            if matches!(
                item.kind,
                ItemKind::File | ItemKind::Folder | ItemKind::Worktree
            ) {
                label_width =
                    label_width.saturating_sub(files::FileLabel::decoration_width(label_width));
                return files::display_path(&item.label)
                    .width()
                    .saturating_sub(usize::from(label_width));
            }
            safe_text(&item.label)
                .width()
                .saturating_sub(usize::from(label_width))
        })
        .max()
        .unwrap_or(0)
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
    for (row, (item, decoration)) in items
        .iter()
        .zip(tree_decorations(items))
        .enumerate()
        .skip(state.offset)
        .take(height)
    {
        let y = area.y + (row - state.offset) as u16;
        let selected = !item.disabled && row == state.selected;
        let active = selected && state.focus == Focus::Navigator;
        if let Some(file) = item.file_state {
            let row_style = if item.disabled {
                style(Role::Muted).add_modifier(Modifier::DIM)
            } else {
                tree_row_style(selected, active, state.nav_hovered == Some(row))
            };
            let disclosure = render_tree_item(
                buffer,
                Rect::new(area.x, y, area.width, 1),
                item,
                TreePaint {
                    file,
                    decoration,
                    horizontal: state.nav_horizontal,
                    appearance: row_style,
                    neutral: state.workspace != Workspace::Changes,
                    animation: state
                        .folder_animation
                        .filter(|animation| !item.disabled && animation.row == row),
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
        } else if matches!(
            item.kind,
            ItemKind::File | ItemKind::Folder | ItemKind::Worktree
        ) {
            let mut label = files::FileLabel::new(&item.label, item.kind != ItemKind::File);
            label.horizontal = state.nav_horizontal;
            if current {
                label.label_style = appearance.add_modifier(Modifier::BOLD);
            }
            label.render(buffer, Rect::new(x + graph_width, y, label_width, 1));
        } else {
            let label = &item.label;
            let label = if state.nav_horizontal > 0 {
                window(
                    &safe_text(label),
                    state.nav_horizontal,
                    usize::from(label_width),
                )
            } else if item.kind == ItemKind::Stash {
                elide_end(label, usize::from(label_width))
            } else {
                elide_middle(label, usize::from(label_width))
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

pub(super) fn render_file_tree(buffer: &mut Buffer, area: Rect, offset: usize, items: &[NavItem]) {
    if area.is_empty() {
        return;
    }
    for (row, item) in items
        .iter()
        .enumerate()
        .skip(offset)
        .take(usize::from(area.height))
    {
        render_tree_item(
            buffer,
            Rect::new(area.x, area.y + (row - offset) as u16, area.width, 1),
            item,
            TreePaint {
                file: item.file_state.unwrap_or_default(),
                decoration: TreeDecoration::default(),
                horizontal: 0,
                appearance: style(Role::Muted),
                neutral: true,
                animation: None,
            },
        );
    }
    render_directory_guides(buffer, area, offset, items);
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
    Deleted,
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

#[derive(Clone, Copy, Default)]
struct TreeDecoration {
    status: TreeStatus,
    highlight_directory: bool,
}

fn tree_decorations(items: &[NavItem]) -> impl Iterator<Item = TreeDecoration> + '_ {
    let mut parents: Vec<(usize, FileState, bool)> = Vec::new();
    items.iter().map(move |item| {
        while parents
            .last()
            .is_some_and(|(depth, _, _)| *depth >= item.depth)
        {
            parents.pop();
        }
        let Some(file) = item.file_state else {
            return TreeDecoration::default();
        };
        let directory = item.kind == ItemKind::Folder;
        let show_file_status = parents
            .last()
            .is_none_or(|(_, parent, _)| parent.staged && parent.unstaged);
        let ancestor_highlighted = parents
            .last()
            .is_some_and(|(_, _, highlighted)| *highlighted);
        let highlight_directory = directory
            && !ancestor_highlighted
            && !file.deleted
            && !file.conflicted
            && file.staged != file.unstaged;
        if directory {
            parents.push((
                item.depth,
                file,
                ancestor_highlighted || highlight_directory,
            ));
        }
        let status = if file.conflicted {
            TreeStatus::Conflict
        } else if file.deleted {
            TreeStatus::Deleted
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
        };
        TreeDecoration {
            status,
            highlight_directory,
        }
    })
}

struct TreePaint {
    file: FileState,
    decoration: TreeDecoration,
    horizontal: usize,
    appearance: Style,
    neutral: bool,
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
        decoration,
        horizontal,
        appearance,
        neutral,
        animation,
    } = paint;
    let decoration = if neutral {
        TreeDecoration::default()
    } else {
        decoration
    };
    let status = decoration.status;
    buffer.set_style(area, appearance);
    let layout = TreeLayout::new(item, status, area.width);
    let x = area.x + layout.indent;
    let directory = item.kind == ItemKind::Folder;
    let label_style = if neutral {
        appearance.fg(foreground(Role::Muted))
    } else if file.staged_deletion || (directory && file.conflicted) {
        appearance.fg(foreground(Role::DiffRemoved))
    } else if decoration.highlight_directory {
        let side = if file.staged {
            DiffSide::Index
        } else {
            DiffSide::Worktree
        };
        appearance.fg(file_side_style(side).fg.unwrap_or(Color::Reset))
    } else if directory {
        appearance.fg(foreground(Role::Muted))
    } else {
        appearance
    };
    let label_style = if directory && !item.disabled {
        label_style.add_modifier(Modifier::BOLD)
    } else {
        label_style
    };
    let icon_style = if directory {
        label_style.remove_modifier(Modifier::BOLD)
    } else if file.staged_deletion && !neutral {
        label_style
    } else {
        appearance.fg(foreground(Role::Muted))
    };
    files::FileLabel {
        text: &item.label,
        directory,
        expanded: file.expanded,
        root: item.depth == 0,
        horizontal,
        animation,
        label_style,
        icon_style,
    }
    .render(
        buffer,
        Rect::new(x, area.y, layout.decoration + layout.label_width, 1),
    );
    let disclosure = (directory && !item.disabled && layout.decoration > 0).then_some(Rect::new(
        x,
        area.y,
        layout.decoration,
        1,
    ));
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
            TreeStatus::Deleted => ("D", style(Role::DiffRemoved).fg),
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
        let decoration = files::FileLabel::decoration_width(content);
        let indent = Self::indent(item.depth, width);
        Self {
            indent,
            decoration,
            label_width: content.saturating_sub(indent - 1 + decoration),
            status_width,
        }
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
