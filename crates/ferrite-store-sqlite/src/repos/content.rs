//! Content types, content items and meta.

use async_trait::async_trait;
use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentQuery, ContentType, NewContent, Page,
    ParentFilter, Revision, RevisionId, Timestamp,
};
use ferrite_store::{
    ContentRepo, ContentTypeRepo, MetaRepo, RevisionRepo, SortKey, StoreError, check_limit,
    content_cursor, encode_content_cursor, validate_json, validate_text,
};
use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite};

use super::SqliteRepos;
use crate::{
    codec::{self, CONTENT_COLUMNS, REVISION_COLUMNS, TYPE_COLUMNS},
    exec::{atomic, map_db_err, run},
};

#[async_trait]
impl ContentTypeRepo for SqliteRepos {
    async fn upsert(&self, ty: &ContentType) -> Result<(), StoreError> {
        validate_text("content type name", &ty.name)?;
        validate_text("content type label", &ty.label)?;
        validate_json(&ty.fields)?;
        // `built_in` and `created_at` are fixed when the type is first inserted.
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO content_types \
             (name, label, hierarchical, has_archive, fields, built_in, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (name) DO UPDATE SET label = excluded.label, \
             hierarchical = excluded.hierarchical, has_archive = excluded.has_archive, \
             fields = excluded.fields"
        )
        .bind(&ty.name)
        .bind(&ty.label)
        .bind(ty.hierarchical)
        .bind(ty.has_archive)
        .bind(codec::json_to_db(&ty.fields)?)
        .bind(ty.built_in)
        .bind(ty.created_at.as_micros())
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, name: &str) -> Result<Option<ContentType>, StoreError> {
        validate_text("content type name", name)?;
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TYPE_COLUMNS} FROM content_types WHERE name = ?"
        )))
        .bind(name)
        .fetch_optional(e))?;
        row.as_ref().map(codec::content_type).transpose()
    }

    async fn list(&self) -> Result<Vec<ContentType>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TYPE_COLUMNS} FROM content_types ORDER BY name"
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::content_type).collect()
    }

    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        validate_text("content type name", name)?;
        let built_in: Option<bool> = run!(&self.writer, |e| sqlx::query_scalar(
            "SELECT built_in FROM content_types WHERE name = ?"
        )
        .bind(name)
        .fetch_optional(e))?;
        if built_in == Some(true) {
            return Err(StoreError::Invalid(format!(
                "built-in content type {name} cannot be deleted"
            )));
        }
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM content_types WHERE name = ? AND built_in = 0"
        )
        .bind(name)
        .execute(e))
        .map(|_| ())
    }
}

