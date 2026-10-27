//! Content types: register, change and remove them, with their field schemas (P18).
//!
//! Every write needs `manage_settings` and runs in one transaction that also rebuilds the
//! permalink rules from the `permalink.*` options with the new set of type and taxonomy
//! names ([`permalink::rules_from`]), so a name that a structure's first segment would
//! shadow is refused (P19). It emits `types.changed` (`type:<name>`) plus
//! `invalidate [all]` and appends an audit row (`type.create`, `type.update`,
//! `type.delete`). Reads need `require_read(Content)` (F16).
//!
//! **Names** (types and taxonomies alike, [`check_type_name`]): `[a-z][a-z0-9_-]{0,31}`,
//! not a [`slug::RESERVED`], [`slug::ROUTE_WORDS`] or [`API_WORDS`] word, not four digits, not the name
//! of an existing type or taxonomy, and not the slug of an existing top-level post or
//! page (their paths start with `/<slug>/`, which the new `/<name>/` prefix would
//! shadow, F26). A name its own registry already holds is a conflict
//! (`Store(UniqueViolation { "content_types_pkey" | "taxonomies_pkey" })`, `409` with
//! `errors[0].field = name`); one the other registry holds, or a post/page slug, is
//! `InvalidField { field: "name", .. }`.
//!
//! **Concurrency.** Every type and taxonomy create, update and delete runs under the
//! named store lock [`LOCK_TYPES`] (Postgres: the advisory lock `ferrite:<schema>:types`,
//! cluster-wide; SQLite: in process), taken before the transaction and released after
//! it ends, waiting at most [`TYPES_LOCK_WAIT`] (then `Store(Busy)`, `503`). So the name
//! checks above, which span two tables no constraint links, see every earlier
//! registration, and a create is a plain insert: the primary key backs the conflict
//! check up. A content write racing a type delete fails on the foreign key (`409`).
//!
//! **Permalink check errors.** The structures are first parsed from the stored options;
//! a stored option that no longer parses is not the request's fault and is plain
//! `Invalid` naming the option. A first segment that shadows a name is
//! `InvalidField { field: "name", .. }` on create (the new name causes it) and plain
//! `Invalid` on update and delete (the body changes no name, so the stored state already
//! clashed).

use std::{sync::Arc, time::Duration};

use ferrite_domain::{ContentQuery, ContentType, NewAudit, Timestamp, UserId};
use ferrite_store::{LOCK_TYPES, LockGuard, Repos, Store, StoreError, Tx, lock};
use serde_json::{Value, json};

use crate::{
    Actor, CoreError,
    actor::{Area, Capability, require, require_read},
    events,
    fields::{self, FieldDef},
    permalink, slug,
    tx::finish,
};

/// How long a type or taxonomy write waits for the [`LOCK_TYPES`] lock.
pub const TYPES_LOCK_WAIT: Duration = Duration::from_secs(30);
/// Longest type or taxonomy name.
pub const MAX_TYPE_NAME: usize = 32;
/// Longest type or taxonomy label (characters).
pub const MAX_TYPE_LABEL: usize = 100;

/// The input member name errors are reported on.
const NAME: &str = "name";
/// The types whose top-level items live at `/<slug>/` (F26).
const ROOT_TYPES: [&str; 2] = ["post", "page"];

/// A new content type ([`ContentTypeService::create`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewType {
    pub name: String,
    pub label: String,
    pub hierarchical: bool,
    pub has_archive: bool,
    /// The schema items are validated against; `None` registers the type without one
    /// (any `fields` object, like the built-in types).
    pub fields: Option<Vec<FieldDef>>,
}

/// A change to a content type; `None` keeps the stored value. The name and
/// `hierarchical` cannot change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypePatch {
    pub label: Option<String>,
    pub has_archive: Option<bool>,
    /// `Some(Some(defs))` replaces the schema (`Some(vec![])`: items take `{}` only),
    /// `Some(None)` removes it (any object). Stored items are not re-validated; each is
    /// checked on its next save.
    pub fields: Option<Option<Vec<FieldDef>>>,
}

/// Registers and manages content types (see the module docs).
pub struct ContentTypeService {
    store: Arc<dyn Store>,
}

