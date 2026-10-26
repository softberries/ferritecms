//! An item's SEO title and description (M2b P28): two `content_meta` rows that the
//! head (06 §5), SEO plugins (`content.meta.write = ["seo.*"]`) and the WordPress
//! importer share. Editors set them through content writes ([`SeoPatch`] on a
//! [`ContentDraft`](super::ContentDraft)), in the same transaction as the item.
//! Revisions do not restore them (P13).

use ferrite_domain::ContentId;
use ferrite_store::{Repos, StoreError};
use serde_json::{Map, Value};

use crate::CoreError;

/// Meta key holding an item's SEO title (JSON string): when set it is the whole
/// `<title>` and `og:title`.
pub const SEO_TITLE_META: &str = "seo.title";
/// Meta key holding an item's SEO description (JSON string): the meta description
/// before the excerpt.
pub const SEO_DESCRIPTION_META: &str = "seo.description";
/// Longest SEO title, in characters.
pub const MAX_SEO_TITLE: usize = 200;
/// Longest SEO description, in characters.
pub const MAX_SEO_DESCRIPTION: usize = 500;

/// An item's SEO meta as stored. A stored value that is not a JSON string (a plugin
/// may write any JSON under `seo.*`) reads as `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seo {
    pub title: Option<String>,
    pub description: Option<String>,
}

impl Seo {
    /// The SEO meta of item `id` (one meta read).
    pub async fn load(repos: &dyn Repos, id: ContentId) -> Result<Self, StoreError> {
        let mut seo = Self::default();
        for (_, key, value) in repos
            .meta()
            .get_many(&[id], &[SEO_TITLE_META, SEO_DESCRIPTION_META])
            .await?
        {
            let Value::String(text) = value else {
                continue;
            };
            match key.as_str() {
                SEO_TITLE_META => seo.title = Some(text),
                SEO_DESCRIPTION_META => seo.description = Some(text),
                _ => {}
            }
        }
        Ok(seo)
    }
}

/// A change to an item's SEO meta. Per member: `None` keeps the stored row,
/// `Some(None)` or a string that is blank after trimming deletes it, any other string
/// sets it (trimmed). A string must not hold control characters (NUL included) and,
/// trimmed, be at most [`MAX_SEO_TITLE`] / [`MAX_SEO_DESCRIPTION`] characters, else
/// `InvalidField` naming `seo.title` / `seo.description`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeoPatch {
    pub title: Option<Option<String>>,
    pub description: Option<Option<String>>,
}

impl SeoPatch {
    /// The checks of [`SeoPatch`], without writing.
    pub fn check(&self) -> Result<(), CoreError> {
        check_member(SEO_TITLE_META, &self.title, MAX_SEO_TITLE)?;
        check_member(SEO_DESCRIPTION_META, &self.description, MAX_SEO_DESCRIPTION)
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
        Value::Object(out)
    }

    /// Writes or deletes the meta rows of item `id` through `repos` (the caller's
    /// transaction). The caller has run [`check`](Self::check).
    pub(crate) async fn apply(&self, repos: &dyn Repos, id: ContentId) -> Result<(), CoreError> {
        for (key, member) in [
            (SEO_TITLE_META, &self.title),
            (SEO_DESCRIPTION_META, &self.description),
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
        Ok(())
    }
}

/// One member of a [`SeoPatch`]: no control characters, at most `max` characters once
/// trimmed.
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
    if text.chars().any(char::is_control) {
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
            }
            .check()
            .is_ok()
        );
    }

    #[test]
    fn json_keeps_absent_and_null_apart() {
        assert_eq!(SeoPatch::default().to_json(), json!({}));
        let p = SeoPatch {
            title: Some(None),
            description: Some(Some(" as given ".into())),
        };
        assert_eq!(
            p.to_json(),
            json!({"title": null, "description": " as given "})
        );
    }
}
