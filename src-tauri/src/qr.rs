//! Offline QR payload decoding. Never interpret or open the decoded content.
pub fn decode(image: &image::GrayImage) -> Result<Option<String>, String> {
    let mut decoder = quircs::Quirc::default();
    let mut payloads = Vec::new();
    for code in decoder.identify(image.width() as usize, image.height() as usize, image.as_raw()) {
        // A QR-like shape or damaged code must not prevent the OCR fallback.
        let Ok(code) = code else { continue };
        let Ok(data) = code.decode() else { continue };
        let text = String::from_utf8(data.payload)
            .map_err(|_| "QR code contains non-UTF-8 data and cannot be copied as text.")?;
        // Preserve payload whitespace, and avoid repeating identical codes.
        if !text.is_empty() && !payloads.contains(&text) { payloads.push(text); }
    }
    Ok(if payloads.is_empty() { None } else { Some(payloads.join("\n")) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_image_has_no_qr() {
        let image = image::GrayImage::from_pixel(128, 128, image::Luma([255]));
        assert_eq!(decode(&image).unwrap(), None);
    }

    #[test]
    fn decodes_exact_payload() {
        let image = image::load_from_memory(include_bytes!("../tests/fixtures/qr-text.png")).unwrap().into_luma8();
        assert_eq!(decode(&image).unwrap().as_deref(), Some("  https://example.com/qr?x=1&y=2\nHalo Indonesia!  "));
    }

    #[test]
    fn deduplicates_and_handles_multiple_codes() {
        let first = image::load_from_memory(include_bytes!("../tests/fixtures/qr-text.png")).unwrap().into_luma8();
        let second = image::load_from_memory(include_bytes!("../tests/fixtures/qr-wifi.png")).unwrap().into_luma8();
        let mut canvas = image::GrayImage::from_pixel(first.width() * 3, first.height(), image::Luma([255]));
        image::imageops::replace(&mut canvas, &first, 0, 0);
        image::imageops::replace(&mut canvas, &first, first.width() as i64, 0);
        image::imageops::replace(&mut canvas, &second, (first.width() * 2) as i64, 0);
        let result = decode(&canvas).unwrap().unwrap();
        assert_eq!(result.matches("https://example.com").count(), 1);
        assert!(result.contains("WIFI:T:WPA;S:PureShot Test;P:local-only;;"));
    }

    #[test]
    fn decodes_rotated_qr() {
        let image = image::load_from_memory(include_bytes!("../tests/fixtures/qr-text.png")).unwrap().into_luma8();
        assert_eq!(decode(&image::imageops::rotate90(&image)).unwrap(), decode(&image).unwrap());
    }
}
