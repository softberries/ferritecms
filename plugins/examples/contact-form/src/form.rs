//! The posted form: the fields, their cleaning and their limits (plan P45).

/// Most body bytes read. The largest valid form, every character a four-byte one sent
/// percent-encoded, is about 68 KB; the host passes up to 1 MiB, and decoding that much
/// would only cost fuel.
pub const MAX_BODY_BYTES: usize = 96 * 1024;

pub const NAME_MAX: usize = 100;
pub const EMAIL_MIN: usize = 3;
pub const EMAIL_MAX: usize = 254;
pub const SUBJECT_MAX: usize = 150;
pub const MESSAGE_MIN: usize = 10;
pub const MESSAGE_MAX: usize = 5000;
/// Longest `return` path, in bytes.
pub const RETURN_MAX: usize = 512;

/// What was posted, cleaned. The limits are checked by [`Submission::problems`].
#[derive(Debug, Default)]
pub struct Submission {
    pub name: String,
    pub email: String,
    pub subject: String,
    pub message: String,
    /// The honeypot `website` was filled in: a bot.
    pub honeypot: bool,
    /// `return`, when it is a path on this site ([`safe_return`]).
    pub back: Option<String>,
}

impl Submission {
    /// The fields from urlencoded `pairs`; the first value of a repeated key counts and
    /// unknown keys are ignored.
    pub fn from_pairs(pairs: &[(String, String)]) -> Self {
        let first = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| k == key)
                .map_or("", |(_, v)| v.as_str())
        };
        Self {
            name: single_line(first("name")),
            email: without_breaks(first("email")).trim().to_owned(),
            subject: single_line(first("subject")),
            message: message_text(first("message")),
            honeypot: !first("website").trim().is_empty(),
            back: safe_return(first("return")).map(str::to_owned),
        }
    }

    /// What is wrong, one sentence per problem naming the field; empty when the form is
    /// valid.
    pub fn problems(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let name = self.name.chars().count();
        if name == 0 {
            out.push("Name is required.");
        } else if name > NAME_MAX {
            out.push("Name must be at most 100 characters.");
        }
        if self.email.is_empty() {
            out.push("E-mail is required.");
        } else if !valid_email(&self.email) {
            out.push("E-mail must be a valid address.");
        }
        if self.subject.chars().count() > SUBJECT_MAX {
            out.push("Subject must be at most 150 characters.");
        }
        let message = self.message.chars().count();
        if message == 0 {
            out.push("Message is required.");
        } else if message < MESSAGE_MIN {
            out.push("Message must be at least 10 characters.");
        } else if message > MESSAGE_MAX {
            out.push("Message must be at most 5000 characters.");
        }
        out
    }
}

/// True when `content_type` is `application/x-www-form-urlencoded`, in any case, with or
/// without parameters such as `; charset=UTF-8`.
pub fn is_urlencoded(content_type: &str) -> bool {
    content_type.split(';').next().is_some_and(|t| {
        t.trim()
            .eq_ignore_ascii_case("application/x-www-form-urlencoded")
    })
}

/// `s` without CR and LF.
fn without_breaks(s: &str) -> String {
    s.chars().filter(|&c| c != '\r' && c != '\n').collect()
}

/// A one-line field: CR, LF and format characters ([`is_format`]) removed, any other control
/// character or Unicode line separator made a space, then trimmed.
fn single_line(s: &str) -> String {
    let line: String = s
        .chars()
        .filter(|&c| c != '\r' && c != '\n' && !is_format(c))
        .map(|c| if is_break_like(c) { ' ' } else { c })
        .collect();
    line.trim().to_owned()
}

/// A Unicode format character (General Category Cf, Unicode 16): bidirectional overrides
/// and isolates (U+202A..U+202E, U+2066..U+2069), zero-width characters, the soft hyphen,
/// tags and the like. Invisible, they can make a name or a subject display as something
/// else in a mail client (U+202E reverses what follows), so one-line fields drop them. The
/// message keeps them: it is shown escaped and never goes into a header.
pub fn is_format(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// The message keeps its line breaks, as LF (CR LF and a lone CR become LF), and its tabs;
/// other control characters are dropped. Then trimmed.
fn message_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.trim().to_owned()
}

