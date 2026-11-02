//! The site's locales (design 13 §2.2): read from the `site.locales` option, settled at
//! startup, and checked when an admin changes them.
//!
//! The database is the authority (D3): [`settle`] writes `site.locales` once from the
//! old `site.locale` option (or the config) and heals rows an older binary wrote (a
//! blank locale, no translation group). [`check_change`] is the in-transaction check of a
//! `site.locales` write: prefixes that would shadow a route, a type, a taxonomy or a
//! top-level page are refused, and a locale that still holds content cannot be removed.

use ferrite_domain::locale::{
    LOCALE_TEXTS_OPTION, LOCALES_OPTION, LocaleDef, SiteLocales, bcp47_loose,
};
use ferrite_store::{LocaleUsage, Repos, Store};
use serde_json::{Map, Value};

use crate::{
    CoreError, events,
    permalink::PermalinkRules,
    settings::{MAX_DESCRIPTION, MAX_TITLE},
    slug,
    tx::finish,
};

/// The option holding the one-locale site's language (`<html lang>`, feeds).
pub const SITE_LOCALE_OPTION: &str = "site.locale";

/// The site's locales: the `site.locales` option, else (a database `settle` has not
/// seen yet, or an unreadable value) one locale from the `site.locale` option, else
/// `en`.
pub async fn read(repos: &dyn Repos) -> Result<SiteLocales, CoreError> {
    let options = repos.options();
    if let Some(entry) = options.get(LOCALES_OPTION).await? {
        match SiteLocales::from_option(&entry.value) {
            Ok(locales) => return Ok(locales),
            Err(e) => tracing::warn!(error = %e, "site.locales is unreadable; using site.locale"),
        }
    }
    let locale = options
        .get(SITE_LOCALE_OPTION)
        .await?
        .and_then(|e| e.value.as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(SiteLocales::single(bcp47_loose(&locale)))
}

/// What [`settle`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettleReport {
    /// `site.locales` was absent and is now written (with this default tag).
    pub wrote_option: Option<String>,
    /// Content items and terms that got their translation group.
    pub groups: u64,
    /// Rows relabelled to the default locale.
    pub relabelled: LocaleUsage,
}

impl SettleReport {
    /// Whether anything changed.
    pub fn changed(&self) -> bool {
        self.wrote_option.is_some() || self.groups > 0 || !self.relabelled.is_empty()
    }
}

/// Settles the locales at startup (13 §2.2), in one transaction; the caller holds the
/// migrate lock, so concurrent starts settle once:
///
/// 1. `site.locales` absent: write `[{ tag }]` with `bcp47_loose` of the `site.locale`
///    option, else of `config_locale`;
/// 2. give content items and terms without a translation group their own id;
/// 3. one locale: relabel every row whose locale is not the default; several: relabel
///    only blank ones;
/// 4. when anything changed, `invalidate [all]` in the same transaction.
pub async fn settle(store: &dyn Store, config_locale: &str) -> Result<SettleReport, CoreError> {
    let tx = store.begin().await?;
    let done = settle_in(tx.as_ref(), config_locale).await;
    finish(tx, done).await
}

async fn settle_in(repos: &dyn Repos, config_locale: &str) -> Result<SettleReport, CoreError> {
    let mut report = SettleReport::default();
    let options = repos.options();
    if options.get(LOCALES_OPTION).await?.is_none() {
        let old = options
            .get(SITE_LOCALE_OPTION)
            .await?
            .and_then(|e| e.value.as_str().map(str::to_owned));
        let tag = bcp47_loose(old.as_deref().unwrap_or(config_locale));
        let value = Value::Array(vec![serde_json::json!({ "tag": tag })]);
        options.set(LOCALES_OPTION, &value, true).await?;
        repos
            .events()
            .append(&events::options_changed(LOCALES_OPTION))
            .await?;
        report.wrote_option = Some(tag);
    }
    let locales = read(repos).await?;
    report.groups = repos.locales().fill_groups().await?;
    report.relabelled = if locales.is_multilingual() {
        repos.locales().relabel_blank(&locales.default.tag).await?
    } else {
        repos.locales().relabel_all(&locales.default.tag).await?
    };
    if report.changed() {
        crate::types::invalidate_all(repos).await?;
    }
    Ok(report)
}

/// How a `site.locales` write changes the list ([`check_change`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// The same tags in the same order (labels may differ).
    Unchanged,
    /// One locale to one other: every row is relabelled.
    Rename { from: String, to: String },
    /// The same tags in another order (a new default): no row changes, URLs move.
    Reorder,
    /// Tags added (the order of the old ones may change).
    Grow,
    /// These tags removed (others may be added).
    Shrink(Vec<String>),
}

