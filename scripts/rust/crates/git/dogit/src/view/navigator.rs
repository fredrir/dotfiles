use super::*;

pub fn tree_prefixes(items: &[NavItem]) -> Vec<String> {
    let mut last = vec![true; items.len()];
    let mut following = Vec::new();
    for (index, item) in items.iter().enumerate().rev() {
        let depth = item.depth;
        following.resize(depth + 1, false);
        last[index] = !following[depth];
        following[depth] = true;
    }
    let mut branches = vec![false; items.iter().map(|item| item.depth).max().unwrap_or(0) + 1];
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let depth = item.depth;
            let mut prefix = String::new();
            for continuation in branches.iter().take(depth).skip(1) {
                prefix.push_str(if *continuation { "│ " } else { "  " });
            }
            if depth > 0 {
                prefix.push_str(if last[index] { "└─" } else { "├─" });
            }
            branches[depth] = !last[index];
            prefix
        })
        .collect()
}

pub fn nav_horizontal_limit(items: &[NavItem], width: u16, workspace: Workspace) -> usize {
    items
        .iter()
        .map(|item| {
            if let Some(file) = item.file_state {
                let (_, label_width) = tree_widths(item, width);
                return safe_text(&tree_label(item, file))
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
) {
    if area.is_empty() {
        return;
    }
    if items.is_empty() {
        return;
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
    for (row, item) in items.iter().enumerate().skip(state.offset).take(height) {
        let y = area.y + (row - state.offset) as u16;
        let selected = row == state.selected;
        let active = selected && state.focus == Focus::Navigator;
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
        if let Some(file) = item.file_state {
            render_tree_item(buffer, area, y, item, file, state.nav_horizontal);
            continue;
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
}

fn render_tree_item(
    buffer: &mut Buffer,
    area: Rect,
    y: u16,
    item: &NavItem,
    file: FileState,
    horizontal: usize,
) {
    let (guide_width, label_width) = tree_widths(item, area.width);
    let start = area.x + 2;
    let root = item.depth == 0 && item.kind == ItemKind::Folder;
    let guide = if root {
        if file.expanded {
            "▾ ".to_owned()
        } else {
            "▸ ".to_owned()
        }
    } else {
        window(
            &item.graph,
            item.graph.width().saturating_sub(usize::from(guide_width)),
            usize::from(guide_width),
        )
    };
    put(buffer, start, y, guide_width, &guide, style(Role::Border));
    let mut appearance = match (file.staged, file.unstaged) {
        (true, false) => file_side_style(DiffSide::Index),
        (false, true) => file_side_style(DiffSide::Worktree),
        _ => style(Role::Plain),
    };
    if file.conflicted {
        appearance = style(Role::DiffRemoved);
    }
    let label = tree_label(item, file);
    let horizontal = horizontal.min(
        safe_text(&label)
            .width()
            .saturating_sub(usize::from(label_width)),
    );
    let label = if horizontal > 0 {
        window(&safe_text(&label), horizontal, usize::from(label_width))
    } else if root {
        elide_root_path(&label, usize::from(label_width))
    } else {
        elide_middle(&label, usize::from(label_width))
    };
    put(
        buffer,
        start + guide_width,
        y,
        label_width,
        &label,
        appearance,
    );
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

fn tree_label(item: &NavItem, file: FileState) -> String {
    if item.kind == ItemKind::Folder && item.depth > 0 {
        format!("{}/{}", item.label, if file.expanded { "" } else { "…" })
    } else {
        item.label.clone()
    }
}

fn tree_widths(item: &NavItem, width: u16) -> (u16, u16) {
    let available = width.saturating_sub(3);
    let guide = if item.depth == 0 && item.kind == ItemKind::Folder {
        2
    } else {
        item.graph.width() as u16
    };
    let guide = guide.min(available.saturating_sub(12));
    (guide, available.saturating_sub(guide))
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
