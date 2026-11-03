//! An item's SEO title, description, canonical URL and robots directives (M2b P28,
//! M3 P10): `content_meta` rows that the head (06 §5), SEO plugins (`content.meta.write = ["seo.*"]`) and the WordPress
//! importer share. Editors set them through content writes ([`SeoPatch`] on a
//! [`ContentDraft`](super::ContentDraft)), in the same transaction as the item.
//! Revisions do not restore them (P13).

use ferrite_domain::ContentId;
use ferrite_store::{Repos, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::CoreError;

/// Meta key holding an item's SEO title (JSON string): when set it is the whole
/// `<title>` and `og:title`.
pub const SEO_TITLE_META: &str = "seo.title";
/// Meta key holding an item's SEO description (JSON string): the meta description
/// before the excerpt.
pub const SEO_DESCRIPTION_META: &str = "seo.description";
/// Meta key holding an item's canonical URL (JSON string, absolute `http(s)`): it
/// replaces the page's own URL in `<link rel="canonical">` and `og:url`.
pub const SEO_CANONICAL_META: &str = "seo.canonical";
/// Meta key holding an item's robots directives (JSON array of strings, sorted and
/// deduplicated, see [`Robots`]).
pub const SEO_ROBOTS_META: &str = "seo.robots";
/// Longest SEO canonical URL, in characters.
pub const MAX_SEO_CANONICAL: usize = 2048;
/// Longest SEO title, in characters.
pub const MAX_SEO_TITLE: usize = 200;
/// Longest SEO description, in characters.
pub const MAX_SEO_DESCRIPTION: usize = 500;

/// A robots directive of an item's page (`<meta name="robots">`). Declared in
/// alphabetical order, so the derived `Ord` is the stored order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Robots {
    NoArchive,
    NoFollow,
    NoImageIndex,
    NoIndex,
    NoSnippet,
}

impl Robots {
    /// The directive as written in the tag and in storage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoArchive => "noarchive",
            Self::NoFollow => "nofollow",
            Self::NoImageIndex => "noimageindex",
            Self::NoIndex => "noindex",
            Self::NoSnippet => "nosnippet",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [
            Self::NoArchive,
            Self::NoFollow,
            Self::NoImageIndex,
            Self::NoIndex,
            Self::NoSnippet,
        ]
        .into_iter()
        .find(|r| r.as_str() == text)
    }
}

/// An item's SEO meta as stored. A stored value of the wrong JSON type (a plugin may
/// write any JSON under `seo.*`) reads as unset; unknown robots directives are skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seo {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical: Option<String>,
    /// Sorted and deduplicated; empty = none set.
    pub robots: Vec<Robots>,
}

impl Seo {
    /// The SEO meta of item `id` (one meta read).
    pub async fn load(repos: &dyn Repos, id: ContentId) -> Result<Self, StoreError> {
        let mut seo = Self::default();
        for (_, key, value) in repos
            .meta()
            .get_many(
                &[id],
                &[
                    SEO_TITLE_META,
                    SEO_DESCRIPTION_META,
                    SEO_CANONICAL_META,
                    SEO_ROBOTS_META,
                ],
            )
            .await?
        {
            match (key.as_str(), value) {
                (SEO_TITLE_META, Value::String(text)) => seo.title = Some(text),
                (SEO_DESCRIPTION_META, Value::String(text)) => seo.description = Some(text),
                // A plugin may have stored anything: only a URL the checks would accept is
                // ever read (the head never emits another).
                (SEO_CANONICAL_META, Value::String(text))
                    if text.chars().count() <= MAX_SEO_CANONICAL
                        && !text.chars().any(|c| c.is_control() || c.is_whitespace())
                        && is_http_url(&text) =>
                {
                    seo.canonical = Some(text);
                }
                (SEO_ROBOTS_META, Value::Array(items)) => {
                    seo.robots = normalise(
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .filter_map(Robots::parse),
                    );
                }
                _ => {}
            }
        }
        Ok(seo)
    }
}

