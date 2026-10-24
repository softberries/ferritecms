//! The site settings the admin edits (P19): an allowlist of option keys, each with a
//! validator. `PATCH /settings` writes them through
//! [`OptionService::set_many`](crate::OptionService::set_many); [`read_all`] is the
//! actor-checked read (F16).
//!
//! | key | value |
//! |---|---|
//! | `site.title` | string of at most 200 characters, no control characters |
//! | `site.description` | string of at most 1000 characters (line breaks and tabs allowed) |
//! | `site.locale` | 1..=35 ASCII letters, digits, `-` or `_` (a BCP 47 tag such as `pt-BR`) |
//! | `reading.per_page` | integer 1..=100 |
//! | `reading.front_page` | the UUID string of an existing, non-trashed page (checked by `set_many`), or null |
//! | `reading.discourage_indexing` | boolean |
//! | `permalink.post`, `permalink.page` | a permalink structure ([`PermalinkRules::from_options`]); the pair is checked against the registered types and taxonomies inside the write's transaction |
//! | `comments.enabled` | boolean (default `true`, [`COMMENTS_ENABLED_DEFAULT`](crate::comments::COMMENTS_ENABLED_DEFAULT)) |
//! | `comments.moderation` | `"all"` (every guest comment is held) or `"known"` (default, [`COMMENTS_MODERATION_DEFAULT`](crate::comments::COMMENTS_MODERATION_DEFAULT): a commenter with an approved comment posts directly; anyone who knows a regular guest's exact name and e-mail posts unmoderated as them, which `"all"` prevents) |
//!
//! A `null` value (or a missing key in the store) means the default.

use ferrite_domain::{ContentId, ContentStatus};
use ferrite_store::Store;
use serde_json::{Map, Value};

use crate::{
    Actor, CoreError,
    actor::{Area, require_read},
    comments::{ENABLED_OPTION, MODERATION_CHOICES, MODERATION_OPTION},
    permalink::{PAGE_OPTION, POST_OPTION, PermalinkRules},
};

/// Longest `site.title` (characters).
pub const MAX_TITLE: usize = 200;
/// Longest `site.description` (characters).
pub const MAX_DESCRIPTION: usize = 1000;
/// Longest `site.locale` (characters).
pub const MAX_LOCALE: usize = 35;

/// What a setting holds; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// A string of at most `max` characters; `multiline` allows `\n`, `\r` and `\t`.
    Text {
        max: usize,
        multiline: bool,
    },
    /// A locale tag.
    Locale,
    /// An integer in `min..=max`.
    Integer {
        min: i64,
        max: i64,
    },
    Bool,
    /// The id of a page, or null.
    FrontPage,
    /// A permalink structure.
    Permalink,
    /// One of the listed strings.
    Choice(&'static [&'static str]),
}

/// One allowlisted setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingDef {
    pub key: &'static str,
    pub kind: SettingKind,
}

/// Every setting `PATCH /settings` accepts, in the order `GET /settings` lists them.
pub const SETTINGS: &[SettingDef] = &[
    SettingDef {
        key: "site.title",
        kind: SettingKind::Text {
            max: MAX_TITLE,
            multiline: false,
        },
    },
    SettingDef {
        key: "site.description",
        kind: SettingKind::Text {
            max: MAX_DESCRIPTION,
            multiline: true,
        },
    },
    SettingDef {
        key: "site.locale",
        kind: SettingKind::Locale,
    },
    SettingDef {
        key: "reading.per_page",
        kind: SettingKind::Integer { min: 1, max: 100 },
    },
    SettingDef {
        key: "reading.front_page",
        kind: SettingKind::FrontPage,
    },
    SettingDef {
        key: "reading.discourage_indexing",
        kind: SettingKind::Bool,
    },
    SettingDef {
        key: POST_OPTION,
        kind: SettingKind::Permalink,
    },
    SettingDef {
        key: PAGE_OPTION,
        kind: SettingKind::Permalink,
    },
    SettingDef {
        key: ENABLED_OPTION,
        kind: SettingKind::Bool,
    },
    SettingDef {
        key: MODERATION_OPTION,
        kind: SettingKind::Choice(&MODERATION_CHOICES),
    },
];

/// The allowlisted setting `key`.
pub fn def(key: &str) -> Option<&'static SettingDef> {
    SETTINGS.iter().find(|d| d.key == key)
}

