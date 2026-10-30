//! What is stored: one blob under the KV key `inbox`, JSON lines, a version line first
//! and then one message per line, newest first:
//!
//! ```text
//! {"v":1}
//! {"at":…,"name":…,"email":…,"subject":…,"page":…,"message":…}
//! {"at":…,…}
//! ```
//!
//! Lines, not one JSON document, because of fuel: decoding and encoding JSON costs guest
//! instructions per byte, and a message every character of which needs an escape (a
//! message of `"`) costs most. Compact JSON never holds a raw line break, so a submit finds
//! the stored messages by their line breaks, adds its own line and writes the blob back
//! without decoding any of them; the inbox page decodes only the messages it shows.

use serde::{Deserialize, Serialize};

/// The KV key.
pub const KEY: &str = "inbox";
/// The blob format. A blob with a higher version is left alone.
pub const VERSION: u64 = 1;
const HEADER: &[u8] = b"{\"v\":1}";
/// Most messages kept.
pub const KEEP: usize = 100;
/// Most bytes the blob holds: past it the oldest messages go, before [`KEEP`] is reached.
/// A hundred messages of 5000 plain characters fit (about 660 KB); the largest valid
/// message takes about 23 KB (four-byte characters), so the blob then holds about 40.
/// The cap keeps the write under the 1 MiB a call may write.
pub const MAX_BYTES: usize = 900 * 1024;

/// One message, as posted (cleaned and checked).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// RFC 3339 UTC, to the second (the wall clock).
    pub at: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub subject: String,
    /// The page the form was on: its `return`, else the `Referer`; empty if neither.
    #[serde(default)]
    pub page: String,
    pub message: String,
}

impl Message {
    /// The message as one line of the blob (compact JSON: no raw line break).
    pub fn line(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }

    /// A stored line read back; `None` when it is not a message, or is longer than any line
    /// this plugin writes ([`MAX_LINE`]), which is never decoded: decoding costs fuel per
    /// byte, and a huge line written into the store by hand must not trap the page.
    pub fn decode(line: &[u8]) -> Option<Self> {
        if line.len() > MAX_LINE {
            return None;
        }
        serde_json::from_slice(line).ok()
    }
}

/// Longest message line decoded. The longest the plugin writes is about 23 KB: 5000
/// four-byte characters of message, 100 of name, 254 of e-mail and 150 of subject, a page of
/// 512 characters each escaped (`\"`), the time and the keys.
pub const MAX_LINE: usize = 32 * 1024;
/// Longest version line decoded. Ours is `{"v":1}`; the slack leaves room for a newer
/// version's line to be recognised (and left alone) rather than replaced.
const MAX_VERSION_LINE: usize = 1024;

/// A stored blob, opened.
#[derive(Debug)]
pub enum Stored<'a> {
    /// The message lines, newest first, still encoded.
    Lines(Vec<&'a [u8]>),
    /// Written by a newer version of this plugin: not ours to overwrite.
    Newer,
    /// Not a blob of ours.
    Corrupt,
}

#[derive(Deserialize)]
struct Version {
    v: Option<u64>,
}

impl<'a> Stored<'a> {
    /// Split a stored blob into its lines. Only the version line is decoded, and only when
    /// it is at most [`MAX_VERSION_LINE`] bytes (a longer one is not ours).
    pub fn open(bytes: &'a [u8]) -> Self {
        let mut lines = Lines(bytes);
        let first = lines.next().unwrap_or_default();
        if first.len() > MAX_VERSION_LINE {
            return Self::Corrupt;
        }
        match serde_json::from_slice::<Version>(first) {
            Ok(Version { v: Some(VERSION) }) => {
                Self::Lines(lines.filter(|l| !l.is_empty()).collect())
            }
            Ok(Version { v: Some(v) }) if v > VERSION => Self::Newer,
            _ => Self::Corrupt,
        }
    }
}

/// The lines of a blob, split at LF (the last line may lack one).
struct Lines<'a>(&'a [u8]);

impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.0.is_empty() {
            return None;
        }
        let (line, rest) = match find_lf(self.0) {
            Some(at) => (&self.0[..at], &self.0[at + 1..]),
            None => (self.0, &self.0[self.0.len()..]),
        };
        self.0 = rest;
        Some(line)
    }
}

/// The index of the first LF in `bytes`. It tests eight bytes per step (a word with a zero
/// byte after XOR with LFs): the blob is up to 900 KB and fuel is counted per guest
/// instruction, so a byte-at-a-time loop costs several times more.
fn find_lf(bytes: &[u8]) -> Option<usize> {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGHS: u64 = 0x8080_8080_8080_8080;
    const LFS: u64 = 0x0a0a_0a0a_0a0a_0a0a;
    let (words, _) = bytes.as_chunks::<8>();
    let mut at = 0;
    for word in words {
        let x = u64::from_le_bytes(*word) ^ LFS;
        if x.wrapping_sub(ONES) & !x & HIGHS != 0 {
            break;
        }
        at += 8;
    }
    bytes[at..].iter().position(|&b| b == b'\n').map(|i| at + i)
}