/// A change to an item's SEO meta. Per member: `None` keeps the stored row,
/// `Some(None)` or a string that is blank after trimming deletes it, any other string
/// sets it (trimmed). A string must not hold NUL, nor control characters once trimmed
/// (a trailing newline is trimmed away, one inside the text is refused), and, trimmed,
/// be at most [`MAX_SEO_TITLE`] / [`MAX_SEO_DESCRIPTION`] characters, else
/// `InvalidField` naming `seo.title` / `seo.description`. `canonical`, trimmed, must be
/// an absolute `http(s)` URL with a host, without whitespace or control characters, at
/// most [`MAX_SEO_CANONICAL`] characters (`seo.canonical`). `robots` is stored sorted and
/// deduplicated; `Some(None)` or an empty list deletes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeoPatch {
    pub title: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub canonical: Option<Option<String>>,
    pub robots: Option<Option<Vec<Robots>>>,
}

impl SeoPatch {
    /// The checks of [`SeoPatch`], without writing.
    pub fn check(&self) -> Result<(), CoreError> {
        check_member(SEO_TITLE_META, &self.title, MAX_SEO_TITLE)?;
        check_member(SEO_DESCRIPTION_META, &self.description, MAX_SEO_DESCRIPTION)?;
        check_canonical(&self.canonical)
    }

    /// The inverse of [`to_json`](Self::to_json): an object whose members `title`,
    /// `description`, `canonical` (string or `null`) and `robots` (array of directives
    /// or `null`) become `Some`; anything else is `InvalidField { field: "seo" }`. Not
    /// checked: call [`check`](Self::check).
    pub fn from_json(value: &Value) -> Result<Self, CoreError> {
        let bad = |m: &str| CoreError::invalid_field("seo", m.to_owned());
        let Value::Object(map) = value else {
            return Err(bad("seo must be an object"));
        };
        let text = |key: &str| -> Result<Option<Option<String>>, CoreError> {
            match map.get(key) {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
                Some(_) => Err(bad("seo members are strings")),
            }
        };
        if map
            .keys()
            .any(|k| !matches!(k.as_str(), "title" | "description" | "canonical" | "robots"))
        {
            return Err(bad("unknown seo member"));
        }
        let robots = match map.get("robots") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(v) => Some(Some(
                serde_json::from_value::<Vec<Robots>>(v.clone())
                    .map_err(|_| bad("unknown robots directive"))?,
            )),
        };
        Ok(Self {
            title: text("title")?,
            description: text("description")?,
            canonical: text("canonical")?,
            robots,
        })
    }

    /// The patch as JSON, as the autosave slot stores it: an object with a member for
    /// each `Some` (`null` for `Some(None)`), the strings as given.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        if let Some(title) = &self.title {
            out.insert(
                "title".into(),
                title.clone().map_or(Value::Null, Value::String),
            );
        }
        if let Some(description) = &self.description {
            out.insert(
                "description".into(),
                description.clone().map_or(Value::Null, Value::String),
            );
        }
        if let Some(canonical) = &self.canonical {
            out.insert(
                "canonical".into(),
                canonical.clone().map_or(Value::Null, Value::String),
            );
        }
        if let Some(robots) = &self.robots {
            out.insert(
                "robots".into(),
                robots.as_ref().map_or(Value::Null, |r| {
                    Value::Array(r.iter().map(|d| Value::String(d.as_str().into())).collect())
                }),
            );
        }
        Value::Object(out)
    }

    /// Writes or deletes the meta rows of item `id` through `repos` (the caller's
    /// transaction). The caller has run [`check`](Self::check).
    pub(crate) async fn apply(&self, repos: &dyn Repos, id: ContentId) -> Result<(), CoreError> {
        for (key, member) in [
            (SEO_TITLE_META, &self.title),
            (SEO_DESCRIPTION_META, &self.description),
            (SEO_CANONICAL_META, &self.canonical),
        ] {
            let Some(value) = member else {
                continue;
            };
            match value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                Some(text) => {
                    repos
                        .meta()
                        .set(id, key, &Value::String(text.to_owned()))
                        .await?
                }
                None => repos.meta().delete(id, key).await?,
            }
        }
        if let Some(robots) = &self.robots {
            let robots = normalise(robots.iter().flatten().copied());
            if robots.is_empty() {
                repos.meta().delete(id, SEO_ROBOTS_META).await?;
            } else {
                let value = Value::Array(
                    robots
                        .iter()
                        .map(|r| Value::String(r.as_str().into()))
                        .collect(),
                );
                repos.meta().set(id, SEO_ROBOTS_META, &value).await?;
            }
        }
        Ok(())
    }
}

/// Sorted, deduplicated.
fn normalise(robots: impl Iterator<Item = Robots>) -> Vec<Robots> {
    let mut out: Vec<Robots> = robots.collect();
    out.sort();
    out.dedup();
    out
}

