//! HTML helpers for plugin routes.

/// Escape text for use in HTML element content or a quoted attribute value:
/// `&`, `<`, `>`, `"` and `'`.
///
/// It works on bytes and copies the runs between those five in one piece: a plugin call
/// is metered in guest instructions, and decoding every character costs several times
/// more for non-ASCII text. The five are ASCII, so they never occur inside a multi-byte
/// UTF-8 sequence and every cut falls on a character boundary.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = 0;
    for (i, b) in s.bytes().enumerate() {
        let entity = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&#39;",
            _ => continue,
        };
        if run < i {
            out.push_str(&s[run..i]);
        }
        out.push_str(entity);
        run = i + 1;
    }
    out.push_str(&s[run..]);
    out
}

#[cfg(test)]
mod tests {
    use super::escape;

    #[test]
    fn escapes_the_five_characters() {
        assert_eq!(
            escape(r#"<a href="x">'&"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;"
        );
    }

    #[test]
    fn ampersand_is_escaped_once() {
        assert_eq!(escape("&amp;"), "&amp;amp;");
    }

    #[test]
    fn plain_and_non_ascii_text_is_unchanged() {
        assert_eq!(escape(""), "");
        assert_eq!(escape("Zażółć gęślą jaźń"), "Zażółć gęślą jaźń");
    }

    #[test]
    fn runs_around_entities_are_kept() {
        assert_eq!(escape("ż<😀>ł"), "ż&lt;😀&gt;ł");
        assert_eq!(escape("<<"), "&lt;&lt;");
        assert_eq!(escape("a&"), "a&amp;");
        assert_eq!(escape("&a"), "&amp;a");
    }
}