/// Checks `value` for setting `key` as far as it can without the store: an unknown key
/// is `Invalid` naming it, a bad value `InvalidField { field: key, .. }`. The front page
/// must still exist and `permalink.*` must still fit the registered names, which
/// `set_many` checks in its transaction.
pub fn validate(key: &str, value: &Value) -> Result<(), CoreError> {
    let def = def(key).ok_or_else(|| CoreError::Invalid(format!("unknown setting {key:?}")))?;
    let bad = |message: String| Err(CoreError::invalid_field(def.key, message));
    match def.kind {
        SettingKind::Text { max, multiline } => {
            let Some(s) = value.as_str() else {
                return bad(format!("{key} must be a string"));
            };
            let control =
                |c: char| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t'));
            if s.chars().count() > max || s.chars().any(control) {
                return bad(format!(
                    "{key} must be at most {max} characters without control characters"
                ));
            }
        }
        SettingKind::Locale => {
            let ok = value.as_str().is_some_and(|s| {
                (1..=MAX_LOCALE).contains(&s.len())
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            });
            if !ok {
                return bad(format!(
                    "{key} must be 1 to {MAX_LOCALE} ASCII letters, digits, '-' or '_'"
                ));
            }
        }
        SettingKind::Integer { min, max } => {
            if !value.as_i64().is_some_and(|n| (min..=max).contains(&n)) {
                return bad(format!("{key} must be an integer from {min} to {max}"));
            }
        }
        SettingKind::Bool => {
            if !value.is_boolean() {
                return bad(format!("{key} must be true or false"));
            }
        }
        SettingKind::FrontPage => {
            if !value.is_null() && front_page_id(value).is_none() {
                return bad(format!("{key} must be the id of a page, or null"));
            }
        }
        SettingKind::Choice(choices) => {
            if !value.as_str().is_some_and(|s| choices.contains(&s)) {
                return bad(format!("{key} must be one of {}", choices.join(", ")));
            }
        }
        SettingKind::Permalink => {
            let Some(s) = value.as_str() else {
                return bad(format!("{key} must be a string"));
            };
            let parsed = if def.key == PAGE_OPTION {
                PermalinkRules::from_options(None, Some(s))
            } else {
                PermalinkRules::from_options(Some(s), None)
            };
            if let Err(e) = parsed {
                return bad(message_of(e));
            }
        }
    }
    Ok(())
}

/// The content id a `reading.front_page` value names.
fn front_page_id(value: &Value) -> Option<ContentId> {
    value.as_str().and_then(|s| s.parse::<ContentId>().ok())
}

/// `Ok` if `value` names an existing, non-trashed page (read through `repos`).
pub(crate) async fn check_front_page(
    repos: &dyn ferrite_store::Repos,
    value: &Value,
) -> Result<(), CoreError> {
    let Some(id) = front_page_id(value) else {
        return Ok(());
    };
    match repos.content().get(id).await? {
        Some(item) if item.kind == "page" && item.status != ContentStatus::Trash => Ok(()),
        _ => Err(CoreError::invalid_field(
            "reading.front_page",
            "reading.front_page must be the id of an existing page",
        )),
    }
}

/// The text of an `Invalid` error (other errors are shown with their `Display`).
fn message_of(e: CoreError) -> String {
    match e {
        CoreError::Invalid(m) | CoreError::InvalidField { message: m, .. } => m,
        other => other.to_string(),
    }
}

/// Every allowlisted setting as stored, `null` for unset ones, in [`SETTINGS`] order
/// (`require_read(Settings)`, F16). Read from the store, not a node's cache.
pub async fn read_all(store: &dyn Store, actor: &Actor) -> Result<Map<String, Value>, CoreError> {
    require_read(actor, Area::Settings)?;
    let options = store.read().options();
    let mut out = Map::new();
    for def in SETTINGS {
        let value = options.get(def.key).await?.map_or(Value::Null, |e| e.value);
        out.insert(def.key.to_owned(), value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_key_is_unique_and_validates_null_as_its_kind_says() {
        for (i, d) in SETTINGS.iter().enumerate() {
            assert!(SETTINGS[..i].iter().all(|o| o.key != d.key), "{}", d.key);
            let null_ok = d.kind == SettingKind::FrontPage;
            assert_eq!(validate(d.key, &json!(null)).is_ok(), null_ok, "{}", d.key);
        }
    }

    #[test]
    fn comment_settings_validate() {
        validate("comments.enabled", &json!(false)).unwrap();
        for ok in ["all", "known"] {
            validate("comments.moderation", &json!(ok)).unwrap();
        }
        for bad in [json!("none"), json!("ALL"), json!(1), json!(true)] {
            match validate("comments.moderation", &bad) {
                Err(CoreError::InvalidField { field, message }) => {
                    assert_eq!(field, "comments.moderation");
                    assert!(message.contains("all, known"), "{message}");
                }
                other => panic!("{bad}: {other:?}"),
            }
        }
        assert!(validate("comments.enabled", &json!("yes")).is_err());
    }
}
