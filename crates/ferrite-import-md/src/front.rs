//! Front matter: optional TOML between a first line `+++` and the next `+++` line (P62).

use std::path::Path;

use ferrite_domain::Timestamp;
use serde::Deserialize;

use crate::error::MdError;

/// The front matter keys a Markdown file may set (P62). Every key is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub slug: Option<String>,
    pub order: Option<i32>,
    pub draft: bool,
    /// RFC 3339 with an offset: a TOML offset date-time or a string; kept to the
    /// microsecond.
    pub date: Option<Timestamp>,
    /// The excerpt and the SEO description.
    pub description: Option<String>,
}

/// Splits `text` into its front matter and the Markdown after it.
///
/// A leading UTF-8 BOM is dropped and CRLF line ends are accepted. Text whose first line is
/// not `+++` has no front matter: the default is returned with the whole text. A first line
/// `---` (a YAML block), a block without a closing `+++` line, invalid TOML, an unknown key
/// or a value of the wrong type is [`MdError::FrontMatter`], naming `file` and the line.
pub fn split_front_matter<'a>(
    text: &'a str,
    file: &Path,
) -> Result<(FrontMatter, &'a str), MdError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let err = |line: usize, message: String| MdError::FrontMatter {
        file: file.to_path_buf(),
        line,
        message,
    };
    let (first, block_start) = next_line(text, 0);
    match first {
        "+++" => {}
        "---" => {
            return Err(err(
                1,
                "YAML front matter (`---`) is not supported: put TOML between `+++` lines \
                 instead (`title: A` becomes `title = \"A\"`)"
                    .to_owned(),
            ));
        }
        _ => return Ok((FrontMatter::default(), text)),
    }
    let mut pos = block_start;
    loop {
        if pos >= text.len() {
            return Err(err(
                1,
                "the front matter has no closing `+++` line".to_owned(),
            ));
        }
        let (line, next) = next_line(text, pos);
        if line == "+++" {
            let toml_text = &text[block_start..pos];
            let front = parse(toml_text).map_err(|(offset, message)| {
                // The block starts on line 2.
                err(2 + toml_text[..offset].matches('\n').count(), message)
            })?;
            return Ok((front, &text[next..]));
        }
        pos = next;
    }
}

/// The line of `text` starting at `start`, without its line end and trailing whitespace,
/// and where the next line starts (`text.len()` after the last one).
fn next_line(text: &str, start: usize) -> (&str, usize) {
    let rest = &text[start..];
    match rest.find('\n') {
        Some(i) => (rest[..i].trim_end(), start + i + 1),
        None => (rest.trim_end(), text.len()),
    }
}

/// The keys as written; `date` keeps its span for the error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    title: Option<toml::Spanned<String>>,
    slug: Option<toml::Spanned<String>>,
    order: Option<i32>,
    #[serde(default)]
    draft: bool,
    date: Option<toml::Spanned<toml::Value>>,
    description: Option<String>,
}

/// The TOML block, or the byte offset in it and the message of what is wrong.
fn parse(block: &str) -> Result<FrontMatter, (usize, String)> {
    let raw: Raw = toml::from_str(block).map_err(|e| {
        let offset = e.span().map_or(0, |s| s.start.min(block.len()));
        (offset, e.message().trim_end().to_owned())
    })?;
    let date = match raw.date {
        None => None,
        Some(spanned) => {
            let offset = spanned.span().start.min(block.len());
            let text = match spanned.into_inner() {
                toml::Value::Datetime(d) => d.to_string(),
                toml::Value::String(s) => s,
                _ => String::new(),
            };
            // Stored with microseconds, as every timestamp of the site.
            let parsed = text
                .parse::<jiff::Timestamp>()
                .ok()
                .and_then(|t| Timestamp::from_micros(t.as_microsecond()).ok());
            Some(parsed.ok_or_else(|| {
                (
                    offset,
                    "`date` must be an RFC 3339 date-time with an offset, as \
                     2026-10-30T12:00:00Z"
                        .to_owned(),
                )
            })?)
        }
    };
    Ok(FrontMatter {
        title: not_blank("title", raw.title, block)?,
        slug: not_blank("slug", raw.slug, block)?,
        order: raw.order,
        draft: raw.draft,
        date,
        description: raw.description,
    })
}