/// The blob holding `newest` and then as many of `older` (newest first) as fit [`KEEP`]
/// messages and [`MAX_BYTES`]. `newest` always stays: one message is far under the cap.
pub fn blob(newest: &[u8], older: &[&[u8]]) -> Vec<u8> {
    let mut size = HEADER.len() + 1 + newest.len() + 1;
    let mut kept = 0;
    for line in older.iter().take(KEEP - 1) {
        if size + line.len() + 1 > MAX_BYTES {
            break;
        }
        size += line.len() + 1;
        kept += 1;
    }
    let mut out = Vec::with_capacity(size);
    for line in [HEADER, newest]
        .into_iter()
        .chain(older[..kept].iter().copied())
    {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> Message {
        Message {
            at: "2026-01-01T00:00:00Z".into(),
            name: "N \"q\"".into(),
            email: "a@b".into(),
            subject: String::new(),
            page: "/x".into(),
            message: text.into(),
        }
    }

    /// Push `text` onto `stored` as the plugin does.
    fn push(stored: &[u8], text: &str) -> Vec<u8> {
        let line = message(text).line().unwrap();
        match Stored::open(stored) {
            Stored::Lines(older) => blob(&line, &older),
            other => panic!("{other:?}"),
        }
    }

    fn texts(stored: &[u8]) -> Vec<String> {
        match Stored::open(stored) {
            Stored::Lines(lines) => lines
                .iter()
                .map(|l| Message::decode(l).unwrap().message)
                .collect(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lines_split_at_every_lf() {
        fn split(b: &[u8]) -> Vec<&[u8]> {
            Lines(b).collect()
        }
        assert!(split(b"").is_empty());
        assert_eq!(split(b"\n"), [&b""[..]]);
        assert_eq!(split(b"a\n\nb"), [&b"a"[..], b"", b"b"]);
        for len in 0..40 {
            let mut bytes = vec![b'x'; len];
            bytes.push(b'\n');
            bytes.extend_from_slice("żółw\n😀".as_bytes());
            assert_eq!(
                split(&bytes),
                [&vec![b'x'; len][..], "żółw".as_bytes(), "😀".as_bytes()]
            );
            assert_eq!(find_lf(&bytes), Some(len));
        }
        assert_eq!(find_lf(&[0x8a; 20]), None);
        assert_eq!(find_lf(b"0123456789abcdef"), None);
    }

    #[test]
    fn the_longest_line_written_is_decoded_and_a_longer_one_is_not() {
        let m = Message {
            at: "2026-01-01T00:00:00Z".into(),
            name: "😀".repeat(100),
            email: format!("{}@{}", "😀".repeat(126), "😀".repeat(127)),
            subject: "😀".repeat(150),
            page: format!("/{}", "\"".repeat(511)),
            message: "😀".repeat(5000),
        };
        let line = m.line().unwrap();
        assert!(line.len() <= MAX_LINE, "{}", line.len());
        assert!(Message::decode(&line).is_some());
        let mut long = m.clone();
        long.message.push_str(&"x".repeat(MAX_LINE));
        assert!(Message::decode(&long.line().unwrap()).is_none());
        // A version line padded past the bound is not read, whatever it says.
        let header = format!("{{\"v\":1,\"pad\":\"{}\"}}\n", "x".repeat(MAX_VERSION_LINE));
        assert!(matches!(Stored::open(header.as_bytes()), Stored::Corrupt));
    }

    #[test]
    fn a_line_holds_no_raw_break() {
        let line = message("a\nb\r\nc\u{2028}d").line().unwrap();
        assert!(!line.contains(&b'\n'));
        assert_eq!(
            Message::decode(&line).unwrap().message,
            "a\nb\r\nc\u{2028}d"
        );
    }

    #[test]
    fn keeps_the_newest_hundred() {
        let mut stored = blob(&message("0").line().unwrap(), &[]);
        for i in 1..105 {
            stored = push(&stored, &i.to_string());
        }
        let texts = texts(&stored);
        assert_eq!(texts.len(), KEEP);
        assert_eq!(texts[0], "104");
        assert_eq!(texts[KEEP - 1], "5");
    }

    #[test]
    fn stays_within_the_byte_cap() {
        let big = "😀".repeat(5000);
        let mut stored = blob(&message(&big).line().unwrap(), &[]);
        for _ in 0..KEEP {
            stored = push(&stored, &big);
        }
        assert!(stored.len() <= MAX_BYTES, "{}", stored.len());
        assert!(stored.len() > MAX_BYTES - 21_000, "{}", stored.len());
        assert!(texts(&stored).len() < KEEP);
    }

    #[test]
    fn opens_ours_newer_and_foreign() {
        assert!(matches!(Stored::open(b"{\"v\":1}\n"), Stored::Lines(l) if l.is_empty()));
        assert!(matches!(Stored::open(b"{\"v\":1}"), Stored::Lines(l) if l.is_empty()));
        assert!(matches!(
            Stored::open(b"{\"v\":2,\"other\":[1]}\nwhatever"),
            Stored::Newer
        ));
        for bad in [&b"x"[..], b"", b"[]", b"{\"v\":0}\n", b"{\"messages\":[]}"] {
            assert!(matches!(Stored::open(bad), Stored::Corrupt), "{bad:?}");
        }
    }
}
