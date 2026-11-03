//! Front matter: optional TOML between a first line `+++` and the next `+++` line (P62).

use std::path::Path;

use ferrite_core::content::SeoPatch;
use ferrite_core::content::seo::MAX_SEO_DESCRIPTION;
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
    /// The excerpt and the SEO description: whitespace and control characters collapsed
    /// to single spaces, trimmed, and cut to [`MAX_SEO_DESCRIPTION`] characters; `None`
    /// when nothing is left.
    pub description: Option<String>,
    /// The description was longer than [`MAX_SEO_DESCRIPTION`] characters and was cut.
    pub description_cut: bool,
    /// The page's locale as written (M6-1); checked against the site's locales by the run.
    pub locale: Option<String>,
    /// The page this one translates: a path relative to this file (M6-1).
    pub translation_of: Option<String>,
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

/// The earliest `date` accepted, 0001-01-01T00:00:00Z, in Unix microseconds.
const MIN_DATE: i64 = -62_135_596_800_000_000;
/// The latest `date` accepted, 9999-12-31T23:59:59.999999Z, in Unix microseconds (the
/// parser itself stops at 9999-12-30T22:00Z, jiff's limit).
const MAX_DATE: i64 = 253_402_300_799_999_999;

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
    description: Option<toml::Spanned<String>>,
    locale: Option<toml::Spanned<String>>,
    translation_of: Option<toml::Spanned<String>>,
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
            let parsed = text.parse::<jiff::Timestamp>().ok().ok_or_else(|| {
                (
                    offset,
                    "`date` must be an RFC 3339 date-time with an offset, as \
                     2026-10-30T12:00:00Z"
                        .to_owned(),
                )
            })?;
            // Both backends store the years 1 to 9999 (Postgres `timestamptz` starts at
            // 4713 BC; final re-review): another date would fail the whole run there.
            let in_range = (MIN_DATE..=MAX_DATE).contains(&parsed.as_microsecond());
            let stored = in_range
                .then(|| Timestamp::from_micros(parsed.as_microsecond()).ok())
                .flatten();
            Some(stored.ok_or_else(|| {
                (
                    offset,
                    "`date` must be in the years 1 to 9999 (UTC)".to_owned(),
                )
            })?)
        }
    };
    // NUL is stripped (Postgres refuses it in text), as the WordPress import does.
    let title = raw
        .title
        .map(|t| toml::Spanned::new(t.span(), t.get_ref().replace('\0', "")));
    let (description, description_cut) = match raw.description {
        None => (None, false),
        Some(d) => {
            let offset = d.span().start.min(block.len());
            let (text, cut) = clean_description(d.get_ref());
            // A backstop: the cleaned text always passes core's SEO check.
            let check = SeoPatch {
                description: Some(Some(text.clone())),
                ..SeoPatch::default()
            };
            check.check().map_err(|e| (offset, e.to_string()))?;
            ((!text.is_empty()).then_some(text), cut)
        }
    };
    let locale = match raw.locale {
        None => None,
        Some(l) => {
            let offset = l.span().start.min(block.len());
            Some(
                ferrite_domain::locale::parse_tag(l.get_ref().trim()).map_err(|_| {
                    (
                        offset,
                        "`locale` must be a language tag, as \"de\" or \"pt-BR\"".to_owned(),
                    )
                })?,
            )
        }
    };
    Ok(FrontMatter {
        locale,
        translation_of: not_blank("translation_of", raw.translation_of, block)?,
        title: not_blank("title", title, block)?,
        slug: not_blank("slug", raw.slug, block)?,
        order: raw.order,
        draft: raw.draft,
        date,
        description,
        description_cut,
    })
}

