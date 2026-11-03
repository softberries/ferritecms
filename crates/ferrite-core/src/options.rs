//! Site option writes (the admin's settings and the low-level option API).

use std::sync::Arc;

use ferrite_domain::{
    NewAudit,
    locale::{LOCALE_TEXTS_OPTION, LOCALES_OPTION, SiteLocales, bcp47_loose},
};
use ferrite_store::{Repos, Store};
use serde_json::{Value, json};

use crate::{
    Actor, CoreError,
    actor::{Capability, require},
    events,
    jobs::{
        PERMALINK_REDIRECTS_VERSION, PermalinkRedirects, PermalinkStructures,
        permalink_redirects_job,
    },
    locales::{self, SITE_LOCALE_OPTION},
    permalink::{self, PAGE_OPTION, POST_OPTION, PermalinkRules},
    settings,
    site::FRONT_PAGE_OPTION,
    tx::finish,
    types::{begin_locked, finish_locked, invalidate_all},
};

const MAX_KEY_LEN: usize = 200;

/// One pending write: `value` `None` deletes the option.
struct Change<'a> {
    key: &'a str,
    value: Option<&'a Value>,
    autoload: bool,
}

/// Writes options (`manage_settings`). Reads go to the store (`OptionRepo`, or
/// [`settings::read_all`] for the admin's settings): there is no per-node cache yet. The
/// autoload cache with its `options.changed` refresh on every node (R13) was removed in
/// R26 because nothing read it; it comes back with its first reader (M2a-2).
///
/// **`permalink.*` writes** ([`set`](Self::set), [`delete`](Self::delete),
/// [`set_many`](Self::set_many)) take the [`LOCK_TYPES`](ferrite_store::LOCK_TYPES)
/// lock that type and taxonomy writes take (R19) and check the resulting pair of
/// structures against the registered types and taxonomies inside the transaction
/// ([`permalink::stored_rules`] + [`permalink::check_registered`], P19), so neither
/// side can race the other into a combination the router cannot serve. A clash is
/// `InvalidField { field: "permalink.post" | "permalink.page", .. }` naming the
/// structure. They emit `invalidate [all]` (R10) besides `options.changed`; each node's
/// reloader rebuilds its permalink rules and route table from the event. A write that
/// changes the structures in effect also enqueues a
/// [`permalink.redirects`](crate::jobs::PERMALINK_REDIRECTS_KIND) job in its
/// transaction (M3-2 P30), which redirects the published posts and pages from their old
/// paths.
pub struct OptionService {
    store: Arc<dyn Store>,
}

