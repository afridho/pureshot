//! Rounded-corner processing for captures selected by the Linux compositor.

/// Radius is in source-image pixels, just like the Windows selector, not GTK logical pixels.
fn effective_radius(width: u32, height: u32, radius: u32) -> f64 {
    (radius as f64)
        .min(width as f64 / 2.0)
        .min(height as f64 / 2.0)
}

/// Apply antialiased coverage to straight RGBA (never premultiply PNG RGB).
/// Only visit corner squares; the screenshot's interior stays byte-for-byte intact.
pub(super) fn round_capture(image: image::DynamicImage, radius: u32) -> image::DynamicImage {
    let (w, h) = (image.width(), image.height());
    let r = effective_radius(w, h, radius);
    if r == 0.0 {
        return image;
    }
    let mut rgba = image.to_rgba8();
    let band = r.ceil() as u32;
    for y in 0..band {
        for x in 0..band {
            let distance = (r - x as f64 - 0.5).hypot(r - y as f64 - 0.5) - r;
            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            let corners = [
                (x, y),
                (w - 1 - x, y),
                (x, h - 1 - y),
                (w - 1 - x, h - 1 - y),
            ];
            for (i, &(px, py)) in corners.iter().enumerate() {
                // Odd-sized, clamped crops share a center row/column: mask it only once.
                if corners[..i].contains(&(px, py)) {
                    continue;
                }
                let pixel = rgba.get_pixel_mut(px, py);
                pixel.0[3] = (pixel.0[3] as f64 * coverage).round() as u8;
            }
        }
    }
    image::DynamicImage::ImageRgba8(rgba)
}

#[cfg(test)]
mod tests {
    use super::{effective_radius, round_capture};

    fn solid(w: u32, h: u32, alpha: u8) -> image::DynamicImage {
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([37, 91, 153, alpha]),
        ))
    }

    #[test]
    fn zero_radius_is_unchanged() {
        let original = solid(20, 12, 123);
        assert_eq!(
            round_capture(original.clone(), 0).to_rgba8(),
            original.to_rgba8()
        );
    }

    #[test]
    fn png_has_transparent_symmetric_antialiased_corners_and_original_rgb() {
        let rounded = round_capture(solid(32, 20, 200), 8);
        let mut png = std::io::Cursor::new(Vec::new());
        rounded.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let decoded = image::load_from_memory(png.get_ref()).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (32, 20));
        assert_eq!(decoded.get_pixel(0, 0).0[3], 0);
        assert_eq!(decoded.get_pixel(16, 10).0, [37, 91, 153, 200]);
        assert!(decoded.pixels().any(|p| p.0[3] > 0 && p.0[3] < 200));
        for y in 0..20 {
            for x in 0..32 {
                let p = decoded.get_pixel(x, y);
                assert_eq!(&p.0[..3], &[37, 91, 153]);
                assert_eq!(p, decoded.get_pixel(31 - x, y));
                assert_eq!(p, decoded.get_pixel(x, 19 - y));
            }
        }
    }

    #[test]
    fn oversized_radius_clamps_and_tiny_crops_are_safe() {
        assert_eq!(effective_radius(9, 5, u32::MAX), 2.5);
        assert_eq!(
            round_capture(solid(9, 5, 255), u32::MAX).to_rgba8(),
            round_capture(solid(9, 5, 255), 3).to_rgba8()
        );
        assert_eq!(
            round_capture(solid(1, 1, 123), 13)
                .to_rgba8()
                .get_pixel(0, 0)
                .0[3],
            123
        );
        let odd = round_capture(solid(5, 5, 123), 99).to_rgba8();
        assert_eq!(odd.get_pixel(2, 0).0[3], 123);
        assert_eq!(odd.get_pixel(0, 2).0[3], 123);
        assert_eq!(round_capture(solid(0, 0, 255), 13).width(), 0);
    }
}
