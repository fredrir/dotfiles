use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use image::{DynamicImage, RgbaImage};
use ratatui::{
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
    style::Color,
    widgets::Widget,
};
use ratatui_image::{
    Resize,
    picker::{Picker, ProtocolType},
    protocol::Protocol,
};
use sha2::{Digest, Sha256};

pub const COLUMNS: u16 = 14;
pub const ROWS: u16 = 7;
const SOURCE_SIZE: u32 = 256;
const BYTE_LIMIT: usize = 2 * 1024 * 1024;

pub enum Avatar {
    Native(Protocol),
    Blocks(Buffer),
}

impl Avatar {
    pub(crate) fn encode(image: RgbaImage, picker: &Picker) -> Option<Self> {
        let image = circle(image);
        if picker.protocol_type() == ProtocolType::Halfblocks {
            // Text terminals cannot display pixel graphics. Preserve the circular silhouette
            // and the terminal background, including cells containing only half the photo.
            let image = image::imageops::resize(
                &image,
                u32::from(COLUMNS),
                u32::from(ROWS) * 2,
                image::imageops::FilterType::Lanczos3,
            );
            let mut buffer = Buffer::empty(Rect::new(0, 0, COLUMNS, ROWS));
            for y in 0..ROWS {
                for x in 0..COLUMNS {
                    let top = image[(u32::from(x), u32::from(y) * 2)];
                    let bottom = image[(u32::from(x), u32::from(y) * 2 + 1)];
                    let color = |pixel: image::Rgba<u8>| Color::Rgb(pixel[0], pixel[1], pixel[2]);
                    let cell = &mut buffer[(x, y)];
                    match (top[3] >= 128, bottom[3] >= 128) {
                        (true, true) => {
                            cell.set_char('▀').set_fg(color(top)).set_bg(color(bottom));
                        }
                        (true, false) => {
                            cell.set_char('▀').set_fg(color(top)).set_bg(Color::Reset);
                        }
                        (false, true) => {
                            cell.set_char('▄')
                                .set_fg(color(bottom))
                                .set_bg(Color::Reset);
                        }
                        (false, false) => {}
                    }
                }
            }
            return Some(Self::Blocks(buffer));
        }
        picker
            .new_protocol(
                DynamicImage::ImageRgba8(image),
                Rect::new(0, 0, COLUMNS, ROWS),
                Resize::Fit(Some(image::imageops::FilterType::Lanczos3)),
            )
            .ok()
            .map(Self::Native)
    }

    pub fn render(&self, area: Rect, buffer: &mut Buffer) {
        match self {
            Self::Native(protocol) => {
                ratatui_image::Image::new(protocol).render(area, buffer);
                // The MSRV-compatible image encoder predates Ratatui's explicit escape width.
                // Its image payload occupies one buffer cell, not thousands of text columns.
                for y in area.top()..area.bottom() {
                    if let Some(cell) = buffer.cell_mut((area.x, y))
                        && cell.symbol().contains('\u{1b}')
                    {
                        cell.set_diff_option(CellDiffOption::ForcedWidth(
                            std::num::NonZeroU16::MIN,
                        ));
                    }
                }
            }
            Self::Blocks(image) => {
                for y in 0..area.height.min(ROWS) {
                    for x in 0..area.width.min(COLUMNS) {
                        if let Some(cell) = buffer.cell_mut((area.x + x, area.y + y)) {
                            *cell = image[(x, y)].clone();
                        }
                    }
                }
            }
        }
    }
}

// Obtain cell dimensions without a terminal query/response timeout or stealing input events.
#[allow(deprecated)]
pub(crate) fn picker() -> Picker {
    let font = crossterm::terminal::window_size()
        .ok()
        .and_then(|size| {
            if size.columns == 0 || size.rows == 0 {
                return None;
            }
            let (width, height) = (size.width / size.columns, size.height / size.rows);
            (width > 0 && height > 0).then_some((width, height))
        })
        .unwrap_or((10, 20));
    let mut picker = Picker::from_fontsize(font);
    let term = std::env::var("TERM").unwrap_or_default();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let protocol = if program == "WezTerm"
        || program == "iTerm.app"
        || std::env::var_os("WEZTERM_EXECUTABLE").is_some()
    {
        ProtocolType::Iterm2
    } else if term.contains("kitty") || program == "ghostty" || term.contains("ghostty") {
        ProtocolType::Kitty
    } else {
        // The compatible SIXEL encoder discards alpha, so keep the circular text fallback.
        ProtocolType::Halfblocks
    };
    picker.set_protocol_type(protocol);
    picker
}

pub(crate) fn load(url: &str, picker: &Picker, cancel: &AtomicBool) -> Option<Avatar> {
    load_cached(
        url,
        picker,
        cancel,
        super::github::cache_directory().as_deref(),
        |url, cancel| super::github::fetch(url, BYTE_LIMIT, cancel),
    )
}