/// A control character or a Unicode line or paragraph separator.
pub fn is_break_like(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}'
}

/// 3 to 254 characters, exactly one `@` with something on each side, and no whitespace or
/// control character.
fn valid_email(s: &str) -> bool {
    let len = s.chars().count();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    (EMAIL_MIN..=EMAIL_MAX).contains(&len)
        && !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !s.chars().any(|c| c.is_whitespace() || is_break_like(c))
}

/// `ret` when it is a path on this site: it starts with `/`, not `//`, and holds only
/// visible ASCII without `\` (so `/\host` is refused too), at most [`RETURN_MAX`] bytes.
/// This is the rule the host applies to a redirect's `Location`. The value is checked as
/// sent, never repaired.
pub fn safe_return(ret: &str) -> Option<&str> {
    let ok = ret.len() <= RETURN_MAX
        && ret.starts_with('/')
        && !ret.starts_with("//")
        && ret.bytes().all(|b| b.is_ascii_graphic() && b != b'\\');
    ok.then_some(ret)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn breaks_are_stripped_from_one_line_fields() {
        let s = Submission::from_pairs(&pairs(&[
            ("name", " Ann\r\nBcc: x@evil.test "),
            ("subject", "Hi\u{2028}there\tnow"),
            ("email", "a\r\n@b.test"),
        ]));
        assert_eq!(s.name, "AnnBcc: x@evil.test");
        assert_eq!(s.subject, "Hi there now");
        assert_eq!(s.email, "a@b.test");
    }

    #[test]
    fn format_characters_leave_one_line_fields_only() {
        let s = Submission::from_pairs(&pairs(&[
            ("name", "Ann\u{202E}gpj.exe\u{200B}"),
            ("subject", "\u{FEFF}Invoice\u{2066}\u{00AD}x\u{E0041}"),
            ("message", "Hello \u{202E}there\u{200D} friend"),
        ]));
        assert_eq!(s.name, "Anngpj.exe");
        assert_eq!(s.subject, "Invoicex");
        assert_eq!(s.message, "Hello \u{202E}there\u{200D} friend");
        // Not format characters: letters, marks, ordinary spaces.
        for c in ['a', 'ż', '\u{0301}', ' ', '😀', '\u{2028}'] {
            assert!(!is_format(c), "{c:?}");
        }
    }

    #[test]
    fn message_keeps_lines_as_lf() {
        assert_eq!(message_text("a\r\nb\rc\nd\u{0}e\tf\u{7}"), "a\nb\nc\nde\tf");
        assert_eq!(message_text("  \n x \n "), "x");
    }

    #[test]
    fn the_first_value_counts() {
        let s = Submission::from_pairs(&pairs(&[("name", "A"), ("name", "B")]));
        assert_eq!(s.name, "A");
    }

    #[test]
    fn emails() {
        for ok in ["a@b", "ann@example.org", "\"q\"@x.test"] {
            assert!(valid_email(ok), "{ok}");
        }
        for bad in ["ab", "@b.test", "a@", "a@b@c", "a b@c", "a\u{85}@b"] {
            assert!(!valid_email(bad), "{bad}");
        }
    }

    #[test]
    fn returns() {
        assert_eq!(safe_return("/contact/?a=1"), Some("/contact/?a=1"));
        for bad in [
            "",
            "//evil.test",
            "/\\evil.test",
            "/a\\b",
            "javascript:x",
            "https://x.test/",
            "/a b",
            "/ł",
        ] {
            assert_eq!(safe_return(bad), None, "{bad}");
        }
        assert!(safe_return(&format!("/{}", "a".repeat(511))).is_some());
        assert!(safe_return(&format!("/{}", "a".repeat(512))).is_none());
    }

    #[test]
    fn content_types() {
        assert!(is_urlencoded("application/x-www-form-urlencoded"));
        assert!(is_urlencoded(
            "Application/X-WWW-Form-URLencoded ; charset=utf-8"
        ));
        assert!(!is_urlencoded("text/plain"));
        assert!(!is_urlencoded("application/x-www-form-urlencodedx"));
        assert!(!is_urlencoded(""));
    }
}
