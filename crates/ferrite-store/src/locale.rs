//! Shared rules of the locale filters (design 13 §3), the same on both backends.

use ferrite_domain::{ContentQuery, MEDIA_KIND};

use crate::{MAX_GROUPS, StoreError};

/// How a [`ContentQuery::locale`] (or a search's locale) becomes SQL: media items are
/// shared across locales and never filtered by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleFilter {
    /// No predicate: no locale asked, or only media can match.
    None,
    /// `locale = ?`: no media item can match.
    Plain,
    /// `(locale = ? OR type = 'media')`: media items may match.
    ExceptMedia,
}

/// The locale predicate of `query` (its kinds decide whether media items may match).
pub fn locale_filter(query: &ContentQuery) -> LocaleFilter {
    if query.locale.is_none() {
        return LocaleFilter::None;
    }
    kinds_locale_filter(query.kind.as_deref(), &query.kinds)
}

/// The locale predicate for a listing of `kind` or `kinds` (both empty: every type).
pub fn kinds_locale_filter(kind: Option<&str>, kinds: &[String]) -> LocaleFilter {
    match kind {
        Some(MEDIA_KIND) => LocaleFilter::None,
        Some(_) => LocaleFilter::Plain,
        None if kinds.is_empty() => LocaleFilter::ExceptMedia,
        None if kinds.iter().all(|k| k == MEDIA_KIND) => LocaleFilter::None,
        None if kinds.iter().any(|k| k == MEDIA_KIND) => LocaleFilter::ExceptMedia,
        None => LocaleFilter::Plain,
    }
}

/// `Invalid` for more than [`MAX_GROUPS`] groups in one call.
pub fn check_groups(n: usize) -> Result<(), StoreError> {
    if n > MAX_GROUPS {
        return Err(StoreError::Invalid(format!(
            "at most {MAX_GROUPS} translation groups per call"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_is_never_filtered() {
        let q = |kind: Option<&str>, kinds: &[&str]| ContentQuery {
            kind: kind.map(str::to_owned),
            kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
            locale: Some("de".into()),
            ..Default::default()
        };
        assert_eq!(locale_filter(&q(Some("post"), &[])), LocaleFilter::Plain);
        assert_eq!(locale_filter(&q(Some("media"), &[])), LocaleFilter::None);
        assert_eq!(locale_filter(&q(None, &[])), LocaleFilter::ExceptMedia);
        assert_eq!(
            locale_filter(&q(None, &["post", "page"])),
            LocaleFilter::Plain
        );
        assert_eq!(
            locale_filter(&q(None, &["post", "media"])),
            LocaleFilter::ExceptMedia
        );
        assert_eq!(locale_filter(&q(None, &["media"])), LocaleFilter::None);
        assert_eq!(locale_filter(&ContentQuery::default()), LocaleFilter::None);
    }
}