fn load_cached(
    url: &str,
    picker: &Picker,
    cancel: &AtomicBool,
    directory: Option<&Path>,
    fetch: impl FnOnce(&str, &AtomicBool) -> Option<Vec<u8>>,
) -> Option<Avatar> {
    if !super::github::valid_avatar_url(url) {
        return None;
    }
    let path = directory.map(|base| base.join(format!("avatar-{:x}.png", Sha256::digest(url))));
    let cached = path.as_ref().and_then(|path| {
        let file = std::fs::File::open(path).ok()?;
        if file.metadata().ok()?.modified().ok()?.elapsed().ok()?
            > Duration::from_secs(7 * 24 * 60 * 60)
        {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(BYTE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        decode(&bytes)
    });
    let image = if let Some(image) = cached {
        image
    } else {
        // Ask the CDN for a bounded, high-resolution thumbnail once per author.
        let url = format!("{}?s={SOURCE_SIZE}", url.split('?').next()?);
        let image = decode(&fetch(&url, cancel)?)?;
        if let Some(path) = path {
            let mut encoded = Cursor::new(Vec::new());
            if image
                .write_to(&mut encoded, image::ImageFormat::Png)
                .is_ok()
            {
                super::github::persist(&path, encoded.get_ref());
            }
        }
        image
    };
    Avatar::encode(image, picker)
}

fn decode(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() > BYTE_LIMIT {
        return None;
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    Some(
        reader
            .decode()
            .ok()?
            .resize_to_fill(
                SOURCE_SIZE,
                SOURCE_SIZE,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8(),
    )
}

fn circle(mut image: RgbaImage) -> RgbaImage {
    let radius = image.width().min(image.height()) as f32 / 2.0;
    let (cx, cy) = (image.width() as f32 / 2.0, image.height() as f32 / 2.0);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
        let coverage = (radius - distance).clamp(0.0, 1.0);
        pixel[3] = (f32::from(pixel[3]) * coverage).round() as u8;
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circular_mask_preserves_resolution_and_transparent_corners() {
        let photo = RgbaImage::from_pixel(256, 256, image::Rgba([20, 100, 200, 255]));
        let photo = circle(photo);
        assert_eq!(photo.dimensions(), (256, 256));
        for (x, y) in [(0, 0), (255, 0), (0, 255), (255, 255)] {
            assert_eq!(photo[(x, y)][3], 0);
        }
        assert_eq!(photo[(128, 128)][3], 255);
        assert!(photo.pixels().any(|pixel| pixel[3] > 0 && pixel[3] < 255));
    }

    #[test]
    fn text_fallback_keeps_corners_clear_and_circular_edges() {
        let image = RgbaImage::from_pixel(256, 256, image::Rgba([20, 100, 200, 255]));
        let avatar = Avatar::encode(image, &Picker::halfblocks()).unwrap();
        let area = Rect::new(0, 0, COLUMNS, ROWS);
        let mut buffer = Buffer::empty(area);
        avatar.render(area, &mut buffer);
        for (x, y) in [
            (0, 0),
            (COLUMNS - 1, 0),
            (0, ROWS - 1),
            (COLUMNS - 1, ROWS - 1),
        ] {
            assert_eq!(buffer[(x, y)].symbol(), " ");
            assert_eq!(buffer[(x, y)].bg, Color::Reset);
        }
        assert_eq!(buffer[(COLUMNS / 2, ROWS / 2)].fg, Color::Rgb(20, 100, 200));
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.symbol() == "▄" && cell.bg == Color::Reset)
        );
    }

    #[test]
    fn native_images_do_not_swallow_surrounding_text_on_redraw() {
        use ratatui::{backend::TestBackend, widgets::Paragraph};
        for protocol in [ProtocolType::Iterm2, ProtocolType::Kitty] {
            let mut picker = Picker::halfblocks();
            picker.set_protocol_type(protocol);
            let avatar = Avatar::encode(
                RgbaImage::from_pixel(256, 256, image::Rgba([20, 100, 200, 255])),
                &picker,
            )
            .unwrap();
            let mut terminal = ratatui::Terminal::new(TestBackend::new(50, 15)).unwrap();
            for caption in ["First commit", "Second commit", "First commit"] {
                terminal
                    .draw(|frame| {
                        frame.render_widget(Paragraph::new(caption), Rect::new(1, 4, 20, 1));
                        frame.render_widget(Paragraph::new("Diff below"), Rect::new(1, 12, 20, 1));
                        avatar.render(Rect::new(25, 2, COLUMNS, ROWS), frame.buffer_mut());
                    })
                    .unwrap();
                let text = |y| {
                    (1..21)
                        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                        .collect::<String>()
                };
                assert_eq!(text(4).trim(), caption);
                assert_eq!(text(12).trim(), "Diff below");
            }
        }
    }

    #[test]
    fn disk_cache_avoids_download_on_next_session() {
        let temp = tempfile::tempdir().unwrap();
        let url = "https://avatars.githubusercontent.com/u/1";
        let cancel = AtomicBool::new(false);
        let mut encoded = Cursor::new(Vec::new());
        RgbaImage::from_pixel(256, 256, image::Rgba([20, 100, 200, 255]))
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let picker = Picker::halfblocks();
        assert!(
            load_cached(url, &picker, &cancel, Some(temp.path()), |_, _| Some(
                encoded.into_inner()
            ))
            .is_some()
        );
        assert!(
            load_cached(url, &picker, &cancel, Some(temp.path()), |_, _| panic!(
                "warm avatar downloaded again"
            ))
            .is_some()
        );
    }
}
