//! Content types: register, change and remove them, with their field schemas (P18).
//!
//! Every write needs `manage_settings` and runs in one transaction that also rebuilds the
//! permalink rules from the `permalink.*` options with the new set of type and taxonomy
//! names ([`permalink::rules_from`]), so a name that a structure's first segment would
//! shadow is refused (P19). It emits `types.changed` (`type:<name>`) plus
//! `invalidate [all]` and appends an audit row (`type.create`, `type.update`,
//! `type.delete`). Reads need `require_read(Content)` (F16).
//!
//! **Names** (types and taxonomies alike, [`check_name`]): `[a-z][a-z0-9_-]{0,31}`, not a
//! [`slug::RESERVED`] or [`slug::ROUTE_WORDS`] word, not four digits, not the name of an
//! existing type or taxonomy, and not the slug of an existing top-level post or page
//! (their paths start with `/<slug>/`, which the new `/<name>/` prefix would shadow, F26).
//!
//! **Concurrency** (M1c-1 P2): these checks read committed rows inside the transaction.
//! On SQLite writes are serialised; on Postgres two concurrent creates of the same name
//! both pass and the later upsert wins (it then behaves like an update), and a type and
//! a taxonomy created with the same name at the same moment can both succeed. Type
//! registration is a rare admin action, so this is accepted and documented rather than
//! locked; the outbox events and audit rows show what happened. A content write racing
//! a type delete fails on the foreign key (`409`).

use std::sync::Arc;

use ferrite_domain::{ContentQuery, ContentType, NewAudit, Timestamp, UserId};
use ferrite_store::{Repos, Store};
use serde_json::{Value, json};

use crate::{
    Actor, CoreError,
    actor::{Area, Capability, require, require_read},
    events,
    fields::{self, FieldDef},
    permalink, slug,
    tx::finish,
};

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
    pub fields: Vec<FieldDef>,
}

/// A change to a content type; `None` keeps the stored value. The name and
/// `hierarchical` cannot change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypePatch {
    pub label: Option<String>,
    pub has_archive: Option<bool>,
    /// Replaces the schema. Stored items are not re-validated; each is checked against
    /// the new schema on its next save.
    pub fields: Option<Vec<FieldDef>>,
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
    /// module docs (`InvalidField { field: "name", .. }`), the label is 1..=100
    /// characters after trimming, and the schema the rules of [`fields`]. The stored
    /// schema is `{"fields": [ … ]}`, also when empty, so items of a registered type are
    /// always validated.
    pub async fn create(&self, actor: &Actor, new: NewType) -> Result<ContentType, CoreError> {
        require(actor, Capability::ManageSettings)?;
        check_type_name(&new.name)?;
        let label = check_label(&new.label)?;
        fields::check_schema(&new.fields)?;
        let ty = ContentType {
            name: new.name,
            label,
            hierarchical: new.hierarchical,
            has_archive: new.has_archive,
            fields: fields::schema_value(&new.fields),
            built_in: false,
            created_at: Timestamp::now(),
        };
        let tx = self.store.begin().await?;
        let done = async {
            check_name_free(tx.as_ref(), &ty.name).await?;
            tx.types().upsert(&ty).await?;
            check_permalinks(tx.as_ref()).await?;
            let details = json!({
                "label": ty.label,
                "hierarchical": ty.hierarchical,
                "has_archive": ty.has_archive,
                "fields": new.fields.len(),
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
        finish(tx, done).await
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
        if let Some(defs) = &patch.fields {
            fields::check_schema(defs)?;
        }
        let tx = self.store.begin().await?;
        let done = async {
            let mut ty = tx.types().get(name).await?.ok_or(CoreError::NotFound)?;
            if let Some(label) = label {
                ty.label = label;
            }
            if let Some(has_archive) = patch.has_archive {
                ty.has_archive = has_archive;
            }
            if let Some(defs) = &patch.fields {
                ty.fields = fields::schema_value(defs);
            }
            tx.types().upsert(&ty).await?;
            check_permalinks(tx.as_ref()).await?;
            let details = json!({
                "label": patch.label.is_some(),
                "has_archive": patch.has_archive,
                "fields": patch.fields.as_ref().map(Vec::len),
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
        finish(tx, done).await
    }

    /// Removes type `name` (needs `manage_settings`). Built-in types are
    /// `Invalid("built-in type")`, a type with items in any status (trash included)
    /// `Invalid("type has content")`, one a taxonomy is attached to `Invalid` naming the
    /// taxonomy. Missing: `NotFound`.
    pub async fn delete(&self, actor: &Actor, name: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageSettings)?;
        let tx = self.store.begin().await?;
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
        finish(tx, done).await
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
}

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
/// inside the write's transaction).
pub(crate) async fn check_name_free(repos: &dyn Repos, name: &str) -> Result<(), CoreError> {
    if repos.types().get(name).await?.is_some() {
        return Err(CoreError::invalid_field(NAME, "name is a content type"));
    }
    if repos.taxonomies().get(name).await?.is_some() {
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

/// The permalink rules still hold with the names `repos` lists now (P18/P19); a clash
/// is reported on `name`.
pub(crate) async fn check_permalinks(repos: &dyn Repos) -> Result<(), CoreError> {
    match permalink::rules_from(repos, &[]).await {
        Ok(_) => Ok(()),
        Err(CoreError::Invalid(message)) => Err(CoreError::InvalidField {
            field: NAME,
            message,
        }),
        Err(e) => Err(e),
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