impl ContentTypeService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Every content type, by name (`require_read(Content)`).
    pub async fn list(&self, actor: &Actor) -> Result<Vec<ContentType>, CoreError> {
        require_read(actor, Area::Content)?;
        Ok(self.store.read().types().list().await?)
    }

    /// Content type `name` (`require_read(Content)`; missing: `NotFound`).
    pub async fn get(&self, actor: &Actor, name: &str) -> Result<ContentType, CoreError> {
        require_read(actor, Area::Content)?;
        self.store
            .read()
            .types()
            .get(name)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// Registers a type (needs `manage_settings`). The name follows the rules in the
    /// module docs, the label is 1..=100 characters after trimming, and the schema the
    /// rules of [`fields`]. The stored schema is `{"fields": [ … ]}` (also when empty),
    /// or `{}` for a type without one.
    pub async fn create(&self, actor: &Actor, new: NewType) -> Result<ContentType, CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_type_name(&new.name)?;
        let label = check_label(&new.label)?;
        if let Some(defs) = &new.fields {
            fields::check_schema(defs)?;
        }
        let ty = ContentType {
            name: new.name,
            label,
            hierarchical: new.hierarchical,
            has_archive: new.has_archive,
            fields: stored_schema(new.fields.as_deref()),
            built_in: false,
            created_at: Timestamp::now(),
        };
        let (guard, tx) = begin_locked(self.store.as_ref()).await?;
        let done = async {
            check_name_free(tx.as_ref(), &ty.name, Change::Type).await?;
            if let Some(defs) = &new.fields {
                check_targets(tx.as_ref(), defs, Some(&ty.name)).await?;
            }
            tx.types().insert(&ty).await?;
            check_permalinks(tx.as_ref(), true).await?;
            let details = json!({
                "label": ty.label,
                "hierarchical": ty.hierarchical,
                "has_archive": ty.has_archive,
                "fields": new.fields.as_ref().map(Vec::len),
            });
            announce(
                tx.as_ref(),
                actor.user,
                Change::Type,
                "create",
                &ty.name,
                details,
            )
            .await?;
            Ok(ty)
        }
        .await;
        finish_locked(guard, tx, done).await
    }

    /// Changes the label, `has_archive` or schema of type `name` (needs
    /// `manage_settings`; built-in types too, e.g. to give `post` a schema). Missing:
    /// `NotFound`.
    pub async fn update(
        &self,
        actor: &Actor,
        name: &str,
        patch: TypePatch,
    ) -> Result<ContentType, CoreError> {
        require(actor, Capability::ManageSettings)?;
        let label = patch.label.as_deref().map(check_label).transpose()?;
        if let Some(Some(defs)) = &patch.fields {
            fields::check_schema(defs)?;
        }
        let (guard, tx) = begin_locked(self.store.as_ref()).await?;
        let done = async {
            let mut ty = tx.types().get(name).await?.ok_or(CoreError::NotFound)?;
            if let Some(label) = label {
                ty.label = label;
            }
            if let Some(has_archive) = patch.has_archive {
                ty.has_archive = has_archive;
            }
            if let Some(defs) = &patch.fields {
                if let Some(defs) = defs {
                    check_targets(tx.as_ref(), defs, None).await?;
                }
                ty.fields = stored_schema(defs.as_deref());
            }
            tx.types().upsert(&ty).await?;
            check_permalinks(tx.as_ref(), false).await?;
            let details = json!({
                "label": patch.label.is_some(),
                "has_archive": patch.has_archive,
                "fields": patch.fields.as_ref().map(|f| f.as_ref().map(Vec::len)),
            });
            announce(
                tx.as_ref(),
                actor.user,
                Change::Type,
                "update",
                &ty.name,
                details,
            )
            .await?;
            Ok(ty)
        }
        .await;
        finish_locked(guard, tx, done).await
    }

    /// Removes type `name` (needs `manage_settings`). Built-in types are
    /// `Invalid("built-in type")`, a type with items in any status (trash included)
    /// `Invalid("type has content")`, one another type's relation field lists in its
    /// `types` `Invalid` naming that type, one a taxonomy is attached to `Invalid`
    /// naming the taxonomy. Missing: `NotFound`.
    pub async fn delete(&self, actor: &Actor, name: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        let (guard, tx) = begin_locked(self.store.as_ref()).await?;
        let done = async {
            let ty = tx.types().get(name).await?.ok_or(CoreError::NotFound)?;
            if ty.built_in {
                return Err(CoreError::Invalid("built-in type".into()));
            }
            let items = tx
                .content()
                .list(&ContentQuery {
                    kind: Some(ty.name.clone()),
                    limit: 1,
                    ..ContentQuery::default()
                })
                .await?;
            if !items.items.is_empty() {
                return Err(CoreError::Invalid("type has content".into()));
            }
            if let Some(other) = tx.types().list().await?.into_iter().find(|t| {
                t.name != ty.name
                    && fields::schema_of(&t.fields)
                        .ok()
                        .flatten()
                        .is_some_and(|defs| fields::names_type(&defs, &ty.name))
            }) {
                return Err(CoreError::Invalid(format!(
                    "type is a relation target of type {:?}",
                    other.name
                )));
            }
            if let Some(tax) = tx
                .taxonomies()
                .list()
                .await?
                .into_iter()
                .find(|t| t.content_types.contains(&ty.name))
            {
                return Err(CoreError::Invalid(format!(
                    "type is used by taxonomy {:?}",
                    tax.name
                )));
            }
            tx.types().delete(&ty.name).await?;
            announce(
                tx.as_ref(),
                actor.user,
                Change::Type,
                "delete",
                &ty.name,
                json!({}),
            )
            .await
        }
        .await;
        finish_locked(guard, tx, done).await
    }
}