/// `value` unless it is empty or only whitespace: then the offset of `key`'s value and
/// what is wrong.
fn not_blank(
    key: &str,
    value: Option<toml::Spanned<String>>,
    block: &str,
) -> Result<Option<String>, (usize, String)> {
    match value {
        Some(v) if v.get_ref().trim().is_empty() => Err((
            v.span().start.min(block.len()),
            format!("`{key}` must not be empty (leave the key out instead)"),
        )),
        v => Ok(v.map(toml::Spanned::into_inner)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn split(text: &str) -> Result<(FrontMatter, &str), MdError> {
        split_front_matter(text, Path::new("guides/a.md"))
    }

    fn ts(rfc3339: &str) -> Timestamp {
        let t: jiff::Timestamp = rfc3339.parse().unwrap();
        Timestamp::from_micros(t.as_microsecond()).unwrap()
    }

    fn line_of(e: MdError) -> (usize, String) {
        match e {
            MdError::FrontMatter {
                file,
                line,
                message,
            } => {
                assert_eq!(file, Path::new("guides/a.md"));
                (line, message)
            }
            other => panic!("not a front matter error: {other}"),
        }
    }

    #[test]
    fn every_key() {
        let text = "+++\ntitle = \"Install\"\nslug = \"setup\"\norder = 3\ndraft = true\n\
                    date = 2026-10-30T12:00:00Z\ndescription = \"How to install.\"\n+++\n# Body\n";
        let (fm, body) = split(text).unwrap();
        assert_eq!(
            fm,
            FrontMatter {
                title: Some("Install".into()),
                slug: Some("setup".into()),
                order: Some(3),
                draft: true,
                date: Some(ts("2026-10-30T12:00:00Z")),
                description: Some("How to install.".into()),
            }
        );
        assert_eq!(body, "# Body\n");
    }

    #[test]
    fn date_as_a_string_with_an_offset() {
        let (fm, _) = split("+++\ndate = \"2026-10-30T14:00:00+02:00\"\n+++\n").unwrap();
        assert_eq!(fm.date, Some(ts("2026-10-30T12:00:00Z")));
    }

    #[test]
    fn a_date_without_an_offset_is_refused() {
        let (line, msg) = line_of(split("+++\n\ndate = 2026-10-30T12:00:00\n+++\n").unwrap_err());
        assert_eq!(line, 3);
        assert!(msg.contains("date"), "{msg}");
    }

    #[test]
    fn no_block() {
        let (fm, body) = split("# Title\n\ntext\n").unwrap();
        assert_eq!(fm, FrontMatter::default());
        assert_eq!(body, "# Title\n\ntext\n");
        let (fm, body) = split("").unwrap();
        assert_eq!(fm, FrontMatter::default());
        assert_eq!(body, "");
        // `+++` later in the file is not front matter.
        let (_, body) = split("a\n+++\nb\n").unwrap();
        assert_eq!(body, "a\n+++\nb\n");
    }

    #[test]
    fn yaml_names_line_one() {
        let (line, msg) = line_of(split("---\ntitle: x\n---\nbody\n").unwrap_err());
        assert_eq!(line, 1);
        assert!(msg.contains("YAML"), "{msg}");
        assert!(msg.contains("+++"), "{msg}");
    }

    #[test]
    fn unknown_key_names_its_line() {
        let (line, msg) =
            line_of(split("+++\ntitle = \"a\"\nweight = 3\n+++\nbody\n").unwrap_err());
        assert_eq!(line, 3);
        assert!(msg.contains("weight"), "{msg}");
    }

    #[test]
    fn invalid_toml_and_wrong_types() {
        let (line, _) = line_of(split("+++\ntitle = \"a\"\n\ntitle = = 1\n+++\n").unwrap_err());
        assert_eq!(line, 4);
        let (line, _) = line_of(split("+++\norder = \"first\"\n+++\n").unwrap_err());
        assert_eq!(line, 2);
        let (line, _) = line_of(split("+++\norder = 99999999999\n+++\n").unwrap_err());
        assert_eq!(line, 2);
        let (line, _) = line_of(split("+++\ndraft = \"yes\"\n+++\n").unwrap_err());
        assert_eq!(line, 2);
    }

    #[test]
    fn empty_title_or_slug_is_refused() {
        let (line, msg) = line_of(split("+++\norder = 1\ntitle = \"\"\n+++\n").unwrap_err());
        assert_eq!(line, 3);
        assert!(msg.contains("title"), "{msg}");
        let (line, msg) = line_of(split("+++\nslug = \"  \"\n+++\n").unwrap_err());
        assert_eq!(line, 2);
        assert!(msg.contains("slug"), "{msg}");
    }

    #[test]
    fn unterminated_block() {
        let (line, msg) = line_of(split("+++\ntitle = \"a\"\n\nbody\n").unwrap_err());
        assert_eq!(line, 1);
        assert!(msg.contains("+++"), "{msg}");
    }

    #[test]
    fn bom_and_crlf() {
        let text = "\u{feff}+++\r\ntitle = \"T\"\r\norder = 2\r\n+++\r\nbody\r\n";
        let (fm, body) = split(text).unwrap();
        assert_eq!(fm.title.as_deref(), Some("T"));
        assert_eq!(fm.order, Some(2));
        assert_eq!(body, "body\r\n");
        let (fm, body) = split("\u{feff}# x\r\n").unwrap();
        assert_eq!(fm, FrontMatter::default());
        assert_eq!(body, "# x\r\n");
    }

    #[test]
    fn closing_line_at_the_end_of_the_file() {
        let (fm, body) = split("+++\ndraft = true\n+++").unwrap();
        assert!(fm.draft);
        assert_eq!(body, "");
        let (fm, body) = split("+++\n+++\nx").unwrap();
        assert_eq!(fm, FrontMatter::default());
        assert_eq!(body, "x");
    }
}