#[async_trait]
impl ContentRepo for SqliteRepos {
    async fn insert(&self, item: &NewContent) -> Result<ContentItem, StoreError> {
        validate_text("content kind", &item.kind)?;
        validate_text("content slug", &item.slug)?;
        validate_text("content title", &item.title)?;
        validate_text("content excerpt", &item.excerpt)?;
        if let Some(html) = &item.body_html {
            validate_text("content body_html", html)?;
        }
        validate_json(&item.body)?;
        validate_json(&item.fields)?;
        let now = Timestamp::now().as_micros();
        let row = run!(&self.writer, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO content_items (id, type, status, slug, parent_id, author_id, title, \
             excerpt, body, body_html, fields, menu_order, published_at, scheduled_at, \
             created_at, updated_at, version) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1) \
             RETURNING {CONTENT_COLUMNS}"
        )))
        .bind(item.id.as_uuid())
        .bind(&item.kind)
        .bind(item.status.as_str())
        .bind(&item.slug)
        .bind(item.parent_id.map(|p| p.as_uuid()))
        .bind(item.author_id.as_uuid())
        .bind(&item.title)
        .bind(&item.excerpt)
        .bind(codec::json_to_db(&item.body)?)
        .bind(&item.body_html)
        .bind(codec::json_to_db(&item.fields)?)
        .bind(item.menu_order)
        .bind(item.published_at.map(|t| t.as_micros()))
        .bind(item.scheduled_at.map(|t| t.as_micros()))
        .bind(now)
        .bind(now)
        .fetch_one(e))?;
        codec::content_item(&row)
    }

    async fn get(&self, id: ContentId) -> Result<Option<ContentItem>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::content_item).transpose()
    }

    async fn get_by_slug(
        &self,
        kind: &str,
        parent: Option<ContentId>,
        slug: &str,
    ) -> Result<Option<ContentItem>, StoreError> {
        validate_text("content kind", kind)?;
        validate_text("content slug", slug)?;
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items \
             WHERE type = ? AND parent_id IS ? AND slug = ?"
        )))
        .bind(kind)
        .bind(parent.map(|p| p.as_uuid()))
        .bind(slug)
        .fetch_optional(e))?;
        row.as_ref().map(codec::content_item).transpose()
    }

    async fn update(&self, item: &ContentItem) -> Result<ContentItem, StoreError> {
        validate_text("content kind", &item.kind)?;
        validate_text("content slug", &item.slug)?;
        validate_text("content title", &item.title)?;
        validate_text("content excerpt", &item.excerpt)?;
        if let Some(html) = &item.body_html {
            validate_text("content body_html", html)?;
        }
        validate_json(&item.body)?;
        validate_json(&item.fields)?;
        let row = run!(&self.writer, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE content_items SET type = ?, status = ?, slug = ?, parent_id = ?, \
             author_id = ?, title = ?, excerpt = ?, body = ?, body_html = ?, fields = ?, \
             menu_order = ?, published_at = ?, scheduled_at = ?, updated_at = ?, \
             version = version + 1 \
             WHERE id = ? AND version = ? RETURNING {CONTENT_COLUMNS}"
        )))
        .bind(&item.kind)
        .bind(item.status.as_str())
        .bind(&item.slug)
        .bind(item.parent_id.map(|p| p.as_uuid()))
        .bind(item.author_id.as_uuid())
        .bind(&item.title)
        .bind(&item.excerpt)
        .bind(codec::json_to_db(&item.body)?)
        .bind(&item.body_html)
        .bind(codec::json_to_db(&item.fields)?)
        .bind(item.menu_order)
        .bind(item.published_at.map(|t| t.as_micros()))
        .bind(item.scheduled_at.map(|t| t.as_micros()))
        .bind(Timestamp::now().as_micros())
        .bind(item.id.as_uuid())
        .bind(item.version)
        .fetch_optional(e))?;
        if let Some(row) = row {
            return codec::content_item(&row);
        }
        let current: Option<i64> = run!(&self.writer, |e| sqlx::query_scalar(
            "SELECT version FROM content_items WHERE id = ?"
        )
        .bind(item.id.as_uuid())
        .fetch_optional(e))?;
        match current {
            Some(current_version) => Err(StoreError::Conflict { current_version }),
            None => Err(StoreError::NotFound),
        }
    }

    async fn list(&self, query: &ContentQuery) -> Result<Page<ContentItem>, StoreError> {
        if let Some(kind) = &query.kind {
            validate_text("content kind", kind)?;
        }
        let limit = check_limit(query.limit)?;
        let after = query
            .cursor
            .as_deref()
            .map(|c| content_cursor(query.order, c))
            .transpose()?;

        let mut qb = QueryBuilder::<Sqlite>::new(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items WHERE 1 = 1"
        ));
        if let Some(kind) = &query.kind {
            qb.push(" AND type = ").push_bind(kind.clone());
        }
        if !query.statuses.is_empty() {
            qb.push(" AND status IN (");
            let mut list = qb.separated(", ");
            for status in &query.statuses {
                list.push_bind(status.as_str());
            }
            list.push_unseparated(")");
        }
        match query.parent {
            ParentFilter::Any => {}
            ParentFilter::Root => {
                qb.push(" AND parent_id IS NULL");
            }
            ParentFilter::Of(id) => {
                qb.push(" AND parent_id = ").push_bind(id.as_uuid());
            }
        }
        if let Some(author) = query.author {
            qb.push(" AND author_id = ").push_bind(author.as_uuid());
        }
        if let Some(term) = query.term {
            qb.push(" AND id IN (SELECT item_id FROM term_links WHERE term_id = ")
                .push_bind(term.as_uuid())
                .push(")");
        }
        if let Some((key, id)) = after {
            push_keyset(&mut qb, query.order, key, id)?;
        }
        qb.push(match query.order {
            ContentOrder::PublishedDesc => " ORDER BY published_at DESC NULLS LAST, id DESC",
            ContentOrder::UpdatedDesc => " ORDER BY updated_at DESC, id DESC",
            ContentOrder::MenuOrderAsc => " ORDER BY menu_order ASC, id ASC",
            ContentOrder::TitleAsc => " ORDER BY title ASC, id ASC",
        });
        // One extra row tells whether another page follows.
        qb.push(" LIMIT ").push_bind(i64::from(limit) + 1);

        let rows = run!(&self.reader, |e| qb.build().fetch_all(e))?;
        let mut items = rows
            .iter()
            .map(codec::content_item)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|i| encode_content_cursor(query.order, i))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn delete(&self, id: ContentId) -> Result<(), StoreError> {
        // FTS5 rows have no foreign key, so drop the search row in the same batch; if
        // the item cannot be deleted (children RESTRICT) the search row is kept too.
        atomic!(&self.writer, |conn| async {
            sqlx::query("DELETE FROM search_fts WHERE item_id = ?")
                .bind(id.as_uuid())
                .execute(&mut *conn)
                .await
                .map_err(map_db_err)?;
            sqlx::query("DELETE FROM content_items WHERE id = ?")
                .bind(id.as_uuid())
                .execute(&mut *conn)
                .await
                .map_err(map_db_err)?;
            Ok(())
        })
    }

    async fn due_scheduled(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> Result<Vec<ContentItem>, StoreError> {
        let limit = check_limit(limit)?;
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items \
             WHERE status = 'scheduled' AND scheduled_at <= ? \
             ORDER BY scheduled_at ASC, id ASC LIMIT ?"
        )))
        .bind(now.as_micros())
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(codec::content_item).collect()
    }
}

