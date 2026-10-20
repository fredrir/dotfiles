use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

#[derive(Default)]
struct Node {
    directory: bool,
    state: FileState,
    children: Vec<PathBuf>,
}

pub fn file_tree<'a>(
    repo: &Path,
    paths: impl IntoIterator<Item = (&'a Path, FileState)>,
    expanded: impl Fn(&Path) -> bool,
) -> (Vec<(PathBuf, NavItem)>, NavSummary) {
    let mut tree = BTreeMap::<PathBuf, Node>::new();
    let mut summary = NavSummary::default();
    let mut root: Option<PathBuf> = None;
    for (path, state) in paths {
        let parent = path.parent().unwrap_or(Path::new(""));
        if let Some(root) = &mut root {
            while !parent.starts_with(&root) {
                if !root.pop() {
                    break;
                }
            }
        } else {
            root = Some(parent.to_owned());
        }
        summary.items += 1;
        summary.staged += usize::from(state.staged);
        summary.unstaged += usize::from(state.unstaged);
        tree.entry(path.to_owned()).or_default().state = state;
        for parent in path.ancestors().skip(1) {
            let node = tree.entry(parent.to_owned()).or_default();
            node.directory = true;
            merge_state(&mut node.state, state);
        }
    }
    let root = root.unwrap_or_default();
    tree.entry(root.clone()).or_default().directory = true;
    let paths: Vec<_> = tree.keys().cloned().collect();
    for path in paths {
        if let Some(parent) = path.parent()
            && let Some(node) = tree.get_mut(parent)
        {
            node.children.push(path);
        }
    }
    let mut entries = Vec::new();
    let mut pending = vec![(root.clone(), root.clone(), 0)];
    while let Some((mut path, parent, depth)) = pending.pop() {
        while let [child] = tree[&path].children.as_slice()
            && tree[child].directory
        {
            path = child.clone();
        }
        let node = &tree[&path];
        let mut state = node.state;
        state.expanded = node.directory && expanded(&path);
        let label = if depth == 0 {
            if path.as_os_str().is_empty() {
                repo.file_name().unwrap_or(repo.as_os_str())
            } else {
                path.as_os_str()
            }
        } else {
            path.strip_prefix(&parent).unwrap_or(&path).as_os_str()
        };
        entries.push((
            path.clone(),
            NavItem {
                label: label.to_string_lossy().into_owned(),
                depth,
                kind: if node.directory {
                    ItemKind::Folder
                } else {
                    ItemKind::File
                },
                staged: state.staged,
                file_state: Some(state),
                ..Default::default()
            },
        ));
        if state.expanded {
            let mut children: Vec<_> = node.children.iter().collect();
            children.sort_by_key(|path| (!tree[*path].directory, *path));
            pending.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|child| (child.clone(), path.clone(), depth + 1)),
            );
        }
    }
    (entries, summary)
}

fn merge_state(target: &mut FileState, source: FileState) {
    target.staged |= source.staged;
    target.unstaged |= source.unstaged;
    target.conflicted |= source.conflicted;
}

pub(super) struct FileLabel<'a> {
    pub text: &'a str,
    pub directory: bool,
    pub expanded: bool,
    pub root: bool,
    pub horizontal: usize,
    pub animation: Option<FolderAnimation>,
    pub label_style: Style,
    pub icon_style: Style,
}

impl<'a> FileLabel<'a> {
    pub fn new(text: &'a str, directory: bool) -> Self {
        let appearance = style(Role::Muted);
        Self {
            text,
            directory,
            expanded: directory,
            root: directory,
            horizontal: 0,
            animation: None,
            label_style: if directory {
                appearance.add_modifier(Modifier::BOLD)
            } else {
                appearance
            },
            icon_style: appearance,
        }
    }

    pub fn render(self, buffer: &mut Buffer, area: Rect) {
        if area.is_empty() {
            return;
        }
        let decoration = Self::decoration_width(area.width);
        if decoration > 0 {
            let (icon, appearance) = if self.directory {
                folder::appearance(self.expanded, self.animation, self.icon_style)
            } else {
                (file_icons::for_path(self.text), self.icon_style)
            };
            put(buffer, area.x, area.y, decoration, icon, appearance);
        }
        let width = area.width.saturating_sub(decoration);
        let clean = display_path(self.text);
        let horizontal = self
            .horizontal
            .min(clean.width().saturating_sub(usize::from(width)));
        let label = if horizontal > 0 {
            window(&clean, horizontal, usize::from(width))
        } else if self.root && self.directory {
            elide_root_path(&clean, usize::from(width))
        } else {
            elide_middle(&clean, usize::from(width))
        };
        put(
            buffer,
            area.x + decoration,
            area.y,
            width,
            &label,
            self.label_style,
        );
    }

    pub fn decoration_width(width: u16) -> u16 {
        if width >= 3 { 2 } else { 0 }
    }
}

pub(super) fn display_path(path: &str) -> String {
    let mut display = String::with_capacity(path.len());
    for character in path.chars() {
        if character.is_control() {
            display.extend(character.escape_default());
        } else {
            display.push(character);
        }
    }
    display
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
