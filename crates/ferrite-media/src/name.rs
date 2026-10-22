//! The file name used in `/_media/<hash>/<name>` URLs.

/// The routable form of a stored file name: `/`, `\` and control characters become
/// `_` (a `/` would split the URL segment, `%2F` is refused by the router, and `\` or
/// controls are refused outright), and an empty, `.` or `..` name becomes `file`
/// (clients normalise those away and would drop the hash segment). Used by
/// [`MediaStore::public_url`](crate::MediaStore::public_url) and by the HTTP layer to
/// decide the canonical name, so the two always agree.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    match cleaned.as_str() {
        "" | "." | ".." => "file".to_owned(),
        _ => cleaned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for (raw, want) in [
            ("cat.png", "cat.png"),
            ("my photo é#1?.png", "my photo é#1?.png"),
            ("a/../b", "a_.._b"),
            ("a\\b.png", "a_b.png"),
            ("x\u{0}y\u{1f}z\u{7f}.txt", "x_y_z_.txt"),
            ("", "file"),
            (".", "file"),
            ("..", "file"),
            ("...", "..."),
            ("/", "_"),
        ] {
            assert_eq!(sanitize_filename(raw), want, "{raw:?}");
        }
    }
}
