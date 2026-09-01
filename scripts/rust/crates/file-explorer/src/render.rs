use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use workstation::Style;

use crate::view::ViewContext;
use crate::{Directory, EntryKind, ExplorerView, InputKind, Line, Role, Selection, Size, Span};

const DEFAULT_MAX_WIDTH: usize = 78;
const DEFAULT_MAX_ROWS: usize = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RenderLimits {
    pub max_width: usize,
    pub max_rows: usize,
}

impl Default for RenderLimits {
    fn default() -> Self {
        Self {
            max_width: DEFAULT_MAX_WIDTH,
            max_rows: DEFAULT_MAX_ROWS,
        }
    }
}

pub(crate) struct RenderContext<'a, L> {
    pub directory: &'a Directory<L>,
    pub rows: &'a [usize],
    pub cursor: usize,
    pub offset: usize,
    pub prompt: Option<(&'a str, InputKind)>,
    pub error: Option<&'a str>,
    pub selection: Option<&'a Selection<L>>,
    pub help: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderedFrame {
    pub lines: Vec<String>,
    pub viewport_rows: usize,
}

pub(crate) fn render<L, V>(
    context: &RenderContext<'_, L>,
    view: &V,
    style: &Style,
    size: Size,
    limits: RenderLimits,
) -> RenderedFrame
where
    V: ExplorerView<L> + ?Sized,
{
    if size.width == 0 || size.height == 0 {
        return RenderedFrame {
            lines: Vec::new(),
            viewport_rows: 0,
        };
    }

    let focused = context
        .rows
        .get(context.cursor)
        .and_then(|index| context.directory.entries.get(*index));
    let view_context = ViewContext {
        directory: context.directory,
        focused,
        selection: context.selection,
        prompt: context.prompt,
        error: context.error,
    };
    let width = size.width.min(limits.max_width.max(1));
    let footer = footer(view, &view_context, context.help);
    let error = context.error.map(|message| {
        Line::from_spans([
            Span::new("error: ", Role::Danger),
            Span::new(message, Role::Danger),
        ])
    });
    let mut header = view.header(&view_context);
    let footer_rows = usize::from(size.height > 0);
    let error_rows = usize::from(error.is_some() && size.height > footer_rows);
    let box_min_rows = 3;
    let header_room = size
        .height
        .saturating_sub(footer_rows + error_rows + box_min_rows);
    header.truncate(header_room);
    let separator_rows = usize::from(
        !header.is_empty() && size.height > footer_rows + error_rows + box_min_rows + header.len(),
    );
    let box_rows = size
        .height
        .saturating_sub(footer_rows + error_rows + header.len() + separator_rows);
    let content_rows = box_rows.saturating_sub(2);
    let mut viewport_rows = content_rows.min(limits.max_rows);
    let overflow = context.rows.len() > viewport_rows;
    let overflow_rows = usize::from(overflow && viewport_rows > 1);
    viewport_rows = viewport_rows.saturating_sub(overflow_rows);

    let mut lines = Vec::with_capacity(size.height);
    lines.extend(header.into_iter().map(|line| paint(&line, style, width)));
    if separator_rows > 0 {
        lines.push(String::new());
    }

    if box_rows >= 2 {
        lines.extend(listing(
            context,
            view,
            &view_context,
            style,
            width,
            viewport_rows,
            overflow_rows > 0,
        ));
    } else if box_rows == 1 {
        lines.push(paint(&view.directory_title(&view_context), style, width));
    }

    if let Some(error) = error
        && error_rows > 0
    {
        lines.push(paint(&error, style, width));
    }
    if footer_rows > 0 {
        lines.push(paint(&footer, style, width));
    }
    lines.truncate(size.height);

    RenderedFrame {
        lines,
        viewport_rows,
    }
}

fn listing<L, V>(
    context: &RenderContext<'_, L>,
    view: &V,
    view_context: &ViewContext<'_, L>,
    style: &Style,
    width: usize,
    viewport_rows: usize,
    show_overflow: bool,
) -> Vec<String>
where
    V: ExplorerView<L> + ?Sized,
{
    let indent = usize::from(width >= 16) * 2;
    let box_width = width.saturating_sub(indent).max(2);
    let inner = box_width.saturating_sub(2);
    let mut lines = Vec::new();
    lines.push(paint(
        &top_border(view.directory_title(view_context), indent, inner),
        style,
        width,
    ));

    if context.rows.is_empty() && viewport_rows > 0 {
        let label = view
            .state_label(view_context, false)
            .unwrap_or_else(|| "(empty)".to_string());
        lines.push(paint(&boxed(row_label(label), indent, inner), style, width));
    } else {
        for (slot, index) in context
            .rows
            .iter()
            .enumerate()
            .skip(context.offset)
            .take(viewport_rows)
        {
            let Some(entry) = context.directory.entries.get(*index) else {
                continue;
            };
            let active = slot == context.cursor;
            let badge = view.badge(view_context, entry);
            lines.push(paint(
                &boxed(
                    entry_line(&entry.name, entry.kind, active, badge, inner),
                    indent,
                    inner,
                ),
                style,
                width,
            ));
        }
    }

    if show_overflow {
        lines.push(paint(
            &boxed(
                row_label(elsewhere(context.offset, viewport_rows, context.rows.len())),
                indent,
                inner,
            ),
            style,
            width,
        ));
    }
    lines.push(paint(&bottom_border(indent, inner), style, width));
    lines
}

fn top_border(mut title: Line, indent: usize, inner: usize) -> Line {
    let mut line = Line::from_spans([
        Span::new(" ".repeat(indent), Role::Plain),
        Span::new("┌", Role::Muted),
    ]);
    if inner > 0 {
        let room = inner.saturating_sub(2);
        title = fitted(title, room);
        let title_width = line_width(&title);
        if inner >= 2 {
            line.spans.push(Span::new(" ", Role::Muted));
        }
        line.spans.extend(title.spans);
        if inner > title_width + 1 {
            line.spans.push(Span::new(" ", Role::Muted));
        }
        let used = title_width + usize::from(inner >= 2) + usize::from(inner > title_width + 1);
        line.spans.push(Span::new(
            "─".repeat(inner.saturating_sub(used)),
            Role::Muted,
        ));
    }
    line.spans.push(Span::new("┐", Role::Muted));
    line
}

fn bottom_border(indent: usize, inner: usize) -> Line {
    Line::from_spans([
        Span::new(" ".repeat(indent), Role::Plain),
        Span::new(format!("└{}┘", "─".repeat(inner)), Role::Muted),
    ])
}

fn boxed(mut content: Line, indent: usize, inner: usize) -> Line {
    content = fitted(content, inner);
    let padding = inner.saturating_sub(line_width(&content));
    let mut line = Line::from_spans([
        Span::new(" ".repeat(indent), Role::Plain),
        Span::new("│", Role::Muted),
    ]);
    line.spans.extend(content.spans);
    line.spans.push(Span::new(" ".repeat(padding), Role::Plain));
    line.spans.push(Span::new("│", Role::Muted));
    line
}

fn row_label(text: impl Into<String>) -> Line {
    Line::from_spans([
        Span::new("  ", Role::Plain),
        Span::new(text, Role::Muted),
        Span::new(" ", Role::Plain),
    ])
}

fn entry_line(
    name: &str,
    kind: EntryKind,
    active: bool,
    badge: Option<Line>,
    inner: usize,
) -> Line {
    if inner == 0 {
        return Line::default();
    }
    let marker = if active { "▸ " } else { "  " };
    let marker_role = if active { Role::Accent } else { Role::Plain };
    let marker = fitted(Line::styled(marker, marker_role), inner);
    let marker_width = line_width(&marker);
    let mut room = inner.saturating_sub(marker_width);
    let badge = fitted(badge.unwrap_or_default(), room / 2);
    let badge_width = line_width(&badge);
    let gap_width = usize::from(badge_width > 0 && room > badge_width);
    room = room.saturating_sub(badge_width + gap_width);
    let label = match kind.is_directory() {
        true => format!("{name}/"),
        false => name.to_string(),
    };
    let label = fitted(
        Line::styled(label, if active { Role::Strong } else { Role::Plain }),
        room,
    );
    let label_width = line_width(&label);
    let mut line = Line::default();
    line.spans.extend(marker.spans);
    line.spans.extend(label.spans);
    line.spans.push(Span::new(
        " ".repeat(inner.saturating_sub(marker_width + label_width + badge_width)),
        Role::Plain,
    ));
    line.spans.extend(badge.spans);
    line
}

fn footer<L, V>(view: &V, context: &ViewContext<'_, L>, help: bool) -> Line
where
    V: ExplorerView<L> + ?Sized,
{
    if let Some((text, kind)) = context.prompt {
        let label = match kind {
            InputKind::Search => "find ",
            InputKind::Location => "path ",
        };
        return Line::from_spans([
            Span::new(label, Role::Muted),
            Span::new(text, Role::Strong),
            Span::new("▏", Role::Accent),
            Span::new("   ⏎ go   esc back", Role::Muted),
        ]);
    }

    if help {
        return Line::styled(
            "↑↓/jk move   pgup/pgdn page   →/l open   ←/h up   r refresh   esc close",
            Role::Muted,
        );
    }

    Line::from_spans([
        Span::new("↑↓ move   ⏎ ", Role::Muted),
        Span::new(view.accept_label(context), Role::Muted),
        Span::new("   / find   r refresh   ? help   esc cancel", Role::Muted),
    ])
}

fn elsewhere(offset: usize, shown: usize, total: usize) -> String {
    let above = offset;
    let below = total.saturating_sub(offset.saturating_add(shown));
    match (above, below) {
        (0, below) => format!("… {below} below"),
        (above, 0) => format!("… {above} above"),
        (above, below) => format!("… {above} above | {below} below"),
    }
}

fn paint(line: &Line, style: &Style, limit: usize) -> String {
    let line = fitted(line.clone(), limit);
    line.spans
        .into_iter()
        .map(|span| match span.role {
            Role::Plain => span.text,
            Role::Strong => style.bold(&span.text),
            Role::Muted => style.dim(&span.text),
            Role::Accent => style.teal(&span.text),
            Role::Success => style.green(&span.text),
            Role::Danger => style.red(&span.text),
        })
        .collect()
}

fn fitted(line: Line, limit: usize) -> Line {
    let mut sanitized = Line::default();
    for span in line.spans {
        sanitized
            .spans
            .push(Span::new(sanitize(&span.text), span.role));
    }
    if line_width(&sanitized) <= limit {
        return sanitized;
    }
    if limit == 0 {
        return Line::default();
    }

    let keep = limit.saturating_sub(1);
    let mut result = Line::default();
    let mut used = 0;
    let mut ellipsis_role = Role::Plain;
    'spans: for span in sanitized.spans {
        let mut text = String::new();
        ellipsis_role = span.role;
        for character in span.text.chars() {
            let width = UnicodeWidthChar::width(character).unwrap_or(0);
            if used + width > keep {
                if !text.is_empty() {
                    result.spans.push(Span::new(text, span.role));
                }
                break 'spans;
            }
            text.push(character);
            used += width;
        }
        if !text.is_empty() {
            result.spans.push(Span::new(text, span.role));
        }
    }
    result.spans.push(Span::new("…", ellipsis_role));
    result
}

fn sanitize(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() || is_format_control(character) {
            safe.extend(character.escape_default());
        } else {
            safe.push(character);
        }
    }
    safe
}

