//! HTML helpers for plugin routes.

/// Escape text for use in HTML element content or a quoted attribute value:
/// `&`, `<`, `>`, `"` and `'`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
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
}
