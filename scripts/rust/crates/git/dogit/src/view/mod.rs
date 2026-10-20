use std::collections::BTreeSet;
use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};
use ui_theme::{ColorDepth, Palette, Role};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::diff::{DiffSide, ExactDiff, LineKind};

mod chrome;
mod commit;
mod diff;
mod file_icons;
mod files;
mod folder;
mod navigator;
mod overlay;
mod subject;
mod text;
mod toast;
mod wrap;

pub use diff::{
    configured_diff_horizontal_limit, diff_horizontal_limit, diff_rows, text_horizontal_limit,
};
pub use files::file_tree;
pub use folder::FolderAnimation;
pub use navigator::{graph_rows, nav_horizontal_limit};
pub use overlay::confirmation_scroll_max;
pub use overlay::inspection_scroll_max;
pub use toast::ToastView;
pub use wrap::{DiffLayout, DiffPaint, configured_diff_layout};

use chrome::{Badge, nav_badges, render_footer, render_pane_footer, render_tabs};
use diff::{render_diff, render_no_changes, render_structural, render_text};
use navigator::render_navigator;
use overlay::render_overlay;
use text::{
    ansi_line, elide_end, elide_middle, safe_text, safe_text_multiline, syntax_line, window,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Workspace {
    #[default]
    Changes,
    History,
    Branches,
    Stashes,
    Worktrees,
    Compare,
}

impl Workspace {
    pub const ALL: [Self; 6] = [
        Self::Changes,
        Self::History,
        Self::Branches,
        Self::Stashes,
        Self::Worktrees,
        Self::Compare,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::History => "History",
            Self::Branches => "Branches",
            Self::Stashes => "Stashes",
            Self::Worktrees => "Worktrees",
            Self::Compare => "Compare",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Focus {
    #[default]
    Navigator,
    CommitDetails,
    Canvas,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ItemKind {
    #[default]
    File,
    Folder,
    Commit,
    Branch,
    Stash,
    Worktree,
}

#[derive(Clone, Debug, Default)]
pub struct NavItem {
    pub label: String,
    pub detail: String,
    pub status: String,
    pub depth: usize,
    pub kind: ItemKind,
    pub disabled: bool,
    pub staged: bool,
    pub graph: String,
    pub file_state: Option<FileState>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileState {
    pub staged: bool,
    pub unstaged: bool,
    pub conflicted: bool,
    pub deleted: bool,
    pub staged_deletion: bool,
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct DiffScope {
    pub side: DiffSide,
    pub staged: bool,
    pub unstaged: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tone {
    #[default]
    Normal,
    Attention,
}

#[derive(Clone, Debug, Default)]
pub struct OperationView {
    pub label: String,
    pub detail: String,
    pub tone: Tone,
}

#[derive(Clone, Debug)]
pub struct EditorView {
    pub value: String,
    pub cursor: usize,
    pub selection: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Debug)]
pub enum ConfirmationDetail {
    Text(String),
    Files(Vec<NavItem>),
}

impl From<String> for ConfirmationDetail {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for ConfirmationDetail {
    fn from(text: &str) -> Self {
        Self::Text(text.into())
    }
}

impl From<&ui_widgets::EditorBuffer> for EditorView {
    fn from(editor: &ui_widgets::EditorBuffer) -> Self {
        Self {
            value: editor.as_str().into(),
            cursor: editor.cursor(),
            selection: editor.selection(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Overlay {
    Error {
        title: String,
        editor: EditorView,
    },
    Inspect {
        message: String,
        scroll: u16,
    },
    Compose {
        editor: EditorView,
    },
    Input {
        title: String,
        editor: EditorView,
    },
    Commands {
        editor: EditorView,
        items: Vec<NavItem>,
        selected: usize,
    },
    Confirm {
        title: String,
        detail: ConfirmationDetail,
        destructive: bool,
        scroll: u16,
    },
}

#[derive(Clone, Debug)]
pub struct ViewState {
    pub workspace: Workspace,
    pub focus: Focus,
    pub commit_field: usize,
    pub selected: usize,
    pub offset: usize,
    pub diff_offset: usize,
    pub diff_cursor: usize,
    pub horizontal: usize,
    pub nav_horizontal: usize,
    pub nav_manual_scroll: bool,
    pub nav_hovered: Option<usize>,
    pub folder_animation: Option<FolderAnimation>,
    pub canvas_manual_scroll: bool,
    pub commit_offset: usize,
    pub commit_manual_scroll: bool,
    pub split: bool,
    pub nav_width: u16,
    pub overlay: Option<Overlay>,
    pub editor_offset: usize,
    pub divider_hovered: bool,
    pub divider_hover_row: Option<u16>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            workspace: Workspace::Changes,
            focus: Focus::Navigator,
            commit_field: 0,
            selected: 0,
            offset: 0,
            diff_offset: 0,
            diff_cursor: 0,
            horizontal: 0,
            nav_horizontal: 0,
            nav_manual_scroll: false,
            nav_hovered: None,
            folder_animation: None,
            canvas_manual_scroll: false,
            commit_offset: 0,
            commit_manual_scroll: false,
            split: false,
            nav_width: 34,
            overlay: None,
            editor_offset: 0,
            divider_hovered: false,
            divider_hover_row: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NavSummary {
    pub items: usize,
    pub staged: usize,
    pub unstaged: usize,
    pub has_commits: bool,
}

pub struct RenderData<'a> {
    pub commit: Option<&'a crate::commit::CommitDetails>,
    pub avatar: Option<&'a crate::commit::avatar::Avatar>,
    pub inspecting_commit: bool,
    pub repo: &'a str,
    pub branch: &'a str,
    pub ahead: usize,
    pub behind: usize,
    pub items: &'a [NavItem],
    pub nav_summary: Option<NavSummary>,
    pub changes_filter: Option<DiffSide>,
    pub canvas_path: &'a str,
    pub canvas_path_kind: Option<ItemKind>,
    pub directory_scope: Option<&'a std::path::Path>,
    pub diff_scope: Option<DiffScope>,
    pub canvas_subtitle: &'a str,
    pub diff: Option<&'a ExactDiff>,
    pub presentation: Option<&'a crate::presentation::DiffPresentation>,
    pub diff_rows: Option<(&'a [DiffRow], bool, u16)>,
    pub diff_paint: Option<&'a DiffPaint>,
    pub diff_counts: Option<(usize, usize)>,
    pub canvas_lines: &'a [String],
    pub structural: Option<&'a str>,
    pub operation: Option<&'a OperationView>,
    pub toast: Option<&'a ToastView<'a>>,
    pub selected_lines: &'a BTreeSet<usize>,
}

impl Default for RenderData<'_> {
    fn default() -> Self {
        static EMPTY_SELECTION: OnceLock<BTreeSet<usize>> = OnceLock::new();
        Self {
            commit: None,
            avatar: None,
            inspecting_commit: false,
            repo: "",
            branch: "",
            ahead: 0,
            behind: 0,
            items: &[],
            nav_summary: None,
            changes_filter: None,
            canvas_path: "",
            canvas_path_kind: None,
            directory_scope: None,
            diff_scope: None,
            canvas_subtitle: "",
            diff: None,
            presentation: None,
            diff_rows: None,
            diff_paint: None,
            diff_counts: None,
            canvas_lines: &[],
            structural: None,
            operation: None,
            toast: None,
            selected_lines: EMPTY_SELECTION.get_or_init(BTreeSet::new),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub commit_details: Rect,
    pub commit_fields: Vec<(usize, Rect)>,
    pub commit_scroll_max: usize,
    pub commit_subject: Rect,
    pub editor: Option<Rect>,
    pub overlay_close: Option<Rect>,
    pub overlay_confirm: Option<Rect>,
    pub overlay_bounds: Rect,
    pub body: Rect,
    pub nav: Rect,
    pub canvas: Rect,
    pub nav_footer: Rect,
    pub canvas_footer: Rect,
    pub divider_x: Option<u16>,
    pub divider_hit: Option<Rect>,
    pub nav_start: usize,
    pub nav_disclosures: Vec<(usize, Rect)>,
    pub line_ids: Vec<Option<usize>>,
    pub right_line_ids: Vec<Option<usize>>,
    pub hunk_indices: Vec<Option<usize>>,
    pub tabs: Vec<(Workspace, Rect)>,
    pub diff_sides: Vec<(DiffSide, Rect)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiffRow {
    pub hunk: usize,
    pub hunk_end: bool,
    pub line: Option<usize>,
    pub right_line: Option<usize>,
    pub left_offset: usize,
    pub right_offset: usize,
    pub left_part: usize,
    pub right_part: usize,
}

impl DiffRow {
    pub(crate) fn is_hunk_header(&self) -> bool {
        !self.hunk_end && self.line.is_none() && self.right_line.is_none()
    }
}

pub fn canvas_width(area: Rect, state: &ViewState, inspecting_commit: bool) -> u16 {
    if area.width < 20 || area.height < 10 {
        return 0;
    }
    if area.width < 80 {
        return if state.focus == Focus::Canvas
            || (state.focus == Focus::CommitDetails && !inspecting_commit)
        {
            area.width
        } else {
            0
        };
    }
    let navigator = state.nav_width.clamp(22, area.width.saturating_sub(34));
    area.width.saturating_sub(navigator + 1)
}

fn palette() -> &'static Palette {
    static PALETTE: OnceLock<Palette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        Palette::from_json(
            r##"{"version":1,"profile":"dogit","dark":true,"colors":{"fg":"#edf3f8"},"ui":{"foreground":"#edf3f8","muted":"#b0bdcc","accent":"#82d7e9","success":"#83dca2","warning":"#f4c66a","danger":"#f4c66a","info":"#edf3f8","border":"#53667e","selection_foreground":"#edf3f8","selection_background":"#293d50","surface":"#1d2b39","diff_added":"#83dca2","diff_removed":"#ff8c87","diff_context":"#edf3f8","diff_header":"#b0bdcc","ours":"#edf3f8","theirs":"#edf3f8","conflict":"#f4c66a","panel_foreground":"#edf3f8","surface_foreground":"#edf3f8","focus":"#82d7e9"}}"##,
        )
        .unwrap_or_default()
    })
}

fn color_depth() -> ColorDepth {
    static DEPTH: OnceLock<ColorDepth> = OnceLock::new();
    *DEPTH.get_or_init(ColorDepth::detect)
}

fn style(role: Role) -> Style {
    style_at_depth(role, color_depth())
}

fn style_at_depth(role: Role, depth: ColorDepth) -> Style {
    let appearance = palette()
        .style(role)
        .bg(Color::Reset)
        .remove_modifier(Modifier::REVERSED | Modifier::BOLD);
    if depth != ColorDepth::Ansi16 {
        return appearance;
    }
    let foreground = match role {
        Role::Accent | Role::Focus => Color::LightCyan,
        Role::Success | Role::DiffAdded => Color::LightGreen,
        Role::DiffRemoved => Color::LightRed,
        Role::Warning | Role::Danger | Role::Conflict => Color::Yellow,
        Role::Border => Color::DarkGray,
        Role::Muted | Role::DiffHeader => Color::Gray,
        _ => Color::White,
    };
    appearance.fg(foreground)
}

fn foreground(role: Role) -> Color {
    style(role).fg.unwrap_or(Color::Reset)
}

fn file_side_style(side: DiffSide) -> Style {
    let color = match (side, color_depth()) {
        (DiffSide::Index, ColorDepth::Ansi16) => Color::LightGreen,
        (DiffSide::Worktree, ColorDepth::Ansi16) => Color::Yellow,
        (DiffSide::Index, depth) => ui_theme::Color::Rgb(0x73, 0xc9, 0x91)
            .at_depth(depth)
            .ratatui(),
        (DiffSide::Worktree, depth) => ui_theme::Color::Rgb(0xe5, 0xba, 0x7d)
            .at_depth(depth)
            .ratatui(),
    };
    style(Role::Plain).fg(color)
}

pub fn render(frame: &mut Frame, state: &mut ViewState, data: &RenderData<'_>) -> Geometry {
    let area = frame.area();
    frame.buffer_mut().set_style(area, style(Role::Background));
    let tabs = render_tabs(frame, area, state);
    let totals = data.nav_summary.unwrap_or_else(|| {
        data.items
            .iter()
            .fold(NavSummary::default(), |mut totals, item| {
                totals.items += usize::from(item.kind != ItemKind::Folder);
                totals.staged += usize::from(item.kind != ItemKind::Folder && item.staged);
                totals.unstaged += usize::from(
                    item.kind != ItemKind::Folder
                        && item.file_state.is_some_and(|file| file.unstaged),
                );
                totals.has_commits |= item.kind == ItemKind::Commit;
                totals
            })
    });
    let changes = state.workspace == Workspace::Changes;

    if area.width < 20 || area.height < 10 {
        render_footer(frame, area, data, Vec::new(), "");
        let mut geometry = Geometry {
            tabs,
            overlay_bounds: area,
            ..Geometry::default()
        };
        if let Some(overlay) = &state.overlay {
            render_overlay(
                frame,
                area,
                overlay,
                &mut state.editor_offset,
                &mut geometry,
            );
        }
        if state.overlay.is_none()
            && let Some(toast) = data.toast
        {
            toast::render(frame.buffer_mut(), area, toast);
        }
        return geometry;
    }
    let outer = Rect::new(area.x, area.y + 2, area.width, area.height - 3);
    let body = Rect::new(
        outer.x,
        outer.y,
        outer.width,
        outer.height - u16::from(!changes),
    );
    let compact = area.width < 80;
    let show_nav = !compact
        || state.focus == Focus::Navigator
        || (state.focus == Focus::CommitDetails && data.inspecting_commit);
    let show_canvas = !compact
        || state.focus == Focus::Canvas
        || (state.focus == Focus::CommitDetails && !data.inspecting_commit);
    let nav_width = if show_nav && show_canvas {
        state.nav_width.clamp(22, area.width.saturating_sub(34))
    } else if show_nav {
        body.width
    } else {
        0
    };
    let divider_x = (show_nav && show_canvas).then_some(outer.x + nav_width);
    let mut nav = if show_nav {
        Rect::new(body.x, body.y, nav_width, outer.height)
    } else {
        Rect::default()
    };
    let mut canvas = if show_canvas {
        let x = divider_x.map_or(body.x, |x| x + 1);
        Rect::new(x, body.y, outer.right() - x, body.height)
    } else {
        Rect::default()
    };
    let canvas_footer = if show_canvas && !changes {
        Rect::new(canvas.x, outer.bottom() - 1, canvas.width, 1)
    } else {
        Rect::default()
    };
    if let Some(x) = divider_x {
        for y in body.y + 1..body.bottom().saturating_sub(1) {
            put(frame.buffer_mut(), x, y, 1, "│", style(Role::Border));
        }
    }
    let values = data.diff_counts.or_else(|| {
        data.diff.map(|diff| {
            diff.hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .fold((0, 0), |(added, removed), line| {
                    (
                        added + usize::from(line.kind == LineKind::Addition),
                        removed + usize::from(line.kind == LineKind::Deletion),
                    )
                })
        })
    });
    let mut counts = if show_canvas && let Some((added, removed)) = values {
        [
            ("+", Role::DiffAdded, added),
            ("-", Role::DiffRemoved, removed),
        ]
        .into_iter()
        .map(|(sign, role, count)| Badge::new(format!("{sign}{count}"), style(role)))
        .collect()
    } else {
        Vec::new()
    };
    if show_canvas && let Some(details) = data.commit {
        let mut badges = commit::verification_badges(details);
        badges.extend(counts);
        counts = badges;
    }
    let title_reserved = if show_canvas && data.commit.is_some() {
        let badges = render_canvas_badges(frame.buffer_mut(), canvas, std::mem::take(&mut counts));
        if badges.width > 0 {
            canvas.right() - badges.x
        } else {
            0
        }
    } else {
        0
    };
    let mut commit_details = Rect::default();
    let mut commit_fields = Vec::new();
    let mut commit_scroll_max = 0;
    let mut commit_subject = Rect::default();
    if let Some(details) = data.commit {
        if show_canvas && data.inspecting_commit {
            (canvas, commit_subject) =
                commit::subject(frame.buffer_mut(), canvas, details, title_reserved);
        }
        if data.inspecting_commit && show_nav {
            let height =
                (commit::height(nav.width, details, true, 0) + 2).min(nav.height.saturating_sub(4));
            let top = nav.bottom() - height;
            commit_details = Rect::new(nav.x, top + 2, nav.width, height.saturating_sub(2));
            nav.height -= height;
            let width = (nav.width.saturating_sub(4) / 2).min(24);
            put(
                frame.buffer_mut(),
                nav.x + 2,
                top,
                width,
                &"─".repeat(usize::from(width)),
                style(Role::Border),
            );
        } else if !data.inspecting_commit && show_canvas {
            let height = commit::height(canvas.width, details, false, title_reserved).min(
                canvas
                    .height
                    .saturating_sub(if state.focus == Focus::CommitDetails {
                        0
                    } else {
                        4
                    }),
            );
            commit_details = Rect::new(canvas.x, canvas.y, canvas.width, height);
            canvas.y += height;
            canvas.height -= height;
        }
        let (fields, title, scroll_max) = commit::details(
            frame.buffer_mut(),
            commit_details,
            state,
            details,
            if state.overlay.is_none() {
                data.avatar
            } else {
                None
            },
            data.inspecting_commit,
            if data.inspecting_commit {
                0
            } else {
                title_reserved
            },
        );
        commit_fields = fields;
        commit_scroll_max = scroll_max;
        if !data.inspecting_commit {
            commit_subject = title;
        }
    }
    let nav_disclosures = if show_nav {
        render_navigator(frame.buffer_mut(), nav, state, data.items)
    } else {
        Vec::new()
    };
    if show_canvas && !changes {
        render_pane_footer(
            frame.buffer_mut(),
            canvas_footer,
            Vec::new(),
            data.canvas_subtitle,
        );
    }
    let (canvas, diff_sides) = if show_canvas {
        render_canvas_context(frame.buffer_mut(), canvas, data, counts)
    } else {
        (canvas, Vec::new())
    };
    let mut geometry = Geometry {
        commit_details,
        commit_fields,
        commit_scroll_max,
        commit_subject,
        overlay_bounds: area,
        body,
        nav,
        canvas,
        canvas_footer,
        divider_x,
        divider_hit: divider_x.map(|x| {
            Rect::new(
                x.saturating_sub(2),
                body.y + 1,
                5,
                body.height.saturating_sub(2),
            )
        }),
        nav_start: state.offset,
        nav_disclosures,
        tabs,
        diff_sides,
        ..Geometry::default()
    };
    if show_canvas {
        if let Some(structural) = data.structural {
            render_structural(frame.buffer_mut(), canvas, state, structural);
        } else if let Some(diff) = data.diff {
            render_diff(frame.buffer_mut(), canvas, state, data, diff, &mut geometry);
        } else if !data.canvas_lines.is_empty() {
            render_text(frame.buffer_mut(), canvas, state, data.canvas_lines);
        } else if changes && totals.items == 0 {
            render_no_changes(frame.buffer_mut(), canvas);
        }
    }
    if state.divider_hovered
        && state.overlay.is_none()
        && let Some(x) = divider_x
    {
        render_divider_hover(frame.buffer_mut(), body, x, state.divider_hover_row);
    }
    if let Some(overlay) = &state.overlay {
        render_overlay(
            frame,
            area,
            overlay,
            &mut state.editor_offset,
            &mut geometry,
        );
    }
    geometry.nav_footer = render_footer(
        frame,
        area,
        data,
        nav_badges(state.workspace, totals, data.repo, data.changes_filter),
        if changes { data.canvas_subtitle } else { "" },
    );
    if state.overlay.is_none()
        && let Some(toast) = data.toast
    {
        toast::render(frame.buffer_mut(), area, toast);
    }
    geometry
}

fn render_canvas_badges(buffer: &mut Buffer, area: Rect, counts: Vec<Badge>) -> Rect {
    let right = area.right().saturating_sub(1);
    let width = counts
        .iter()
        .map(|badge| badge.width() + 1)
        .sum::<usize>()
        .saturating_sub(1);
    let width = width.min(usize::from(area.width.saturating_sub(2))) as u16;
    if !area.is_empty() && width >= 9 {
        render_pane_footer(
            buffer,
            Rect::new(right - width, area.y, width, 1),
            counts,
            "",
        )
    } else {
        Rect::new(right, area.y, 0, 0)
    }
}

fn render_canvas_context(
    buffer: &mut Buffer,
    area: Rect,
    data: &RenderData<'_>,
    counts: Vec<Badge>,
) -> (Rect, Vec<(DiffSide, Rect)>) {
    if area.is_empty()
        || (data.canvas_path.is_empty() && data.diff_scope.is_none() && counts.is_empty())
    {
        return (area, Vec::new());
    }
    let mut sides = Vec::new();
    let mut left = area.x;
    let mut right = area.right().saturating_sub(1);
    let badges = render_canvas_badges(buffer, area, counts);
    if badges.width > 0 {
        right = badges.x.saturating_sub(1);
    }
    if let Some(scope) = data.diff_scope {
        for (side, available, label) in [
            (DiffSide::Index, scope.staged, " Staged "),
            (DiffSide::Worktree, scope.unstaged, " Unstaged "),
        ] {
            if !available {
                continue;
            }
            let width = label.width() as u16;
            if left + width > right {
                continue;
            }
            let rect = Rect::new(left, area.y, width, area.height.min(2));
            let appearance = chrome::badge_style(file_side_style(side), color_depth());
            put(
                buffer,
                rect.x,
                rect.y,
                rect.width,
                label,
                appearance.add_modifier(Modifier::BOLD),
            );
            sides.push((side, rect));
            left = rect.right() + 1;
        }
    } else {
        let path_x = area.x + 2;
        let path_width = right.saturating_sub(path_x);
        if let Some(kind) = data.canvas_path_kind {
            files::FileLabel::new(data.canvas_path, kind == ItemKind::Folder)
                .render(buffer, Rect::new(path_x, area.y, path_width, 1));
        } else {
            put(
                buffer,
                path_x,
                area.y,
                path_width,
                &elide_middle(data.canvas_path, usize::from(path_width)),
                style(Role::Plain),
            );
        }
    }
    if area.height > 1 {
        let controls_width = sides.last().map_or(0, |(_, rect)| rect.right() - area.x);
        put(
            buffer,
            area.x,
            area.y + 1,
            controls_width,
            &"─".repeat(usize::from(controls_width)),
            style(Role::Border),
        );
        for (side, rect) in &sides {
            if data.diff_scope.is_some_and(|scope| scope.side == *side) {
                put(
                    buffer,
                    rect.x,
                    area.y + 1,
                    rect.width,
                    &"━".repeat(usize::from(rect.width)),
                    file_side_style(*side),
                );
            }
        }
    }
    let used = area.height.min(2);
    (
        Rect::new(area.x, area.y + used, area.width, area.height - used),
        sides,
    )
}

fn render_divider_hover(buffer: &mut Buffer, area: Rect, x: u16, pointer_row: Option<u16>) {
    let appearance = style(Role::Accent);
    for y in area.y + 1..area.bottom() - 1 {
        put(buffer, x, y, 1, "┃", appearance);
    }
    if let Some(row) = pointer_row {
        let row = row.clamp(area.y + 1, area.bottom() - 2);
        put(buffer, x.saturating_sub(1), row, 3, "‹╫›", appearance);
    }
}

fn border_title(buffer: &mut Buffer, x: u16, y: u16, width: u16, title: &str, active: bool) {
    if title.is_empty() {
        return;
    }
    let title = elide_middle(title, usize::from(width.saturating_sub(2)));
    put(
        buffer,
        x,
        y,
        width,
        &format!(" {title} "),
        if active {
            style(Role::Accent)
        } else {
            style(Role::Plain)
        },
    );
}

fn render_operation(buffer: &mut Buffer, area: Rect, operation: &OperationView) {
    let appearance = if operation.tone == Tone::Attention {
        style(Role::Warning)
    } else {
        style(Role::Plain)
    };
    let text = format!(" {} ", operation.label);
    let label_width = text.width().min(usize::from(area.width.saturating_sub(4))) as u16;
    put(
        buffer,
        area.x + 2,
        area.bottom() - 1,
        label_width,
        &text,
        appearance,
    );
    if !operation.detail.is_empty() && area.width > label_width + 20 {
        let detail = format!(" {} ", operation.detail);
        let width = detail
            .width()
            .min(usize::from(area.width.saturating_sub(label_width + 6)))
            as u16;
        put(
            buffer,
            area.right().saturating_sub(width + 2),
            area.bottom() - 1,
            width,
            &detail,
            appearance,
        );
    }
}

fn put(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, appearance: Style) {
    if width == 0
        || x >= buffer.area.right()
        || y >= buffer.area.bottom()
        || x < buffer.area.x
        || y < buffer.area.y
    {
        return;
    }
    let width = width.min(buffer.area.right() - x);
    if text.chars().any(char::is_control) {
        buffer.set_stringn(x, y, safe_text(text), usize::from(width), appearance);
    } else {
        buffer.set_stringn(x, y, text, usize::from(width), appearance);
    }
}
