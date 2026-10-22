/// Detects the MIME type from content only; file names are never consulted.
pub fn sniff_mime(head: &[u8]) -> String {
    infer::get(head)
        .map(|t| t.mime_type().to_owned())
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

/// Raster formats the image pipeline decodes: JPEG, PNG, GIF, WebP.
pub fn is_raster_image(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    #[test]
    fn sniff_mime_ignores_names() {
        // The function has no name parameter: PNG bytes are PNG whatever they are called.
        assert_eq!(sniff_mime(PNG), "image/png");
        let html = b"<!doctype html><html><body>hi</body></html>";
        assert!(!is_raster_image(&sniff_mime(html)));
        assert_eq!(sniff_mime(b"plain text"), "application/octet-stream");
        assert!(is_raster_image("image/webp"));
        assert!(!is_raster_image("image/svg+xml"));
    }
}