impl OptionService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Sets an option and emits `options.changed` with `option:<key>` in the same
    /// transaction. `permalink.*` values are validated (see the type docs); other keys are
    /// stored as given (this is the low-level option API; the admin's settings go through
    /// [`set_many`](Self::set_many)).
    pub async fn set(
        &self,
        actor: &Actor,
        key: &str,
        value: &Value,
        autoload: bool,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_key(key)?;
        if is_permalink(key) || is_locale_key(key) {
            settings::validate(key, value)?;
        }
        self.write(
            actor,
            &[Change {
                key,
                value: Some(value),
                autoload,
            }],
            false,
        )
        .await
    }

    /// Deletes an option (a missing one is a no-op) and emits `options.changed`.
    pub async fn delete(&self, actor: &Actor, key: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_key(key)?;
        self.write(
            actor,
            &[Change {
                key,
                value: None,
                autoload: false,
            }],
            false,
        )
        .await
    }

    /// Writes allowlisted settings ([`settings::SETTINGS`]) in one transaction (P19):
    /// `(key, Some(value))` sets the key with `autoload = true` (every render reads
    /// them, F23), `(key, None)` or a JSON `null` deletes it (back to the default).
    ///
    /// Needs `manage_settings`. Every change is validated first ([`settings::validate`]:
    /// an unknown key or a key given twice is `Invalid`, a bad value
    /// `InvalidField { field: <key>, .. }`), then in the transaction: the front page
    /// must be an existing page, and the `permalink.*` pair (stored values with these
    /// changes on top) must fit the registered types and taxonomies. Any failure writes
    /// nothing and emits nothing. On success each key gets its own `options.changed`
    /// event, `permalink.*` changes add one `invalidate [all]`, and one audit row
    /// `settings.update` lists the keys (P22). An empty list is a no-op.
    pub async fn set_many(
        &self,
        actor: &Actor,
        changes: &[(String, Option<Value>)],
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        let pending = check_settings(changes)?;
        if pending.is_empty() {
            return Ok(());
        }
        self.write(actor, &pending, true).await
    }

    /// The one write path: [`write_changes`] in one transaction (under
    /// [`LOCK_TYPES`](ferrite_store::LOCK_TYPES) when a `permalink.*` key changes).
    async fn write(
        &self,
        actor: &Actor,
        changes: &[Change<'_>],
        settings_write: bool,
    ) -> Result<(), CoreError> {
        if changes
            .iter()
            .any(|c| is_permalink(c.key) || is_locale_key(c.key))
        {
            let (guard, tx) = begin_locked(self.store.as_ref()).await?;
            let done = write_with_invalidation(tx.as_ref(), actor, changes, settings_write).await;
            finish_locked(guard, tx, done).await
        } else {
            let tx = self.store.begin().await?;
            let done = write_with_invalidation(tx.as_ref(), actor, changes, settings_write).await;
            finish(tx, done).await
        }
    }
}

/// [`OptionService::set_many`] inside the caller's `Tx` (M3-1 P1), without the
/// capability check: the same validation, front-page and `permalink.*` checks, events
/// and audit row (and `permalink.redirects` job), except the `invalidate [all]`.
/// Returns whether a `permalink.*` key changed, so the caller emits that invalidation.
/// A `permalink.*` change needs a `Tx` that holds [`LOCK_TYPES`](ferrite_store::LOCK_TYPES) ([`touches_permalinks`]).
pub(crate) async fn set_many_in(
    repos: &dyn Repos,
    actor: &Actor,
    changes: &[(String, Option<Value>)],
) -> Result<bool, CoreError> {
    let pending = check_settings(changes)?;
    if pending.is_empty() {
        return Ok(false);
    }
    write_changes(repos, actor, &pending, true).await
}

/// [`OptionService::set`] inside the caller's `Tx` for a key that is not `permalink.*`
/// (those need the `types` lock), without the capability check: the same key check, row
/// and `options.changed` event.
pub(crate) async fn set_in(
    repos: &dyn Repos,
    actor: &Actor,
    key: &str,
    value: &Value,
    autoload: bool,
) -> Result<(), CoreError> {
    check_key(key)?;
    if is_permalink(key) || is_locale_key(key) {
        return Err(CoreError::Invalid(
            "permalink and locale options need a transaction under the types lock".into(),
        ));
    }
    write_changes(
        repos,
        actor,
        &[Change {
            key,
            value: Some(value),
            autoload,
        }],
        false,
    )
    .await?;
    Ok(())
}

/// Whether `changes` sets or deletes a `permalink.*` option or a locale option
/// (`site.locales`, `site.locale`, `site.locale_texts`), which need the types lock.
pub(crate) fn touches_permalinks(changes: &[(String, Option<Value>)]) -> bool {
    changes
        .iter()
        .any(|(key, _)| is_permalink(key) || is_locale_key(key))
}

/// The validation of [`OptionService::set_many`]: every key allowlisted and given once,
/// every value valid; `None` or JSON `null` deletes.
fn check_settings(changes: &[(String, Option<Value>)]) -> Result<Vec<Change<'_>>, CoreError> {
    let mut pending: Vec<Change<'_>> = Vec::with_capacity(changes.len());
    for (key, value) in changes {
        let value = value.as_ref().filter(|v| !v.is_null());
        match value {
            Some(v) => settings::validate(key, v)?,
            None if settings::def(key).is_none() => {
                return Err(CoreError::Invalid(format!("unknown setting {key:?}")));
            }
            None => {}
        }
        if pending.iter().any(|c| c.key == key) {
            return Err(CoreError::Invalid(format!("setting {key:?} given twice")));
        }
        pending.push(Change {
            key,
            value,
            autoload: true,
        });
    }
    Ok(pending)
}

