//! Backend-neutral constraint names for SQLite unique violations (I1).
//!
//! SQLite reports a unique violation as `UNIQUE constraint failed: t.a, t.b` (or
//! `index 'name'` for an expression index), never with the constraint's name. Postgres
//! reports the name. The migrations name every primary key `<table>_pkey` and every
//! unique index `<table>_<what>_uq`, and this table maps SQLite's column list to that
//! name, so `StoreError::UniqueViolation { constraint }` is identical on both backends.
//! A unit test checks the table covers every unique index in the migrated schema.

/// `t.col[, t.col...]` as SQLite reports it → the constraint name used on every backend.
const UNIQUE_NAMES: &[(&str, &str)] = &[
    ("api_tokens.id", "api_tokens_pkey"),
    ("api_tokens.token_hash", "api_tokens_hash_uq"),
    ("artifacts.hash", "artifacts_pkey"),
    ("audit_log.id", "audit_log_pkey"),
    ("autosaves.item_id, autosaves.user_id", "autosaves_pkey"),
    ("comments.id", "comments_pkey"),
    ("content_items.id", "content_items_pkey"),
    (
        "content_meta.item_id, content_meta.key",
        "content_meta_pkey",
    ),
    ("content_types.name", "content_types_pkey"),
    (
        "idempotency_keys.user_id, idempotency_keys.key_hash",
        "idempotency_keys_pkey",
    ),
    ("jobs.id", "jobs_pkey"),
    ("jobs.unique_key", "jobs_unique_key_uq"),
    ("media.item_id", "media_pkey"),
    (
        "media_variants.item_id, media_variants.name",
        "media_variants_pkey",
    ),
    ("menu_items.id", "menu_items_pkey"),
    ("menus.name", "menus_pkey"),
    ("nodes.id", "nodes_pkey"),
    ("options.key", "options_pkey"),
    (
        "plugin_failures.plugin, plugin_failures.hook, plugin_failures.bucket_start",
        "plugin_failures_pkey",
    ),
    ("plugin_kv.plugin, plugin_kv.key", "plugin_kv_pkey"),
    ("plugins.name", "plugins_pkey"),
    ("rate_limits.bucket", "rate_limits_pkey"),
    ("redirects.id", "redirects_pkey"),
    ("redirects.source_path", "redirects_source_path_uq"),
    ("revisions.id", "revisions_pkey"),
    ("schema_meta.key", "schema_meta_pkey"),
    // On Postgres `item_id` is the primary key of search_docs.
    ("search_docs.item_id", "search_docs_pkey"),
    ("sessions.id_hash", "sessions_pkey"),
    ("taxonomies.name", "taxonomies_pkey"),
    ("term_links.item_id, term_links.term_id", "term_links_pkey"),
    ("terms.id", "terms_pkey"),
    ("terms.taxonomy, terms.slug", "terms_taxonomy_slug_uq"),
    ("themes.active", "themes_one_active_uq"),
    ("themes.name", "themes_pkey"),
    ("user_meta.user_id, user_meta.key", "user_meta_pkey"),
    ("users.email_key", "users_email_uq"),
    ("users.id", "users_pkey"),
    ("users.login", "users_login_uq"),
];

/// The neutral name for the text after `UNIQUE constraint failed: `.
pub(crate) fn unique_name(detail: &str) -> String {
    if let Some(name) = detail
        .strip_prefix("index '")
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return name.to_owned();
    }
    match UNIQUE_NAMES.iter().find(|(cols, _)| *cols == detail) {
        Some((_, name)) => (*name).to_owned(),
        None => {
            tracing::warn!(detail, "unique violation on an unmapped SQLite index");
            detail.to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row;

    use super::*;
    use crate::{SqliteOptions, SqliteStore};

    #[test]
    fn maps_columns_and_expression_indexes() {
        assert_eq!(unique_name("users.login"), "users_login_uq");
        assert_eq!(
            unique_name("terms.taxonomy, terms.slug"),
            "terms_taxonomy_slug_uq"
        );
        assert_eq!(
            unique_name("index 'content_items_slug_uq'"),
            "content_items_slug_uq"
        );
    }

    /// Every unique index of the migrated schema (excluding FTS5 shadow tables and
    /// integer rowid keys, which the store never collides on) is in [`UNIQUE_NAMES`].
    #[tokio::test]
    async fn every_unique_index_is_mapped() {
        use ferrite_store::Store;

        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(SqliteOptions::new(dir.path().join("ferrite.db")))
            .await
            .unwrap();
        store.migrate_up(None).await.unwrap();
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_schema WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '_sqlx_%' \
             AND name NOT LIKE 'search_fts%'",
        )
        .fetch_all(&store.reader)
        .await
        .unwrap();
        let mut checked = 0;
        for table in tables {
            let indexes = sqlx::query("SELECT name, \"unique\" FROM pragma_index_list(?)")
                .bind(&table)
                .fetch_all(&store.reader)
                .await
                .unwrap();
            for index in indexes {
                let (name, unique): (String, bool) = (index.get(0), index.get(1));
                if !unique {
                    continue;
                }
                let cols: Vec<Option<String>> =
                    sqlx::query_scalar("SELECT name FROM pragma_index_info(?) ORDER BY seqno")
                        .bind(&name)
                        .fetch_all(&store.reader)
                        .await
                        .unwrap();
                let detail = if cols.iter().any(Option::is_none) {
                    format!("index '{name}'")
                } else {
                    cols.iter()
                        .map(|c| format!("{table}.{}", c.as_deref().unwrap()))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let mapped = unique_name(&detail);
                assert!(
                    mapped.ends_with("_pkey") || mapped.ends_with("_uq"),
                    "{table}: index {name} ({detail}) is not mapped"
                );
                checked += 1;
            }
        }
        assert!(checked >= 25, "only {checked} unique indexes found");
    }
}
