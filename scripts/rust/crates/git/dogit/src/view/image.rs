use ratatui::buffer::CellDiffOption;
use ratatui_image::protocol::Protocol;

use super::*;

pub struct ImageView<'a> {
    pub label: &'a str,
    pub protocol: Option<&'a Protocol>,
    pub error: Option<&'a str>,
}

pub(crate) fn image_area(canvas: Rect) -> Rect {
    Rect::new(
        canvas.x + canvas.width.min(2),
        canvas.y + canvas.height.min(2),
        canvas.width.saturating_sub(4),
        canvas.height.saturating_sub(3),
    )
}

pub(super) fn render(buffer: &mut Buffer, canvas: Rect, image: &ImageView<'_>, visible: bool) {
    put(
        buffer,
        canvas.x + 2,
        canvas.y,
        canvas.width.saturating_sub(4),
        image.label,
        style(Role::Muted),
    );
    let area = image_area(canvas);
    if area.is_empty() || !visible {
        return;
    }
    if let Some(error) = image.error {
        Paragraph::new(safe_text(error))
            .style(style(Role::Muted))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(area, buffer);
    } else if let Some(protocol) = image.protocol {
        let size = protocol.area();
        // Wait for the worker to resize rather than clipping a native terminal image.
        if size.width > area.width || size.height > area.height {
            return;
        }
        let area = Rect::new(
            area.x + (area.width - size.width) / 2,
            area.y + (area.height - size.height) / 2,
            size.width,
            size.height,
        );
        ratatui_image::Image::new(protocol).render(area, buffer);
        for y in area.top()..area.bottom() {
            if let Some(cell) = buffer.cell_mut((area.x, y))
                && cell.symbol().contains('\u{1b}')
            {
                cell.set_diff_option(CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN));
            }
        }
    } else {
        put(
            buffer,
            area.x,
            area.y,
            area.width,
            "Loading image…",
            style(Role::Muted),
        );
    }
}
