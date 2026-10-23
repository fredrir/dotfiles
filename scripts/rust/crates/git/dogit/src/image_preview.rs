use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use image::{DynamicImage, ImageFormat, ImageReader};

use crate::diff::ExactDiff;

pub(crate) struct Preview {
    pub image: Result<DynamicImage, String>,
    pub label: String,
}

impl Preview {
    pub(crate) fn load(diff: &ExactDiff, cancel: &AtomicBool) -> Option<Self> {
        let extension = ImageFormat::from_path(&diff.path).ok();
        if diff.path.as_os_str().is_empty() || (!diff.binary && extension.is_none()) {
            return None;
        }
        let (bytes, deleted) = match diff.preview_bytes(Some(cancel)) {
            Ok(source) => source,
            Err(error) => {
                return extension.map(|_| Self {
                    image: Err(error),
                    label: "Image preview unavailable".into(),
                });
            }
        };
        let mut preview = Self::decode(&bytes, extension)?;
        if deleted {
            preview.label.push_str(" Deleted previous image");
        }
        Some(preview)
    }

    pub(crate) fn from_bytes(path: &Path, bytes: &[u8]) -> Option<Self> {
        let extension = ImageFormat::from_path(path).ok();
        if extension.is_none() && !bytes.contains(&0) {
            return None;
        }
        Self::decode(bytes, extension)
    }

    fn decode(bytes: &[u8], extension: Option<ImageFormat>) -> Option<Self> {
        let format = image::guess_format(bytes).ok().or(extension)?;
        let image = decode(bytes, format);
        let mut label = format!("{format:?}").to_uppercase();
        if let Ok(image) = &image {
            label.push_str(&format!(" {} × {}", image.width(), image.height()));
        }
        let image = image.map(|image| {
            if image.width() > 2048 || image.height() > 2048 {
                image.resize(2048, 2048, image::imageops::FilterType::Triangle)
            } else {
                image
            }
        });
        Some(Self { image, label })
    }

    pub(crate) fn cache_bytes(&self) -> usize {
        self.image
            .as_ref()
            .map_or(0, |image| image.as_bytes().len())
            + self.label.len()
    }
}

fn decode(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|error| format!("Cannot preview image: {error}"))
}