/// [`write_changes`], then `invalidate [all]` when a `permalink.*` key changed.
async fn write_with_invalidation(
    repos: &dyn Repos,
    actor: &Actor,
    changes: &[Change<'_>],
    settings_write: bool,
) -> Result<(), CoreError> {
    if write_changes(repos, actor, changes, settings_write).await? {
        invalidate_all(repos).await?;
    }
    Ok(())
}

/// Inside the write's transaction: the checks that need the store (`permalink.*` pair;
/// for a `settings_write`, the front page), each set or delete with its
/// `options.changed`, the `permalink.redirects` job when the `permalink.*` structures
/// in effect changed (P30), and for a `settings_write` the audit row `settings.update`.
/// Returns whether a `permalink.*` key changed: the caller then owes `invalidate [all]`.
async fn write_changes(
    repos: &dyn Repos,
    actor: &Actor,
    changes: &[Change<'_>],
    settings_write: bool,
) -> Result<bool, CoreError> {
    let (extra, locales_changed) = plan_locales(repos, changes).await?;
    let extra: Vec<Change<'_>> = extra
        .iter()
        .map(|(key, value)| Change {
            key,
            value: Some(value),
            autoload: true,
        })
        .collect();
    // An extra write of a key replaces the caller's (the canonical `site.locales`).
    let all: Vec<&Change<'_>> = changes
        .iter()
        .filter(|c| !extra.iter().any(|e| e.key == c.key))
        .chain(extra.iter())
        .collect();
    let changed = write_plain(repos, actor, &all, settings_write).await?;
    Ok(changed || locales_changed)
}

/// [`write_changes`] for `changes` (the locale plan already applied): the store checks,
/// the rows and their events, the redirect job and the audit row.
async fn write_plain(
    repos: &dyn Repos,
    actor: &Actor,
    changes: &[&Change<'_>],
    settings_write: bool,
) -> Result<bool, CoreError> {
    let permalinks: Vec<&Change<'_>> = changes
        .iter()
        .copied()
        .filter(|c| is_permalink(c.key))
        .collect();
    // The structures before the write; stored values that no longer parse give no
    // old paths to redirect from.
    let mut prev = None;
    if !permalinks.is_empty() {
        check_permalinks(repos, &permalinks).await?;
        prev = match permalink::stored_rules(repos, &[]).await {
            Ok(rules) => Some(rules),
            Err(CoreError::Invalid(_)) => None,
            Err(e) => return Err(e),
        };
    }
    if settings_write {
        for c in changes {
            if c.key == FRONT_PAGE_OPTION
                && let Some(v) = c.value
            {
                settings::check_front_page(repos, v).await?;
            }
        }
    }
    let mut version = 0;
    for c in changes {
        match c.value {
            Some(v) => repos.options().set(c.key, v, c.autoload).await?,
            None => repos.options().delete(c.key).await?,
        }
        version = repos
            .events()
            .append(&events::options_changed(c.key))
            .await?;
    }
    if let Some(prev) = prev {
        enqueue_redirects(repos, &prev, version).await?;
    }
    if settings_write {
        let keys: Vec<&str> = changes.iter().map(|c| c.key).collect();
        repos
            .audit()
            .append(&NewAudit {
                actor_id: Some(actor.user),
                action: "settings.update".to_owned(),
                target_kind: "settings".to_owned(),
                target_id: None,
                details: json!({ "keys": keys }),
            })
            .await?;
    }
    Ok(!permalinks.is_empty())
}

/// Enqueues the [`permalink.redirects`](crate::jobs::PERMALINK_REDIRECTS_KIND) job when
/// the structures stored now (the write's pending changes) differ from `prev`; `version`
/// keys it (the write's last event sequence number).
async fn enqueue_redirects(
    repos: &dyn Repos,
    prev: &PermalinkRules,
    version: i64,
) -> Result<(), CoreError> {
    let next = permalink::stored_rules(repos, &[]).await?;
    if next.same_structures(prev) {
        return Ok(());
    }
    let structures = |r: &PermalinkRules| PermalinkStructures {
        post: r.post().as_str().to_owned(),
        page: r.page().as_str().to_owned(),
    };
    let job = permalink_redirects_job(&PermalinkRedirects {
        v: PERMALINK_REDIRECTS_VERSION,
        prev: structures(prev),
        next: structures(&next),
        version,
        cursor: None,
    });
    repos.jobs().enqueue(&job).await?;
    Ok(())
}

pub(crate) fn is_permalink(key: &str) -> bool {
    key == POST_OPTION || key == PAGE_OPTION
}

/// `site.locales`, `site.locale` and `site.locale_texts`: written with the checks and
/// couplings of [`plan_locales`].
pub(crate) fn is_locale_key(key: &str) -> bool {
    key == LOCALES_OPTION || key == SITE_LOCALE_OPTION || key == LOCALE_TEXTS_OPTION
}

/// The locale side of a write (13 §2.2), inside its transaction, before anything is
/// written: checks a `site.locales` change ([`locales::check_change`]: prefixes,
/// removals), keeps `site.locale` equal to the default (a `site.locale` write alone on a
/// one-locale site renames the locale; on a multilingual one it must equal the default),
/// relabels every row on a rename, and drops the `site.locale_texts` entries of
/// removed locales. Returns the extra option writes it owes (`site.locale`,
/// `site.locales`, `site.locale_texts`) and whether the locales changed (the caller then
/// owes `invalidate [all]`).
async fn plan_locales(
    repos: &dyn Repos,
    changes: &[Change<'_>],
) -> Result<(Vec<(&'static str, Value)>, bool), CoreError> {
    let find = |key: &str| changes.iter().find(|c| c.key == key);
    let list = find(LOCALES_OPTION);
    let single = find(SITE_LOCALE_OPTION);
    let texts = find(LOCALE_TEXTS_OPTION);
    if list.is_none() && single.is_none() && texts.is_none() {
        return Ok((Vec::new(), false));
    }
    let old = locales::read(repos).await?;
    let mut extra: Vec<(&'static str, Value)> = Vec::new();
    let new = match (list, single) {
        (Some(c), _) => {
            let Some(v) = c.value else {
                return Err(CoreError::invalid_field(
                    LOCALES_OPTION,
                    "site.locales cannot be cleared; list at least the default locale",
                ));
            };
            let new = SiteLocales::from_option(v)
                .map_err(|e| CoreError::invalid_field(LOCALES_OPTION, e.to_string()))?;
            match single.map(|c| c.value) {
                Some(Some(v)) if v.as_str() != Some(new.default.tag.as_str()) => {
                    return Err(CoreError::invalid_field(
                        SITE_LOCALE_OPTION,
                        format!(
                            "site.locale must equal the default locale {}",
                            new.default.tag
                        ),
                    ));
                }
                Some(_) => {}
                None => {
                    let stored = repos.options().get(SITE_LOCALE_OPTION).await?;
                    if stored.and_then(|e| e.value.as_str().map(str::to_owned))
                        != Some(new.default.tag.clone())
                    {
                        extra.push((SITE_LOCALE_OPTION, Value::String(new.default.tag.clone())));
                    }
                }
            }
            // Stored in canonical form (tags, labels filled in).
            extra.push((LOCALES_OPTION, new.to_option()));
            Some(new)
        }
        (None, Some(c)) => match c.value {
            None if old.is_multilingual() => {
                return Err(CoreError::invalid_field(
                    SITE_LOCALE_OPTION,
                    "on a multilingual site site.locale is the default locale; change site.locales",
                ));
            }
            None => None,
            Some(v) => {
                let tag = bcp47_loose(v.as_str().unwrap_or_default());
                if old.is_multilingual() {
                    if tag != old.default.tag {
                        return Err(CoreError::invalid_field(
                            SITE_LOCALE_OPTION,
                            format!(
                                "on a multilingual site site.locale must equal the default locale {}; change site.locales",
                                old.default.tag
                            ),
                        ));
                    }
                    None
                } else if tag == old.default.tag {
                    None
                } else {
                    let new = SiteLocales::single(tag);
                    extra.push((LOCALES_OPTION, new.to_option()));
                    Some(new)
                }
            }
        },
        (None, None) => None,
    };
    let mut changed = false;
    let effective = new.clone().unwrap_or_else(|| old.clone());
    if let Some(new) = &new
        && new != &old
    {
        changed = true;
        let rules = permalink::stored_rules(repos, &[]).await?;
        if let locales::Transition::Rename { to, .. } =
            locales::check_change(repos, &old, new, &rules).await?
        {
            repos.locales().relabel_all(&to).await?;
        } else if new.default.tag != old.default.tag {
            // Media belongs to every locale and carries the default (review fix: else a
            // later lossy down of 0014 would take the library with the old default).
            repos.locales().relabel_media(&new.default.tag).await?;
        }
    }
    // `site.locale_texts`: the written value's keys must be listed (non-default) locales;
    // a list change drops the entries of locales it no longer lists.
    match texts {
        Some(c) => {
            if let Some(v) = c.value
                && let Some(map) = v.as_object()
                && let Some(tag) = map
                    .keys()
                    .find(|t| !effective.others.iter().any(|d| &d.tag == *t))
            {
                return Err(CoreError::invalid_field(
                    LOCALE_TEXTS_OPTION,
                    format!("site.locale_texts: {tag} is not one of the site's other locales"),
                ));
            }
            changed = true;
        }
        None if new.is_some() => {
            if let Some(stored) = repos.options().get(LOCALE_TEXTS_OPTION).await? {
                let kept = locales::texts_for(&stored.value, &effective);
                if kept != stored.value {
                    extra.push((LOCALE_TEXTS_OPTION, kept));
                }
            }
        }
        None => {}
    }
    Ok((extra, changed))
}

/// The `permalink.*` pair with `changes` on top fits the registered types and
/// taxonomies (read through `repos`, the write's transaction). A clash blames the
/// changed key whose structure the message names (else the first changed key); stored
/// options that no longer parse are plain `Invalid`.
async fn check_permalinks(repos: &dyn Repos, changes: &[&Change<'_>]) -> Result<(), CoreError> {
    let overrides: Vec<(&str, Option<&str>)> = changes
        .iter()
        .map(|c| (c.key, c.value.and_then(Value::as_str)))
        .collect();
    let rules = permalink::stored_rules(repos, &overrides).await?;
    permalink::check_registered(repos, &rules)
        .await
        .map_err(|e| match e {
            CoreError::Invalid(message) => {
                let blamed = changes
                    .iter()
                    .find(|c| {
                        c.value
                            .and_then(Value::as_str)
                            .is_some_and(|s| message.contains(&format!("{s:?}")))
                    })
                    .or(changes.first());
                let field = match blamed.map(|c| c.key) {
                    Some(PAGE_OPTION) => PAGE_OPTION,
                    _ => POST_OPTION,
                };
                CoreError::InvalidField { field, message }
            }
            e => e,
        })
}

fn check_key(key: &str) -> Result<(), CoreError> {
    if key.is_empty() || key.len() > MAX_KEY_LEN || key.chars().any(char::is_control) {
        return Err(CoreError::Invalid(format!(
            "option key must be 1-{MAX_KEY_LEN} bytes without control characters"
        )));
    }
    Ok(())
}