/// Appends the "strictly after the cursor" predicate for `order` (keyset pagination).
/// `published_at` sorts NULLs last, so a NULL cursor key only admits other NULL rows.
fn push_keyset(
    qb: &mut QueryBuilder<Sqlite>,
    order: ContentOrder,
    key: SortKey,
    id: uuid::Uuid,
) -> Result<(), StoreError> {
    match (order, key) {
        (ContentOrder::PublishedDesc, SortKey::Micros(Some(k))) => {
            qb.push(" AND (published_at < ")
                .push_bind(k)
                .push(" OR (published_at = ")
                .push_bind(k)
                .push(" AND id < ")
                .push_bind(id)
                .push(") OR published_at IS NULL)");
        }
        (ContentOrder::PublishedDesc, SortKey::Micros(None)) => {
            qb.push(" AND published_at IS NULL AND id < ").push_bind(id);
        }
        (ContentOrder::UpdatedDesc, SortKey::Micros(Some(k))) => {
            qb.push(" AND (updated_at < ")
                .push_bind(k)
                .push(" OR (updated_at = ")
                .push_bind(k)
                .push(" AND id < ")
                .push_bind(id)
                .push("))");
        }
        (ContentOrder::MenuOrderAsc, SortKey::Int(k)) => {
            qb.push(" AND (menu_order > ")
                .push_bind(k)
                .push(" OR (menu_order = ")
                .push_bind(k)
                .push(" AND id > ")
                .push_bind(id)
                .push("))");
        }
        (ContentOrder::TitleAsc, SortKey::Text(k)) => {
            qb.push(" AND (title > ")
                .push_bind(k.clone())
                .push(" OR (title = ")
                .push_bind(k)
                .push(" AND id > ")
                .push_bind(id)
                .push("))");
        }
        _ => return Err(StoreError::Invalid("cursor does not match order".into())),
    }
    Ok(())
}

