use super::*;

mod graph;

const CARD_HEIGHT: u16 = 4;
const SUBJECT_HEIGHT: u16 = 2;
const HORIZONTAL_MARGIN: u16 = 1;
const CARD_PADDING: u16 = 1;

pub(super) fn visible_items(height: u16) -> usize {
    (usize::from(height).saturating_add(1) / usize::from(CARD_HEIGHT)).max(1)
}

#[derive(Default)]
pub(super) struct Layout {
    width: u16,
    graph_width: u16,
    subjects: Vec<Vec<Line<'static>>>,
    graph: graph::Graph,
}

impl Layout {
    fn prepare(&mut self, items: &[NavItem], width: u16) {
        if self.width == width && self.subjects.len() == items.len() {
            return;
        }
        self.width = width;
        self.graph = graph::Graph::new(items);
        self.graph_width = self.graph.width((width.saturating_sub(24) / 3).clamp(1, 6));
        let text_width =
            width.saturating_sub(self.card_offset() + HORIZONTAL_MARGIN + CARD_PADDING * 2);
        self.subjects = items
            .iter()
            .map(|item| subject::list_lines(&item.label, text_width))
            .collect();
    }

    fn card_offset(&self) -> u16 {
        self.graph_width + HORIZONTAL_MARGIN * 2
    }
}

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    state: &mut ViewState,
    items: &[NavItem],
    layout: &mut Layout,
) -> navigator::Navigation {
    layout.prepare(items, area.width);
    let visible = visible_items(area.height);
    let maximum = items.len().saturating_sub(visible);
    state.selected = state.selected.min(items.len().saturating_sub(1));
    state.offset = state.offset.min(maximum);
    if !state.nav_manual_scroll {
        state.offset = state.offset.min(state.selected);
        if state.selected >= state.offset.saturating_add(visible) {
            state.offset = state.selected + 1 - visible;
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64);
    let mut navigation = navigator::Navigation {
        scroll_max: maximum,
        ..navigator::Navigation::default()
    };
    let mut y = area.y;
    for (index, subject) in layout.subjects.iter().enumerate().skip(state.offset) {
        if y >= area.bottom() {
            break;
        }
        let item = &items[index];
        let height = CARD_HEIGHT.min(area.bottom() - y);
        navigation
            .items
            .push((index, Rect::new(area.x, y, area.width, height)));
        let selected = index == state.selected && !item.disabled;
        let appearance = navigator::row_style(
            selected,
            selected && state.focus == Focus::Navigator,
            state.nav_hovered == Some(index),
        );
        let graph = Rect::new(area.x + HORIZONTAL_MARGIN, y, layout.graph_width, height);
        layout.graph.render(
            buffer,
            graph,
            index,
            selected.then(|| subject::list_label_color(&item.label)),
        );
        let offset = layout.card_offset();
        let content = Rect::new(
            area.x + offset,
            y,
            area.width.saturating_sub(offset + HORIZONTAL_MARGIN),
            (CARD_HEIGHT - 1).min(height),
        );
        buffer.set_style(content, appearance);
        let text = Rect::new(
            content.x + CARD_PADDING,
            y,
            content.width.saturating_sub(CARD_PADDING * 2),
            content.height,
        );
        for (line_index, line) in subject.iter().enumerate().take(usize::from(text.height)) {
            let mut line = line.clone();
            for span in &mut line.spans {
                span.style = span
                    .style
                    .bg(appearance.bg.unwrap_or(Color::Reset))
                    .add_modifier(appearance.add_modifier);
                if selected {
                    span.style = span.style.add_modifier(Modifier::BOLD);
                }
            }
            line.render(
                Rect::new(text.x, y + line_index as u16, text.width, 1),
                buffer,
            );
        }
        let metadata_y = y + SUBJECT_HEIGHT;
        if metadata_y < content.bottom()
            && let Some(commit) = &item.commit
        {
            metadata(
                buffer,
                Rect::new(text.x, metadata_y, text.width, 1),
                commit,
                &[],
                now,
                appearance,
            );
        }
        y += height;
    }
    navigation
}

pub(super) fn metadata(
    buffer: &mut Buffer,
    area: Rect,
    commit: &CommitSummary,
    aside: &[Span<'static>],
    now: i64,
    appearance: Style,
) {
    let muted = appearance.fg(foreground(Role::Muted));
    let oid: String = commit.oid.chars().take(7).collect();
    let mut right = Line::default();
    let aside_width = aside.iter().map(Span::width).sum::<usize>() + aside.len() * 2;
    if usize::from(area.width) >= oid.width() + aside_width + 16 {
        let background = appearance.bg.unwrap_or(Color::Reset);
        for span in aside {
            right
                .spans
                .push(span.clone().style(span.style.bg(background)));
            right.spans.push(Span::styled("  ", muted));
        }
    }
    right.spans.push(Span::styled(
        crate::commit::compact_relative_time(commit.timestamp, now),
        muted,
    ));
    let right_width = (right.width() as u16).min(area.width);
    right.render(
        Rect::new(area.right() - right_width, area.y, right_width, 1),
        buffer,
    );
    let left_width = area.width.saturating_sub(right_width + 2);
    let hash_width = oid.width().min(usize::from(left_width)) as u16;
    put(buffer, area.x, area.y, hash_width, &oid, muted);
    let author_width = left_width.saturating_sub(hash_width + 2);
    if author_width >= 3 {
        put(
            buffer,
            area.x + hash_width + 2,
            area.y,
            author_width,
            &elide_end(&commit.author, usize::from(author_width)),
            muted,
        );
    }
}
