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

    /// The character-by-character implementation `escape` replaced: the reference.
    fn escape_by_chars(s: &str) -> String {
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

    #[test]
    fn same_output_as_the_char_by_char_reference() {
        const ALPHABET: [char; 8] = ['a', '&', '<', '>', '"', '\'', 'ż', '😀'];
        let mut checked = 0;
        for len in 1..=3u32 {
            for mut n in 0..ALPHABET.len().pow(len) {
                let mut s = String::new();
                for _ in 0..len {
                    s.push(ALPHABET[n % ALPHABET.len()]);
                    n /= ALPHABET.len();
                }
                assert_eq!(escape(&s), escape_by_chars(&s), "{s:?}");
                checked += 1;
            }
        }
        assert_eq!(checked, 8 + 64 + 512);
        // Longer pseudo-random strings (a fixed linear congruential sequence) over a wider
        // alphabet, multi-byte characters next to the escaped ones.
        let wide = [
            'a', '&', '<', '>', '"', '\'', 'ż', '😀', ' ', '\n', 'é', '\u{202E}', ';',
        ];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        for _ in 0..200 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let len = (seed >> 58) as usize;
            let s: String = (0..len)
                .map(|i| wide[((seed >> (i % 50)) as usize + i) % wide.len()])
                .collect();
            assert_eq!(escape(&s), escape_by_chars(&s), "{s:?}");
        }
    }

    #[test]
    fn runs_around_entities_are_kept() {
        assert_eq!(escape("ż<😀>ł"), "ż&lt;😀&gt;ł");
        assert_eq!(escape("<<"), "&lt;&lt;");
        assert_eq!(escape("a&"), "a&amp;");
        assert_eq!(escape("&a"), "&amp;a");
    }
}
