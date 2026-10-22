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

/// True only for types that may be served `Content-Disposition: inline`: JPEG, PNG, GIF, WebP,
/// MP3/Ogg/WAV audio (`audio/wav`, plus `audio/x-wav` which is what `infer` reports for WAV), MP4/WebM video and PDF. Everything else (including `text/html`,
/// `text/xml` and anything unknown) must be served as `application/octet-stream` with
/// `Content-Disposition: attachment` and `X-Content-Type-Options: nosniff`.
/// Exact, case-sensitive match against the sniffed type.
pub fn inline_safe(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg"
            | "image/png"
            | "image/gif"
            | "image/webp"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/wav"
            | "audio/x-wav"
            | "video/mp4"
            | "video/webm"
            | "application/pdf"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    #[test]
    fn inline_safe_is_an_allowlist() {
        for ok in [
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "audio/mpeg",
            "audio/ogg",
            "audio/wav",
            "audio/x-wav",
            "video/mp4",
            "video/webm",
            "application/pdf",
        ] {
            assert!(inline_safe(ok), "{ok}");
        }
        for bad in [
            "text/html",
            "text/xml",
            "application/xml",
            "image/svg+xml",
            "application/xhtml+xml",
            "application/javascript",
            "application/octet-stream",
            "image/PNG",
            "image/png; charset=x",
            "",
        ] {
            assert!(!inline_safe(bad), "{bad}");
        }
        // What sniffing returns for markup is not inline-safe.
        assert!(!inline_safe(&sniff_mime(
            b"<!doctype html><html><body>hi</body></html>"
        )));
        assert!(!inline_safe(&sniff_mime(
            b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>"
        )));
    }

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
