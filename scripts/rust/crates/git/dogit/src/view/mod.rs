use std::collections::BTreeSet;
use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget, Wrap};
use ui_theme::{ColorDepth, Palette, Role};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::diff::{DiffSide, ExactDiff, LineKind};

mod chrome;
mod diff;
mod navigator;
mod overlay;
mod text;
mod wrap;

pub use diff::{
    configured_diff_horizontal_limit, diff_horizontal_limit, diff_rows, text_horizontal_limit,
};
pub use navigator::{graph_rows, nav_horizontal_limit, tree_prefixes};
pub use overlay::confirmation_scroll_max;
pub use wrap::{DiffLayout, DiffPaint, configured_diff_layout};

use chrome::{render_footer, render_tabs};
use diff::{render_diff, render_structural, render_text};
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
    Operations,
}

impl Workspace {
    pub const ALL: [Self; 7] = [
        Self::Changes,
        Self::History,
        Self::Branches,
        Self::Stashes,
        Self::Worktrees,
        Self::Compare,
        Self::Operations,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::History => "History",
            Self::Branches => "Branches",
            Self::Stashes => "Stashes",
            Self::Worktrees => "Worktrees",
            Self::Compare => "Compare",
            Self::Operations => "Operations",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Focus {
    #[default]
    Navigator,
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
    Operation,
}

#[derive(Clone, Debug, Default)]
pub struct NavItem {
    pub label: String,
    pub detail: String,
    pub status: String,
    pub depth: usize,
    pub kind: ItemKind,
    pub staged: bool,
    pub graph: String,
    pub file_state: Option<FileState>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileState {
    pub staged: bool,
    pub unstaged: bool,
    pub conflicted: bool,
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
    Running,
}

#[derive(Clone, Debug, Default)]
pub struct OperationView {
    pub label: String,
    pub detail: String,
    pub tone: Tone,
    pub phase: usize,
}

#[derive(Clone, Debug)]
pub enum Overlay {
    Input {
        title: String,
        value: String,
        detail: String,
    },
    Compose {
        summary: String,
        body: String,
        body_started: bool,
    },
    Commands {
        query: String,
        items: Vec<NavItem>,
        selected: usize,
    },
    Confirm {
        title: String,
        detail: String,
        destructive: bool,
        scroll: u16,
    },
}

#[derive(Clone, Debug)]
pub struct ViewState {
    pub workspace: Workspace,
    pub focus: Focus,
    pub selected: usize,
    pub offset: usize,
    pub diff_offset: usize,
    pub diff_cursor: usize,
    pub horizontal: usize,
    pub nav_horizontal: usize,
    pub nav_manual_scroll: bool,
    pub canvas_manual_scroll: bool,
    pub split: bool,
    pub nav_width: u16,
    pub overlay: Option<Overlay>,
    pub divider_hovered: bool,
    pub divider_hover_row: Option<u16>,
    pub tab_indicator: Option<chrome::TabIndicator>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            workspace: Workspace::Changes,
            focus: Focus::Navigator,
            selected: 0,
            offset: 0,
            diff_offset: 0,
            diff_cursor: 0,
            horizontal: 0,
            nav_horizontal: 0,
            nav_manual_scroll: false,
            canvas_manual_scroll: false,
            split: false,
            nav_width: 34,
            overlay: None,
            divider_hovered: false,
            divider_hover_row: None,
            tab_indicator: None,
        }
    }
}

impl ViewState {
    pub fn animating(&self) -> bool {
        self.tab_indicator
            .as_ref()
            .is_some_and(|indicator| indicator.moving())
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
    pub mouse_capture: ui_terminal::MouseCapture,
    pub repo: &'a str,
    pub branch: &'a str,
    pub ahead: usize,
    pub behind: usize,
    pub nav_title: &'a str,
    pub items: &'a [NavItem],
    /// Totals cached when navigator contents change.
    pub nav_summary: Option<NavSummary>,
    pub canvas_title: &'a str,
    pub canvas_path: &'a str,
    pub diff_scope: Option<DiffScope>,
    pub canvas_subtitle: &'a str,
    pub diff: Option<&'a ExactDiff>,
    pub presentation: Option<&'a crate::presentation::DiffPresentation>,
    /// Cached display rows with their split mode and canvas width.
    pub diff_rows: Option<(&'a [DiffRow], bool, u16)>,
    pub diff_paint: Option<&'a DiffPaint>,
    /// Added and removed line counts cached with the exact patch.
    pub diff_counts: Option<(usize, usize)>,
    pub canvas_lines: &'a [String],
    pub structural: Option<&'a str>,
    pub operation: Option<&'a OperationView>,
    pub selected_lines: &'a BTreeSet<usize>,
}

impl Default for RenderData<'_> {
    fn default() -> Self {
        static EMPTY_SELECTION: OnceLock<BTreeSet<usize>> = OnceLock::new();
        Self {
            mouse_capture: ui_terminal::MouseCapture::Enabled,
            repo: "",
            branch: "",
            ahead: 0,
            behind: 0,
            nav_title: "",
            items: &[],
            nav_summary: None,
            canvas_title: "",
            canvas_path: "",
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
            selected_lines: EMPTY_SELECTION.get_or_init(BTreeSet::new),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub body: Rect,
    pub nav: Rect,
    pub canvas: Rect,
    pub divider_x: Option<u16>,
    pub divider_hit: Option<Rect>,
    pub nav_start: usize,
    /// Exact Git line ID at each visible canvas row. Hunk headings have no ID.
    pub line_ids: Vec<Option<usize>>,
    /// Right-hand line IDs when a split diff is visible.
    pub right_line_ids: Vec<Option<usize>>,
    pub hunk_indices: Vec<Option<usize>>,
    pub tabs: Vec<(Workspace, Rect)>,
    pub diff_sides: Vec<(DiffSide, Rect)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiffRow {
    pub hunk: usize,
    pub line: Option<usize>,
    pub right_line: Option<usize>,
    pub left_offset: usize,
    pub right_offset: usize,
    pub left_part: usize,
    pub right_part: usize,
}

/// The patch width is independent of the path/scope header height.
pub fn canvas_width(area: Rect, state: &ViewState) -> u16 {
    if area.width < 20 || area.height < 10 {
        return 0;
    }
    if area.width < 80 {
        return if state.focus == Focus::Canvas {
            area.width.saturating_sub(2)
        } else {
            0
        };
    }
    let navigator = state.nav_width.clamp(22, area.width.saturating_sub(34));
    area.width.saturating_sub(navigator + 3)
}

fn palette() -> &'static Palette {
    static PALETTE: OnceLock<Palette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        // Semantic colors never encode staging or success. Surfaces inherit the terminal background.
        Palette::from_json(
            r##"{"version":1,"profile":"dogit","dark":true,"colors":{"fg":"#edf3f8"},"ui":{"foreground":"#edf3f8","muted":"#b0bdcc","accent":"#82d7e9","success":"#edf3f8","warning":"#f4c66a","danger":"#f4c66a","info":"#edf3f8","border":"#53667e","selection_foreground":"#edf3f8","diff_added":"#83dca2","diff_removed":"#ff8c87","diff_context":"#edf3f8","diff_header":"#b0bdcc","ours":"#edf3f8","theirs":"#edf3f8","conflict":"#f4c66a","panel_foreground":"#edf3f8","surface_foreground":"#edf3f8","focus":"#82d7e9"}}"##,
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
        .remove_modifier(Modifier::REVERSED);
    if depth != ColorDepth::Ansi16 {
        return appearance;
    }
    // Nearest-RGB quantization turns several pale semantic colors white. Keep
    // their meanings distinct even when a terminal advertises only 16 colors.
    let foreground = match role {
        Role::Accent | Role::Focus => Color::LightCyan,
        Role::DiffAdded => Color::LightGreen,
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

fn strong() -> Style {
    style(Role::Plain).add_modifier(Modifier::BOLD)
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
    if area.width < 20 || area.height < 10 {
        render_footer(frame, area, data);
        return Geometry {
            tabs,
            ..Geometry::default()
        };
    }
    let outer = Rect::new(area.x, area.y + 4, area.width, area.height - 4);
    let body = Rect::new(outer.x + 1, outer.y + 1, outer.width - 2, outer.height - 4);
    let compact = area.width < 80;
    let show_nav = !compact || state.focus == Focus::Navigator;
    let show_canvas = !compact || state.focus == Focus::Canvas;
    let nav_width = if show_nav && show_canvas {
        state.nav_width.clamp(22, area.width.saturating_sub(34))
    } else if show_nav {
        body.width
    } else {
        0
    };
    let divider_x = (show_nav && show_canvas).then_some(outer.x + nav_width + 1);
    let nav = if show_nav {
        Rect::new(body.x, body.y, nav_width, body.height)
    } else {
        Rect::default()
    };
    let canvas = if show_canvas {
        let x = divider_x.map_or(body.x, |x| x + 1);
        Rect::new(x, body.y, outer.right() - x - 1, body.height)
    } else {
        Rect::default()
    };
    draw_frame(frame.buffer_mut(), outer, divider_x, state.focus);

    if show_nav {
        let title = if data.nav_title.is_empty() {
            state.workspace.label()
        } else {
            data.nav_title
        };
        border_title(
            frame.buffer_mut(),
            nav.x + 1,
            outer.y,
            nav.width.saturating_sub(2),
            title,
            state.focus == Focus::Navigator,
        );
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
        let (count, staged) = (totals.items, totals.staged);
        let summary = if staged > 0 {
            format!("{count} files    {staged} staged")
        } else {
            match state.workspace {
                Workspace::Changes => format!("{count} files"),
                Workspace::History if totals.has_commits => {
                    format!("{count} commits")
                }
                Workspace::History => format!("{count} files"),
                Workspace::Branches => format!("{count} branches"),
                Workspace::Stashes => format!("{count} stashes"),
                Workspace::Worktrees => format!("{count} worktrees"),
                Workspace::Operations => format!("{count} operations"),
                Workspace::Compare => format!("{count} files"),
            }
        };
        if state.workspace == Workspace::Changes {
            render_tree_summary(
                frame.buffer_mut(),
                Rect::new(
                    nav.x + 2,
                    outer.bottom() - 2,
                    nav.width.saturating_sub(3),
                    1,
                ),
                totals,
            );
        } else {
            put(
                frame.buffer_mut(),
                nav.x + 2,
                outer.bottom() - 2,
                nav.width.saturating_sub(3),
                &summary,
                style(Role::Plain),
            );
        }
        render_navigator(frame.buffer_mut(), nav, state, data.items);
    }
    if show_canvas {
        border_title(
            frame.buffer_mut(),
            canvas.x + 1,
            outer.y,
            canvas.width.saturating_sub(2),
            data.canvas_title,
            state.focus == Focus::Canvas,
        );
        let counts = data.diff.map(|diff| {
            data.diff_counts.unwrap_or_else(|| {
                diff.hunks.iter().flat_map(|hunk| hunk.lines.iter()).fold(
                    (0, 0),
                    |(added, removed), line| {
                        (
                            added + usize::from(line.kind == LineKind::Addition),
                            removed + usize::from(line.kind == LineKind::Deletion),
                        )
                    },
                )
            })
        });
        let count_width = counts.map_or(0, |(added, removed)| {
            format!("+{added}  -{removed}").len() as u16 + 3
        });
        put(
            frame.buffer_mut(),
            canvas.x + 2,
            outer.bottom() - 2,
            canvas.width.saturating_sub(count_width + 3),
            data.canvas_subtitle,
            style(Role::Plain),
        );
        if let Some((added, removed)) = counts {
            let text = format!("+{added}  -{removed}");
            let x = canvas.right().saturating_sub(text.len() as u16 + 2);
            put(
                frame.buffer_mut(),
                x,
                outer.bottom() - 2,
                canvas.width,
                &format!("+{added}"),
                style(Role::DiffAdded),
            );
            put(
                frame.buffer_mut(),
                x + format!("+{added}  ").len() as u16,
                outer.bottom() - 2,
                canvas.width,
                &format!("-{removed}"),
                style(Role::DiffRemoved),
            );
        }
    }
    let (canvas, diff_sides) = if show_canvas {
        render_canvas_context(frame.buffer_mut(), canvas, data)
    } else {
        (canvas, Vec::new())
    };
    let mut geometry = Geometry {
        body,
        nav,
        canvas,
        divider_x,
        divider_hit: divider_x
            .map(|x| Rect::new(x.saturating_sub(2), outer.y + 1, 5, outer.height - 2)),
        nav_start: state.offset,
        tabs,
        diff_sides,
        ..Geometry::default()
    };
    if show_canvas {
        if let Some(structural) = data.structural {
            render_structural(frame.buffer_mut(), canvas, state, structural);
        } else if let Some(diff) = data.diff {
            render_diff(frame.buffer_mut(), canvas, state, data, diff, &mut geometry);
        } else if !data.canvas_lines.is_empty()
            || (data.items.is_empty() && data.canvas_subtitle != "Loading")
        {
            render_text(frame.buffer_mut(), canvas, state, data.canvas_lines);
        }
    }
    if state.divider_hovered
        && state.overlay.is_none()
        && let Some(x) = divider_x
    {
        render_divider_hover(frame.buffer_mut(), outer, x, state.divider_hover_row);
    }
    if let Some(overlay) = &state.overlay {
        render_overlay(frame, outer, canvas, overlay);
    }
    render_footer(frame, area, data);
    geometry
}

fn render_tree_summary(buffer: &mut Buffer, area: Rect, summary: NavSummary) {
    let staged = format!("{} Staged", summary.staged);
    let unstaged = format!("{} Unstaged", summary.unstaged);
    let total = format!("{} files", summary.items);
    let full = total.width() + staged.width() + unstaged.width() + 4;
    let mut x = area.x;
    if full <= usize::from(area.width) {
        put(buffer, x, area.y, area.width, &total, style(Role::Plain));
        x += total.width() as u16 + 2;
    }
    let compact = staged.width() + unstaged.width() + 2 > usize::from(area.width);
    let staged = if compact {
        format!("{} S", summary.staged)
    } else {
        staged
    };
    let unstaged = if compact {
        format!("{} U", summary.unstaged)
    } else {
        unstaged
    };
    put(
        buffer,
        x,
        area.y,
        area.right().saturating_sub(x),
        &staged,
        file_side_style(DiffSide::Index),
    );
    x += staged.width() as u16 + 2;
    put(
        buffer,
        x,
        area.y,
        area.right().saturating_sub(x),
        &unstaged,
        file_side_style(DiffSide::Worktree),
    );
}

fn render_canvas_context(
    buffer: &mut Buffer,
    area: Rect,
    data: &RenderData<'_>,
) -> (Rect, Vec<(DiffSide, Rect)>) {
    if area.is_empty() || (data.canvas_path.is_empty() && data.diff_scope.is_none()) {
        return (area, Vec::new());
    }
    let mut sides = Vec::new();
    let mut right = area.right().saturating_sub(1);
    if let Some(scope) = data.diff_scope {
        for (side, available, label) in [
            (DiffSide::Index, scope.staged, " Staged "),
            (DiffSide::Worktree, scope.unstaged, " Unstaged "),
        ] {
            if !available {
                continue;
            }
            let width = label.width() as u16;
            if right < area.x + width + 2 {
                continue;
            }
            let rect = Rect::new(right - width, area.y, width, area.height.min(2));
            let appearance = file_side_style(side);
            put(
                buffer,
                rect.x,
                rect.y,
                rect.width,
                label,
                if scope.side == side {
                    appearance.add_modifier(Modifier::BOLD)
                } else {
                    appearance
                },
            );
            sides.push((side, rect));
            right = rect.x.saturating_sub(1);
        }
    }
    let path_width = right.saturating_sub(area.x + 3);
    put(
        buffer,
        area.x + 2,
        area.y,
        path_width,
        &elide_middle(data.canvas_path, usize::from(path_width)),
        strong(),
    );
    if area.height > 1 {
        put(
            buffer,
            area.x,
            area.y + 1,
            area.width,
            &"─".repeat(usize::from(area.width)),
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

fn draw_frame(buffer: &mut Buffer, area: Rect, divider: Option<u16>, focus: Focus) {
    let border = style(Role::Border);
    for y in area.y..area.bottom() {
        put(buffer, area.x, y, 1, "│", border);
        put(buffer, area.right() - 1, y, 1, "│", border);
        if let Some(x) = divider {
            put(buffer, x, y, 1, "│", border);
        }
    }
    for y in [area.y, area.bottom() - 3, area.bottom() - 1] {
        put(
            buffer,
            area.x,
            y,
            area.width,
            &"─".repeat(usize::from(area.width)),
            border,
        );
    }
    for (x, y, symbol) in [
        (area.x, area.y, "┌"),
        (area.right() - 1, area.y, "┐"),
        (area.x, area.bottom() - 3, "├"),
        (area.right() - 1, area.bottom() - 3, "┤"),
        (area.x, area.bottom() - 1, "└"),
        (area.right() - 1, area.bottom() - 1, "┘"),
    ] {
        put(buffer, x, y, 1, symbol, border);
    }
    if let Some(x) = divider {
        for (y, symbol) in [
            (area.y, "┬"),
            (area.bottom() - 3, "┼"),
            (area.bottom() - 1, "┴"),
        ] {
            put(buffer, x, y, 1, symbol, border);
        }
        let edge = if focus == Focus::Navigator { area.x } else { x };
        put(buffer, edge, area.y + 1, 1, "┃", style(Role::Accent));
    } else {
        put(buffer, area.x, area.y + 1, 1, "┃", style(Role::Accent));
    }
}

fn render_divider_hover(buffer: &mut Buffer, area: Rect, x: u16, pointer_row: Option<u16>) {
    let appearance = style(Role::Accent);
    for y in area.y + 1..area.bottom() - 1 {
        put(
            buffer,
            x,
            y,
            1,
            if y == area.bottom() - 3 { "┼" } else { "┃" },
            appearance,
        );
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
            style(Role::Accent).add_modifier(Modifier::BOLD)
        } else {
            strong()
        },
    );
}

fn render_operation(buffer: &mut Buffer, area: Rect, operation: &OperationView) {
    let appearance = if operation.tone == Tone::Attention {
        style(Role::Warning)
    } else {
        style(Role::Plain)
    };
    let marker = if operation.tone == Tone::Running {
        ["━──", "─━─", "──━", "─━─"][operation.phase % 4]
    } else {
        ""
    };
    let text = if marker.is_empty() {
        format!(" {} ", operation.label)
    } else {
        format!(" {marker} {} ", operation.label)
    };
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

#[cfg(test)]
mod tests;
