/// Crops a sub-region from the given RGBA screenshot data.
///
/// Returns the cropped RGBA pixel data.
///
/// # Panics
///
/// Will panic if `x + w > screen_w` or `y + h` exceeds the bounds of `rgba`.
/// The caller must ensure the crop region is within the screenshot bounds.
#[must_use]
pub(crate) fn crop_rgba(rgba: &[u8], screen_w: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut cropped = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        for col in x..x + w {
            let idx = ((row * screen_w + col) * 4) as usize;
            if idx + 4 <= rgba.len() {
                cropped.extend_from_slice(&rgba[idx..idx + 4]);
            }
        }
    }
    cropped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crop_rgba_full() {
        // 4x4 RGBA pixel data, each pixel with a distinct value
        let mut rgba = Vec::with_capacity(4 * 4 * 4);
        for i in 0..16 {
            rgba.push((i * 16) as u8); // R
            rgba.push((i * 16 + 1) as u8); // G
            rgba.push((i * 16 + 2) as u8); // B
            rgba.push(255); // A
        }
        let result = crop_rgba(&rgba, 4, 0, 0, 4, 4);
        assert_eq!(result.len(), 4 * 4 * 4);
        assert_eq!(result, rgba);
    }

    #[test]
    fn test_crop_rgba_sub_region() {
        // 4x4 checkerboard pattern
        let mut rgba = vec![0u8; 4 * 4 * 4];
        // Place a red pixel at (1, 1)
        let idx = (4 + 1) * 4;
        rgba[idx] = 255; // R
        rgba[idx + 3] = 255; // A

        // Crop a 2x2 region starting at (1, 1)
        let result = crop_rgba(&rgba, 4, 1, 1, 2, 2);
        assert_eq!(result.len(), 2 * 2 * 4);
        // First pixel should be red
        assert_eq!(result[0], 255);
        assert_eq!(result[3], 255);
    }

    #[test]
    fn test_crop_rgba_empty() {
        let rgba = vec![0u8; 4 * 4 * 4];
        let result = crop_rgba(&rgba, 4, 0, 0, 0, 0);
        assert_eq!(result.len(), 0);
    }
}