/// [`fields::check_relation_types`] of `defs` against the registered types, plus
/// `own`, the name of a type being created (a relation may point at its own type).
async fn check_targets(
    repos: &dyn Repos,
    defs: &[FieldDef],
    own: Option<&str>,
) -> Result<(), CoreError> {
    let types = repos.types().list().await?;
    let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).chain(own).collect();
    fields::check_relation_types(defs, &names)
}

/// `{"fields": [ … ]}` for a schema, `{}` for none.
fn stored_schema(defs: Option<&[FieldDef]>) -> Value {
    match defs {
        Some(defs) => fields::schema_value(defs),
        None => json!({}),
    }
}

/// Takes the [`LOCK_TYPES`] lock (bounded by [`TYPES_LOCK_WAIT`]), then opens the write
/// transaction. [`finish_locked`] ends both, the transaction first, so the next holder
/// sees the committed rows.
pub(crate) async fn begin_locked(
    store: &dyn Store,
) -> Result<(Box<dyn LockGuard>, Box<dyn Tx>), CoreError> {
    let guard = lock(store, LOCK_TYPES, TYPES_LOCK_WAIT).await?;
    match store.begin().await {
        Ok(tx) => Ok((guard, tx)),
        Err(e) => {
            release(guard).await;
            Err(e.into())
        }
    }
}

/// Commits `tx` on `Ok` (rolls back on `Err`), then releases the lock.
pub(crate) async fn finish_locked<T>(
    guard: Box<dyn LockGuard>,
    tx: Box<dyn Tx>,
    result: Result<T, CoreError>,
) -> Result<T, CoreError> {
    let done = finish(tx, result).await;
    release(guard).await;
    done
}

/// Releases the lock; a failure is logged (the lock goes away with its connection).
async fn release(guard: Box<dyn LockGuard>) {
    if let Err(e) = guard.release().await {
        tracing::warn!(error = %e, lock = LOCK_TYPES, "releasing the lock failed");
    }
}

/// Which registry a write changed.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Change {
    Type,
    Taxonomy,
}

impl Change {
    fn word(self) -> &'static str {
        match self {
            Change::Type => "type",
            Change::Taxonomy => "taxonomy",
        }
    }

    /// The primary key of the registry's table.
    fn pkey(self) -> &'static str {
        match self {
            Change::Type => "content_types_pkey",
            Change::Taxonomy => "taxonomies_pkey",
        }
    }
}

/// Names the REST API uses as static segments beside `/content/{type}`
/// (`GET /content/refs`), so no type (nor, sharing the rules, taxonomy) may take them.
pub const API_WORDS: &[&str] = &["refs"];

/// The pure name rules of the module docs (`InvalidField { field: "name", .. }`).
pub(crate) fn check_type_name(name: &str) -> Result<(), CoreError> {
    let mut bytes = name.bytes();
    let shaped = matches!(bytes.next(), Some(b'a'..=b'z'))
        && name.len() <= MAX_TYPE_NAME
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'));
    if !shaped {
        return Err(CoreError::invalid_field(
            NAME,
            "name must match [a-z][a-z0-9_-]{0,31}",
        ));
    }
    if slug::is_reserved(name) || slug::ROUTE_WORDS.contains(&name) || slug::is_year(name) {
        return Err(CoreError::invalid_field(
            NAME,
            "name is reserved by the router",
        ));
    }
    if API_WORDS.contains(&name) {
        return Err(CoreError::invalid_field(
            NAME,
            "name is reserved by the API",
        ));
    }
    Ok(())
}