/// The canonical member of a [`SeoPatch`] (blank deletes, so passes).
fn check_canonical(member: &Option<Option<String>>) -> Result<(), CoreError> {
    let Some(Some(text)) = member else {
        return Ok(());
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let field = SEO_CANONICAL_META;
    let bad = |why: &str| Err(CoreError::invalid_field(field, format!("{field} {why}")));
    if text.chars().count() > MAX_SEO_CANONICAL {
        return bad(&format!("is at most {MAX_SEO_CANONICAL} characters"));
    }
    if text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return bad("must not contain whitespace or control characters");
    }
    if !is_http_url(text) {
        return bad("must be an absolute http(s) URL");
    }
    Ok(())
}

/// Whether `text` parses as an `http(s)` URL with a host.
fn is_http_url(text: &str) -> bool {
    Url::parse(text)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

/// One member of a [`SeoPatch`]: no NUL; once trimmed, no control characters and at
/// most `max` characters.
fn check_member(
    field: &'static str,
    member: &Option<Option<String>>,
    max: usize,
) -> Result<(), CoreError> {
    let Some(Some(text)) = member else {
        return Ok(());
    };
    if text.contains('\0') {
        return Err(CoreError::invalid_field(
            field,
            format!("{field} must not contain NUL"),
        ));
    }
    // Checked on the trimmed text: an edge newline (a textarea's) is trimmed away.
    if text.trim().chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            field,
            format!("{field} must not contain control characters"),
        ));
    }
    if text.trim().chars().count() > max {
        return Err(CoreError::invalid_field(
            field,
            format!("{field} is at most {max} characters"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(r: Result<(), CoreError>) -> Option<&'static str> {
        match r {
            Err(CoreError::InvalidField { field, .. }) => Some(field),
            _ => None,
        }
    }

    #[test]
    fn checks_name_the_member() {
        let ok = SeoPatch {
            title: Some(Some(format!("  {}  ", "é".repeat(MAX_SEO_TITLE)))),
            description: Some(Some("d".repeat(MAX_SEO_DESCRIPTION))),
            ..SeoPatch::default()
        };
        assert!(ok.check().is_ok());
        let long = SeoPatch {
            title: Some(Some("t".repeat(MAX_SEO_TITLE + 1))),
            ..SeoPatch::default()
        };
        assert_eq!(field(long.check()), Some("seo.title"));
        let long = SeoPatch {
            description: Some(Some("d".repeat(MAX_SEO_DESCRIPTION + 1))),
            ..SeoPatch::default()
        };
        assert_eq!(field(long.check()), Some("seo.description"));
        for bad in ["a\0b", "a\nb", "a\u{7}b"] {
            let p = SeoPatch {
                description: Some(Some(bad.into())),
                ..SeoPatch::default()
            };
            assert_eq!(field(p.check()), Some("seo.description"), "{bad:?}");
        }
        // Deleting needs no checks.
        assert!(
            SeoPatch {
                title: Some(None),
                description: Some(Some("   ".into())),
                ..SeoPatch::default()
            }
            .check()
            .is_ok()
        );
    }

    #[test]
    fn edge_controls_are_trimmed_interior_ones_refused() {
        // A textarea's trailing newline (or a leading tab) is trimmed, not refused.
        for ok in ["text\n", "text\r\n", "\ttext", " text \n"] {
            let p = SeoPatch {
                description: Some(Some(ok.into())),
                title: Some(Some(ok.into())),
                ..SeoPatch::default()
            };
            assert!(p.check().is_ok(), "{ok:?}");
        }
        for bad in ["two\nlines", "a\tb", "a\u{1b}b"] {
            let p = SeoPatch {
                title: Some(Some(format!("{bad}\n"))),
                ..SeoPatch::default()
            };
            assert_eq!(field(p.check()), Some("seo.title"), "{bad:?}");
        }
    }

    #[test]
    fn json_keeps_absent_and_null_apart() {
        assert_eq!(SeoPatch::default().to_json(), json!({}));
        let p = SeoPatch {
            title: Some(None),
            description: Some(Some(" as given ".into())),
            canonical: Some(None),
            robots: Some(Some(vec![Robots::NoIndex])),
        };
        assert_eq!(
            p.to_json(),
            json!({"title": null, "description": " as given ", "canonical": null, "robots": ["noindex"]})
        );
    }
}