#[async_trait]
impl MetaRepo for SqliteRepos {
    async fn get(&self, id: ContentId, key: &str) -> Result<Option<Value>, StoreError> {
        validate_text("meta key", key)?;
        let text: Option<String> = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT value FROM content_meta WHERE item_id = ? AND key = ?"
        )
        .bind(id.as_uuid())
        .bind(key)
        .fetch_optional(e))?;
        text.as_deref().map(codec::json_from_db).transpose()
    }

    async fn set(&self, id: ContentId, key: &str, value: &Value) -> Result<(), StoreError> {
        validate_text("meta key", key)?;
        validate_json(value)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO content_meta (item_id, key, value) VALUES (?, ?, ?) \
             ON CONFLICT (item_id, key) DO UPDATE SET value = excluded.value"
        )
        .bind(id.as_uuid())
        .bind(key)
        .bind(codec::json_to_db(value)?)
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, id: ContentId, key: &str) -> Result<(), StoreError> {
        validate_text("meta key", key)?;
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM content_meta WHERE item_id = ? AND key = ?"
        )
        .bind(id.as_uuid())
        .bind(key)
        .execute(e))
        .map(|_| ())
    }

    async fn list(&self, id: ContentId) -> Result<Vec<(String, Value)>, StoreError> {
        let rows: Vec<(String, String)> = run!(&self.reader, |e| sqlx::query_as(
            "SELECT key, value FROM content_meta WHERE item_id = ? ORDER BY key"
        )
        .bind(id.as_uuid())
        .fetch_all(e))?;
        rows.into_iter()
            .map(|(k, v)| Ok((k, codec::json_from_db(&v)?)))
            .collect()
    }
}

#[async_trait]
impl RevisionRepo for SqliteRepos {
    async fn insert(&self, revision: &Revision) -> Result<(), StoreError> {
        validate_text("revision title", &revision.title)?;
        validate_text("revision excerpt", &revision.excerpt)?;
        if let Some(note) = &revision.note {
            validate_text("revision note", note)?;
        }
        validate_json(&revision.body)?;
        validate_json(&revision.fields)?;
        validate_json(&revision.meta)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO revisions \
             (id, item_id, author_id, created_at, title, excerpt, body, fields, meta, note) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(revision.id.as_uuid())
        .bind(revision.item_id.as_uuid())
        .bind(revision.author_id.as_uuid())
        .bind(revision.created_at.as_micros())
        .bind(&revision.title)
        .bind(&revision.excerpt)
        .bind(codec::json_to_db(&revision.body)?)
        .bind(codec::json_to_db(&revision.fields)?)
        .bind(codec::json_to_db(&revision.meta)?)
        .bind(&revision.note)
        .execute(e))
        .map(|_| ())
    }

    async fn list(&self, id: ContentId, limit: u32) -> Result<Vec<Revision>, StoreError> {
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM revisions WHERE item_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        )))
        .bind(id.as_uuid())
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(codec::revision).collect()
    }

    async fn get(&self, id: RevisionId) -> Result<Option<Revision>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM revisions WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::revision).transpose()
    }

    async fn prune(&self, id: ContentId, keep: u32) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM revisions WHERE item_id = ? AND id NOT IN \
             (SELECT id FROM revisions WHERE item_id = ? \
              ORDER BY created_at DESC, id DESC LIMIT ?)"
        )
        .bind(id.as_uuid())
        .bind(id.as_uuid())
        .bind(i64::from(keep))
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