/// The in-transaction check of a `site.locales` write from `old` to `new` (13 §2.2):
/// every new prefix must not equal a content type or taxonomy name, a reserved or
/// route word, four digits, or the slug of a top-level default-locale item of a type
/// whose top-level items live at `/<slug>/` (`InvalidField { field: "site.locales" }`
/// naming the clash); a removed tag must hold no content item, term or menu
/// ([`CoreError::InUse`] with the counts). One locale replaced by another is a
/// [`Transition::Rename`].
pub async fn check_change(
    repos: &dyn Repos,
    old: &SiteLocales,
    new: &SiteLocales,
    rules: &PermalinkRules,
) -> Result<Transition, CoreError> {
    let bad = |m: String| CoreError::invalid_field(LOCALES_OPTION, m);
    let types: Vec<String> = repos
        .types()
        .list()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let taxonomies: Vec<String> = repos
        .taxonomies()
        .list()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    for def in &new.others {
        if old.by_prefix(&def.prefix()).is_some() {
            // Already a prefix: whatever it shadows was refused when it was added.
            continue;
        }
        let p = def.prefix();
        let hit = |name: &String| name.eq_ignore_ascii_case(&p);
        let what = if types.iter().any(hit) {
            Some("a content type")
        } else if taxonomies.iter().any(hit) {
            Some("a taxonomy")
        } else if slug::RESERVED.iter().any(|w| w.eq_ignore_ascii_case(&p))
            || slug::ROUTE_WORDS.iter().any(|w| w.eq_ignore_ascii_case(&p))
        {
            Some("a reserved word")
        } else if slug::is_year(&p) {
            Some("a year archive")
        } else {
            None
        };
        if let Some(what) = what {
            return Err(bad(format!(
                "the locale prefix /{p}/ is {what}; choose another locale"
            )));
        }
        for kind in rules.root_slug_kinds() {
            if let Some(item) = repos
                .content()
                .get_by_slug(kind, Some(&new.default.tag), None, &p)
                .await?
            {
                return Err(bad(format!(
                    "the locale prefix /{p}/ is the address of the {kind} {:?} ({}); change \
                     its slug first",
                    item.title, item.id
                )));
            }
        }
    }
    let old_tags: Vec<&str> = old.all().map(|d| d.tag.as_str()).collect();
    let new_tags: Vec<&str> = new.all().map(|d| d.tag.as_str()).collect();
    if old_tags.len() == 1 && new_tags.len() == 1 && old_tags != new_tags {
        return Ok(Transition::Rename {
            from: old_tags[0].to_owned(),
            to: new_tags[0].to_owned(),
        });
    }
    let removed: Vec<String> = old_tags
        .iter()
        .filter(|t| !new_tags.contains(t))
        .map(|t| (*t).to_owned())
        .collect();
    for tag in &removed {
        let usage = repos.locales().usage(tag).await?;
        if !usage.is_empty() {
            return Err(CoreError::InUse(format!(
                "the locale {tag} still holds {} content item(s), {} term(s) and {} menu(s); \
                 delete or move them first",
                usage.items, usage.terms, usage.menus
            )));
        }
    }
    if !removed.is_empty() {
        return Ok(Transition::Shrink(removed));
    }
    if new_tags.len() > old_tags.len() {
        return Ok(Transition::Grow);
    }
    if new_tags != old_tags {
        return Ok(Transition::Reorder);
    }
    Ok(Transition::Unchanged)
}

/// The site title and tagline of one locale (`site.locale_texts[tag]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocaleText {
    pub title: Option<String>,
    pub description: Option<String>,
}

/// The texts of `tag` in the `site.locale_texts` option, if it has any.
pub async fn texts(repos: &dyn Repos, tag: &str) -> Result<Option<LocaleText>, CoreError> {
    let Some(entry) = repos.options().get(LOCALE_TEXTS_OPTION).await? else {
        return Ok(None);
    };
    Ok(texts_of(&entry.value, tag))
}

/// The texts of `tag` in a `site.locale_texts` value.
pub fn texts_of(value: &Value, tag: &str) -> Option<LocaleText> {
    let entry = value.get(tag)?.as_object()?;
    let text = |k: &str| {
        entry
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    };
    Some(LocaleText {
        title: text("title"),
        description: text("description"),
    })
}

/// Checks a `site.locale_texts` value without the store: an object keyed by locale tags
/// (strict), each `{ "title"?, "description"? }` validated like `site.title` and
/// `site.description`. That the keys are listed locales is checked in the write.
pub fn validate_texts(value: &Value) -> Result<(), String> {
    let Some(map) = value.as_object() else {
        return Err("site.locale_texts must be an object keyed by locale".into());
    };
    if map.len() > ferrite_domain::locale::MAX_LOCALES {
        return Err("site.locale_texts holds too many locales".into());
    }
    for (tag, entry) in map {
        let canonical = ferrite_domain::locale::parse_tag(tag).map_err(|e| e.to_string())?;
        if &canonical != tag {
            return Err(format!(
                "site.locale_texts: write the locale as {canonical}"
            ));
        }
        let Some(fields) = entry.as_object() else {
            return Err(format!("site.locale_texts.{tag} must be an object"));
        };
        for (k, v) in fields {
            let (max, multiline) = match k.as_str() {
                "title" => (MAX_TITLE, false),
                "description" => (MAX_DESCRIPTION, true),
                other => {
                    return Err(format!("site.locale_texts.{tag} has no field {other:?}"));
                }
            };
            if v.is_null() {
                continue;
            }
            let Some(s) = v.as_str() else {
                return Err(format!("site.locale_texts.{tag}.{k} must be a string"));
            };
            let control =
                |c: char| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t'));
            if s.chars().count() > max || s.chars().any(control) {
                return Err(format!(
                    "site.locale_texts.{tag}.{k} must be at most {max} characters without \
                     control characters"
                ));
            }
        }
    }
    Ok(())
}

/// `texts` without the entries of locales `locales` does not list (or of its default).
pub fn texts_for(texts: &Value, locales: &SiteLocales) -> Value {
    let mut out = Map::new();
    if let Some(map) = texts.as_object() {
        for (tag, entry) in map {
            if locales.others.iter().any(|d: &LocaleDef| &d.tag == tag) {
                out.insert(tag.clone(), entry.clone());
            }
        }
    }
    Value::Object(out)
}