/// 1..=100 characters after trimming, no control characters.
pub(crate) fn check_label(label: &str) -> Result<String, CoreError> {
    let label = label.trim();
    let len = label.chars().count();
    if len == 0 || len > MAX_TYPE_LABEL || label.chars().any(char::is_control) {
        return Err(CoreError::invalid_field(
            "label",
            format!("label must be 1 to {MAX_TYPE_LABEL} characters without control characters"),
        ));
    }
    Ok(label.to_owned())
}

/// `name` is not a type, a taxonomy or the slug of a top-level post or page (read
/// inside the write's transaction, under [`LOCK_TYPES`]). Taken in `own` registry: the
/// unique violation its insert would raise; elsewhere: `InvalidField("name")`.
pub(crate) async fn check_name_free(
    repos: &dyn Repos,
    name: &str,
    own: Change,
) -> Result<(), CoreError> {
    let is_type = repos.types().get(name).await?.is_some();
    let is_taxonomy = repos.taxonomies().get(name).await?.is_some();
    let taken_here = match own {
        Change::Type => is_type,
        Change::Taxonomy => is_taxonomy,
    };
    if taken_here {
        return Err(CoreError::Store(StoreError::UniqueViolation {
            constraint: own.pkey().to_owned(),
        }));
    }
    if is_type {
        return Err(CoreError::invalid_field(NAME, "name is a content type"));
    }
    if is_taxonomy {
        return Err(CoreError::invalid_field(NAME, "name is a taxonomy"));
    }
    for kind in ROOT_TYPES {
        if repos
            .content()
            .get_by_slug(kind, None, name)
            .await?
            .is_some()
        {
            return Err(CoreError::invalid_field(
                NAME,
                "name used by an existing item",
            ));
        }
    }
    Ok(())
}

/// The permalink rules still hold with the names `repos` lists now (P18/P19). See
/// "Permalink check errors" in the module docs: `blame_name` (a create) reports a clash
/// on `name`; stored options that do not parse are plain `Invalid`.
pub(crate) async fn check_permalinks(repos: &dyn Repos, blame_name: bool) -> Result<(), CoreError> {
    let rules = permalink::stored_rules(repos, &[])
        .await
        .map_err(|e| match e {
            CoreError::Invalid(m) => CoreError::Invalid(format!(
                "the stored permalink options are invalid, fix them first: {m}"
            )),
            e => e,
        })?;
    match permalink::check_registered(repos, &rules).await {
        Err(CoreError::Invalid(message)) if blame_name => Err(CoreError::InvalidField {
            field: NAME,
            message,
        }),
        other => other,
    }
}

/// `types.changed` for `name`, `invalidate [all]` and the audit row
/// `<type|taxonomy>.<verb>`.
pub(crate) async fn announce(
    repos: &dyn Repos,
    actor: UserId,
    change: Change,
    verb: &str,
    name: &str,
    details: Value,
) -> Result<(), CoreError> {
    let key = match change {
        Change::Type => events::type_key(name),
        Change::Taxonomy => events::taxonomy_key(name),
    };
    let events_repo = repos.events();
    events_repo.append(&events::types_changed(&key)).await?;
    events_repo
        .append(&events::invalidate([events::ALL_KEY.to_owned()]))
        .await?;
    repos
        .audit()
        .append(&NewAudit {
            actor_id: Some(actor),
            action: format!("{}.{verb}", change.word()),
            target_kind: change.word().to_owned(),
            target_id: Some(name.to_owned()),
            details,
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules() {
        for ok in ["event", "a", "b2", "my_type", "my-type", &"a".repeat(32)] {
            assert!(check_type_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Event",
            "1a",
            "_a",
            "-a",
            "a b",
            "ä",
            &"a".repeat(33),
            "admin",
            "api",
            "feed",
            "author",
            "page",
            "search",
            "refs",
        ] {
            assert!(
                matches!(
                    check_type_name(bad),
                    Err(CoreError::InvalidField { field: "name", .. })
                ),
                "{bad:?}"
            );
        }
        assert!(check_label("  Events ").is_ok_and(|l| l == "Events"));
        assert!(check_label(" ").is_err());
        assert!(check_label(&"x".repeat(101)).is_err());
        assert!(check_label("a\nb").is_err());
    }
}
