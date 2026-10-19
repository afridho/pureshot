use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum ExportFormat {
    #[default]
    #[serde(rename = "PNG")]
    Png,
    #[serde(rename = "JPEG")]
    Jpeg,
    #[serde(rename = "WebP")]
    WebP,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self { Self::Png => "png", Self::Jpeg => "jpg", Self::WebP => "webp" }
    }

    pub fn encode(self, image: &DynamicImage) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        match self {
            Self::Jpeg => {
                // JPEG has no alpha channel. Composite against white instead of
                // dropping alpha (which turns transparent canvas areas black).
                let rgba = image.to_rgba8();
                let rgb = image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
                    let pixel = rgba.get_pixel(x, y).0;
                    let alpha = u32::from(pixel[3]);
                    image::Rgb(std::array::from_fn(|channel| {
                        ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
                    }))
                });
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
                    .encode_image(&DynamicImage::ImageRgb8(rgb))
                    .map_err(|error| error.to_string())?;
            }
            Self::Png | Self::WebP => {
                let format = if self == Self::Png { ImageFormat::Png } else { ImageFormat::WebP };
                image.write_to(&mut Cursor::new(&mut bytes), format).map_err(|error| error.to_string())?;
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_real_formats_and_preserves_dimensions() {
        let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(3, 2, image::Rgba([25, 70, 120, 255])));
        for (format, expected, extension) in [
            (ExportFormat::Png, ImageFormat::Png, "png"),
            (ExportFormat::Jpeg, ImageFormat::Jpeg, "jpg"),
            (ExportFormat::WebP, ImageFormat::WebP, "webp"),
        ] {
            let bytes = format.encode(&source).unwrap();
            assert_eq!(image::guess_format(&bytes).unwrap(), expected);
            let decoded = image::load_from_memory(&bytes).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (3, 2));
            assert_eq!(format.extension(), extension);
            let json = serde_json::to_vec(&format).unwrap();
            assert_eq!(serde_json::from_slice::<ExportFormat>(&json).unwrap(), format);
        }
    }

    #[test]
    fn transparency_is_preserved_or_composited_on_white() {
        let source = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0])));
        for format in [ExportFormat::Png, ExportFormat::WebP] {
            let decoded = image::load_from_memory(&format.encode(&source).unwrap()).unwrap().to_rgba8();
            assert_eq!(decoded.get_pixel(0, 0).0[3], 0);
        }
        let decoded = image::load_from_memory(&ExportFormat::Jpeg.encode(&source).unwrap()).unwrap().to_rgb8();
        assert!(decoded.get_pixel(0, 0).0.iter().all(|channel| *channel >= 254));
        assert!(serde_json::from_str::<ExportFormat>("\"GIF\"").is_err());
    }
}