fn is_format_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200b}'
            | '\u{200e}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
    )
}

fn line_width(line: &Line) -> usize {
    line.spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.text.as_str()))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DirectoryStatus, Entry};

    fn directory(names: &[(&str, EntryKind)]) -> Directory<usize> {
        Directory {
            location: 1,
            parent: Some(0),
            label: "remote:~/projects".into(),
            entries: names
                .iter()
                .enumerate()
                .map(|(location, (name, kind))| Entry {
                    location,
                    name: (*name).into(),
                    kind: *kind,
                })
                .collect(),
            status: DirectoryStatus::Present,
        }
    }

    fn context<'a>(directory: &'a Directory<usize>, rows: &'a [usize]) -> RenderContext<'a, usize> {
        RenderContext {
            directory,
            rows,
            cursor: 0,
            offset: 0,
            prompt: None,
            error: None,
            selection: None,
            help: false,
        }
    }

    fn frame(directory: &Directory<usize>, rows: &[usize], size: Size) -> RenderedFrame {
        render(
            &context(directory, rows),
            &crate::DefaultView,
            &Style::plain(),
            size,
            RenderLimits::default(),
        )
    }

    #[test]
    fn every_line_fits_the_terminal_in_cells() {
        let directory = directory(&[
            ("東京の長い名前", EntryKind::Directory),
            ("notes", EntryKind::File),
        ]);
        let rendered = frame(
            &directory,
            &[0, 1],
            Size {
                width: 18,
                height: 8,
            },
        );

        assert!(
            rendered
                .lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= 18)
        );
        assert!(rendered.lines.iter().any(|line| line.contains('…')));
    }

    #[test]
    fn narrow_terminals_do_not_force_a_minimum_box_width() {
        let directory = directory(&[("a-very-long-name", EntryKind::File)]);

        for width in 1..12 {
            let rendered = frame(&directory, &[0], Size { width, height: 5 });
            assert!(
                rendered
                    .lines
                    .iter()
                    .all(|line| UnicodeWidthStr::width(line.as_str()) <= width)
            );
        }
    }

    #[test]
    fn short_terminals_never_receive_too_many_lines() {
        let directory = directory(&[("one", EntryKind::File), ("two", EntryKind::File)]);

        for height in 0..8 {
            let rendered = frame(&directory, &[0, 1], Size { width: 40, height });
            assert!(rendered.lines.len() <= height);
        }
    }

    #[test]
    fn terminal_controls_and_direction_overrides_are_visible_text() {
        let directory = directory(&[("safe\x1b[31m\n\u{202e}txt", EntryKind::File)]);
        let rendered = frame(
            &directory,
            &[0],
            Size {
                width: 70,
                height: 5,
            },
        );
        let joined = rendered.lines.join("\n");

        assert!(!joined.contains('\x1b'));
        assert!(!joined.contains('\u{202e}'));
        assert!(joined.contains("\\u{1b}"));
        assert!(joined.contains("\\n"));
        assert!(joined.contains("\\u{202e}"));
    }

    #[test]
    fn directory_names_receive_a_suffix_only_for_display() {
        let directory = directory(&[("src", EntryKind::Directory)]);
        let rendered = frame(
            &directory,
            &[0],
            Size {
                width: 40,
                height: 5,
            },
        );

        assert!(rendered.lines.iter().any(|line| line.contains("src/")));
        assert_eq!(directory.entries[0].name, "src");
    }

    #[test]
    fn prompt_and_inline_errors_are_rendered_safely() {
        let directory = directory(&[]);
        let mut context = context(&directory, &[]);
        context.prompt = Some(("bad\tname", InputKind::Search));
        context.error = Some("denied\x1b[2J");
        let rendered = render(
            &context,
            &crate::DefaultView,
            &Style::plain(),
            Size {
                width: 50,
                height: 7,
            },
            RenderLimits::default(),
        );
        let joined = rendered.lines.join("\n");

        assert!(joined.contains("bad\\tname"));
        assert!(joined.contains("denied\\u{1b}[2J"));
        assert!(!joined.contains('\x1b'));
    }

    #[test]
    fn missing_empty_and_no_match_states_are_distinct() {
        let mut directory = directory(&[]);
        let empty = frame(
            &directory,
            &[],
            Size {
                width: 50,
                height: 5,
            },
        )
        .lines
        .join("\n");
        directory.status = DirectoryStatus::Missing;
        let missing = frame(
            &directory,
            &[],
            Size {
                width: 50,
                height: 5,
            },
        )
        .lines
        .join("\n");
        directory.status = DirectoryStatus::Present;
        let mut context = context(&directory, &[]);
        context.prompt = Some(("xyz", InputKind::Search));
        let no_match = render(
            &context,
            &crate::DefaultView,
            &Style::plain(),
            Size {
                width: 50,
                height: 5,
            },
            RenderLimits::default(),
        )
        .lines
        .join("\n");

        assert!(empty.contains("(empty)"));
        assert!(missing.contains("(not found)"));
        assert!(no_match.contains("(no matches)"));
    }

    #[test]
    fn viewport_reserves_a_row_for_overflow_status() {
        let names: Vec<(String, EntryKind)> = (0..20)
            .map(|index| (format!("entry-{index}"), EntryKind::File))
            .collect();
        let directory = Directory {
            location: 1,
            parent: Some(0),
            label: "many".into(),
            entries: names
                .iter()
                .enumerate()
                .map(|(location, (name, kind))| Entry {
                    location,
                    name: name.clone(),
                    kind: *kind,
                })
                .collect(),
            status: DirectoryStatus::Present,
        };
        let rows: Vec<usize> = (0..20).collect();
        let rendered = frame(
            &directory,
            &rows,
            Size {
                width: 50,
                height: 8,
            },
        );

        assert_eq!(rendered.viewport_rows, 4);
        assert!(rendered.lines.iter().any(|line| line.contains("below")));
    }

    #[test]
    fn stale_offset_does_not_create_overflow_when_every_row_fits() {
        let entries: Vec<_> = (0..5)
            .map(|index| (format!("entry-{index}"), EntryKind::File))
            .collect();
        let borrowed: Vec<_> = entries
            .iter()
            .map(|(name, kind)| (name.as_str(), *kind))
            .collect();
        let directory = directory(&borrowed);
        let rows = [0, 1, 2, 3, 4];
        let mut context = context(&directory, &rows);
        context.offset = 1;
        let rendered = render(
            &context,
            &crate::DefaultView,
            &Style::plain(),
            Size {
                width: 40,
                height: 8,
            },
            RenderLimits::default(),
        );

        assert_eq!(rendered.viewport_rows, 5);
        assert!(!rendered.lines.iter().any(|line| line.contains("above")));
    }

    struct BadgedView;

    impl ExplorerView<usize> for BadgedView {
        fn badge(&self, _context: &ViewContext<'_, usize>, _entry: &Entry<usize>) -> Option<Line> {
            Some(Line::styled("mirror", Role::Success))
        }

        fn accept_label(&self, _context: &ViewContext<'_, usize>) -> String {
            "push here".into()
        }
    }

    #[test]
    fn badges_and_application_accept_labels_share_the_layout() {
        let directory = directory(&[("src", EntryKind::Directory)]);
        let rendered = render(
            &context(&directory, &[0]),
            &BadgedView,
            &Style::plain(),
            Size {
                width: 50,
                height: 5,
            },
            RenderLimits::default(),
        );
        let joined = rendered.lines.join("\n");

        assert!(joined.contains("mirror"));
        assert!(joined.contains("push here"));
    }

    #[test]
    fn help_replaces_the_regular_footer() {
        let directory = directory(&[("src", EntryKind::Directory)]);
        let mut context = context(&directory, &[0]);
        context.help = true;
        let rendered = render(
            &context,
            &crate::DefaultView,
            &Style::plain(),
            Size {
                width: 100,
                height: 5,
            },
            RenderLimits {
                max_width: 100,
                ..RenderLimits::default()
            },
        );
        let joined = rendered.lines.join("\n");

        assert!(joined.contains("pgup/pgdn"));
        assert!(joined.contains("r refresh"));
        assert!(!joined.contains("? help"));
    }

    #[test]
    fn colored_semantic_spans_do_not_change_cell_width() {
        let line = Line::from_spans([
            Span::new("green", Role::Success),
            Span::new(" danger", Role::Danger),
        ]);
        let painted = paint(&line, &Style::for_stdout_with_color(true), 20);

        assert_eq!(workstation::screen::width(&painted), 12);
        assert!(painted.contains("\x1b["));
    }
}