/// `text` with every run of whitespace and control characters made one space, trimmed,
/// and cut to [`MAX_SEO_DESCRIPTION`] characters (`true` when it was cut), so core's SEO
/// check accepts it (final review B-I1; the WordPress import cuts the same way).
fn clean_description(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            gap = !out.is_empty();
        } else {
            if gap {
                out.push(' ');
                gap = false;
            }
            out.push(c);
        }
    }
    match out.char_indices().nth(MAX_SEO_DESCRIPTION) {
        Some((at, _)) => {
            out.truncate(at);
            out.truncate(out.trim_end().len());
            (out, true)
        }
        None => (out, false),
    }
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
                    date = 2026-10-30T12:00:00Z\ndescription = \"How to install.\"\n\
                    locale = \"pt_BR\"\ntranslation_of = \"../en/install.md\"\n+++\n# Body\n";
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
                description_cut: false,
                locale: Some("pt-BR".into()),
                translation_of: Some("../en/install.md".into()),
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

    /// Final re-review: a date outside what both backends store (Postgres `timestamptz`
    /// starts at 4713 BC) skips the file instead of failing the run; years 1 to 9999 pass.
    #[test]
    fn a_date_outside_years_1_to_9999_is_refused() {
        for (date, why) in [
            ("\"-005000-01-01T00:00:00Z\"", "1 to 9999"),
            ("\"0000-12-31T23:59:59Z\"", "1 to 9999"),
            ("0000-06-01T00:00:00Z", "1 to 9999"),
            ("\"0001-01-01T00:30:00+01:00\"", "1 to 9999"),
            // Past what the parser reads at all.
            ("\"+010000-01-01T00:00:00Z\"", "RFC 3339"),
        ] {
            let (line, msg) =
                line_of(split(&format!("+++\ntitle = \"a\"\ndate = {date}\n+++\n")).unwrap_err());
            assert_eq!(line, 3, "{date}");
            assert!(msg.contains(why), "{date}: {msg}");
        }
        for (date, want) in [
            ("0001-01-01T00:00:00Z", "0001-01-01T00:00:00Z"),
            // The parser's own limit is 9999-12-30T22:00Z (jiff), inside the range.
            ("9999-12-30T21:59:59.999999Z", "9999-12-30T21:59:59.999999Z"),
            ("\"9999-12-30T21:00:00-00:30\"", "9999-12-30T21:30:00Z"),
        ] {
            let (fm, _) = split(&format!("+++\ndate = {date}\n+++\n")).unwrap();
            assert_eq!(fm.date, Some(ts(want)), "{date}");
        }
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

    /// Final review B-I1: a description core's SEO check would refuse is cleaned the way
    /// the WordPress import cleans one, never a failed run.
    #[test]
    fn a_multi_line_description_is_collapsed() {
        let text =
            "+++\ndescription = \"\"\"\n  How to\n\tinstall\\u0000\\u001b  it.\r\n\"\"\"\n+++\n";
        let (fm, _) = split(text).unwrap();
        assert_eq!(fm.description.as_deref(), Some("How to install it."));
        assert!(!fm.description_cut);
        // Only whitespace and control characters: no description.
        let (fm, _) = split("+++\ndescription = \"\\n\\t \"\n+++\n").unwrap();
        assert_eq!(fm.description, None);
    }

    #[test]
    fn a_long_description_is_cut_on_a_char_boundary() {
        let long = "é".repeat(600);
        let (fm, _) = split(&format!("+++\ndescription = \"{long}\"\n+++\n")).unwrap();
        let d = fm.description.unwrap();
        assert_eq!(d.chars().count(), MAX_SEO_DESCRIPTION);
        assert_eq!(d, "é".repeat(MAX_SEO_DESCRIPTION));
        assert!(fm.description_cut);
        // Exactly the limit is kept whole.
        let exact = "d".repeat(MAX_SEO_DESCRIPTION);
        let (fm, _) = split(&format!("+++\ndescription = \"{exact}\"\n+++\n")).unwrap();
        assert_eq!(fm.description.as_deref(), Some(exact.as_str()));
        assert!(!fm.description_cut);
    }

    #[test]
    fn nul_is_stripped_from_the_title() {
        let (fm, _) = split("+++\ntitle = \"Ins\\u0000tall\"\n+++\n").unwrap();
        assert_eq!(fm.title.as_deref(), Some("Install"));
        // A title of NUL only is empty.
        let (line, msg) = line_of(split("+++\n\ntitle = \"\\u0000\"\n+++\n").unwrap_err());
        assert_eq!(line, 3);
        assert!(msg.contains("title"), "{msg}");
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
