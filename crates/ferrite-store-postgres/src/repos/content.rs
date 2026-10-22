//! Content types, content items and meta.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentQuery, ContentType, NewContent, Page,
    ParentFilter, Revision, RevisionId, Timestamp,
};
use ferrite_store::{
    ContentRepo, ContentTypeRepo, MetaRepo, RevisionRepo, SortKey, StoreError, check_limit,
    content_cursor, encode_content_cursor, validate_json, validate_text,
};
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, types::Json};

use super::PgRepos;
use crate::{
    codec::{self, CONTENT_COLUMNS, REVISION_COLUMNS, TYPE_COLUMNS},
    exec::run,
};

fn dt(t: Timestamp) -> DateTime<Utc> {
    t.into()
}

fn micros_dt(micros: i64) -> Result<DateTime<Utc>, StoreError> {
    Ok(dt(Timestamp::from_micros(micros)?))
}

#[async_trait]
impl ContentTypeRepo for PgRepos {
    async fn upsert(&self, ty: &ContentType) -> Result<(), StoreError> {
        validate_text("content type name", &ty.name)?;
        validate_text("content type label", &ty.label)?;
        validate_json(&ty.fields)?;
        // `built_in` and `created_at` are fixed when the type is first inserted.
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO content_types \
             (name, label, hierarchical, has_archive, fields, built_in, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (name) DO UPDATE SET label = excluded.label, \
             hierarchical = excluded.hierarchical, has_archive = excluded.has_archive, \
             fields = excluded.fields"
        )
        .bind(&ty.name)
        .bind(&ty.label)
        .bind(ty.hierarchical)
        .bind(ty.has_archive)
        .bind(Json(&ty.fields))
        .bind(ty.built_in)
        .bind(dt(ty.created_at))
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, name: &str) -> Result<Option<ContentType>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TYPE_COLUMNS} FROM content_types WHERE name = $1"
        )))
        .bind(name)
        .fetch_optional(e))?;
        row.as_ref().map(codec::content_type).transpose()
    }

    async fn list(&self) -> Result<Vec<ContentType>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TYPE_COLUMNS} FROM content_types ORDER BY name COLLATE \"C\""
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::content_type).collect()
    }

    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        let built_in: Option<bool> = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT built_in FROM content_types WHERE name = $1"
        )
        .bind(name)
        .fetch_optional(e))?;
        if built_in == Some(true) {
            return Err(StoreError::Invalid(format!(
                "built-in content type {name} cannot be deleted"
            )));
        }
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM content_types WHERE name = $1 AND NOT built_in"
        )
        .bind(name)
        .execute(e))
        .map(|_| ())
    }
}

#[async_trait]
impl ContentRepo for PgRepos {
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
        let now = dt(Timestamp::now());
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO content_items (id, type, status, slug, parent_id, author_id, title, \
             excerpt, body, body_html, fields, menu_order, published_at, scheduled_at, \
             created_at, updated_at, version) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $15, 1) \
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
        .bind(Json(&item.body))
        .bind(&item.body_html)
        .bind(Json(&item.fields))
        .bind(item.menu_order)
        .bind(item.published_at.map(dt))
        .bind(item.scheduled_at.map(dt))
        .bind(now)
        .fetch_one(e))?;
        codec::content_item(&row)
    }

    async fn get(&self, id: ContentId) -> Result<Option<ContentItem>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items WHERE id = $1"
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
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items \
             WHERE type = $1 AND parent_id IS NOT DISTINCT FROM $2 AND slug = $3"
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
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE content_items SET type = $1, status = $2, slug = $3, parent_id = $4, \
             author_id = $5, title = $6, excerpt = $7, body = $8, body_html = $9, fields = $10, \
             menu_order = $11, published_at = $12, scheduled_at = $13, updated_at = $14, \
             version = version + 1 \
             WHERE id = $15 AND version = $16 RETURNING {CONTENT_COLUMNS}"
        )))
        .bind(&item.kind)
        .bind(item.status.as_str())
        .bind(&item.slug)
        .bind(item.parent_id.map(|p| p.as_uuid()))
        .bind(item.author_id.as_uuid())
        .bind(&item.title)
        .bind(&item.excerpt)
        .bind(Json(&item.body))
        .bind(&item.body_html)
        .bind(Json(&item.fields))
        .bind(item.menu_order)
        .bind(item.published_at.map(dt))
        .bind(item.scheduled_at.map(dt))
        .bind(dt(Timestamp::now()))
        .bind(item.id.as_uuid())
        .bind(item.version)
        .fetch_optional(e))?;
        if let Some(row) = row {
            return codec::content_item(&row);
        }
        let current: Option<i64> = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT version FROM content_items WHERE id = $1"
        )
        .bind(item.id.as_uuid())
        .fetch_optional(e))?;
        match current {
            Some(current_version) => Err(StoreError::Conflict { current_version }),
            None => Err(StoreError::NotFound),
        }
    }

    async fn list(&self, query: &ContentQuery) -> Result<Page<ContentItem>, StoreError> {
        let limit = check_limit(query.limit)?;
        let after = query
            .cursor
            .as_deref()
            .map(|c| content_cursor(query.order, c))
            .transpose()?;

        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items WHERE TRUE"
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
            ContentOrder::TitleAsc => " ORDER BY title COLLATE \"C\" ASC, id ASC",
        });
        // One extra row tells whether another page follows.
        qb.push(" LIMIT ").push_bind(i64::from(limit) + 1);

        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
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
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM content_items WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn due_scheduled(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> Result<Vec<ContentItem>, StoreError> {
        let limit = check_limit(limit)?;
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items \
             WHERE status = 'scheduled' AND scheduled_at <= $1 \
             ORDER BY scheduled_at ASC, id ASC LIMIT $2"
        )))
        .bind(dt(now))
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(codec::content_item).collect()
    }
}

/// Appends the "strictly after the cursor" predicate for `order` (keyset pagination).
/// `published_at` sorts NULLs last, so a NULL cursor key only admits other NULL rows.
/// Titles compare with `COLLATE "C"` to match the ORDER BY (and SQLite's BINARY).
fn push_keyset(
    qb: &mut QueryBuilder<Postgres>,
    order: ContentOrder,
    key: SortKey,
    id: uuid::Uuid,
) -> Result<(), StoreError> {
    match (order, key) {
        (ContentOrder::PublishedDesc, SortKey::Micros(Some(k))) => {
            let k = micros_dt(k)?;
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
            let k = micros_dt(k)?;
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
            qb.push(" AND (title COLLATE \"C\" > ")
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
impl MetaRepo for PgRepos {
    async fn get(&self, id: ContentId, key: &str) -> Result<Option<Value>, StoreError> {
        let value: Option<Json<Value>> = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT value FROM content_meta WHERE item_id = $1 AND key = $2"
        )
        .bind(id.as_uuid())
        .bind(key)
        .fetch_optional(e))?;
        Ok(value.map(|v| v.0))
    }

    async fn set(&self, id: ContentId, key: &str, value: &Value) -> Result<(), StoreError> {
        validate_text("meta key", key)?;
        validate_json(value)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO content_meta (item_id, key, value) VALUES ($1, $2, $3) \
             ON CONFLICT (item_id, key) DO UPDATE SET value = excluded.value"
        )
        .bind(id.as_uuid())
        .bind(key)
        .bind(Json(value))
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, id: ContentId, key: &str) -> Result<(), StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM content_meta WHERE item_id = $1 AND key = $2"
        )
        .bind(id.as_uuid())
        .bind(key)
        .execute(e))
        .map(|_| ())
    }

    async fn list(&self, id: ContentId) -> Result<Vec<(String, Value)>, StoreError> {
        let rows: Vec<(String, Json<Value>)> = run!(&self.exec, |e| sqlx::query_as(
            "SELECT key, value FROM content_meta WHERE item_id = $1 ORDER BY key COLLATE \"C\""
        )
        .bind(id.as_uuid())
        .fetch_all(e))?;
        Ok(rows.into_iter().map(|(k, v)| (k, v.0)).collect())
    }
}

#[async_trait]
impl RevisionRepo for PgRepos {
    async fn insert(&self, revision: &Revision) -> Result<(), StoreError> {
        validate_text("revision title", &revision.title)?;
        validate_text("revision excerpt", &revision.excerpt)?;
        if let Some(note) = &revision.note {
            validate_text("revision note", note)?;
        }
        validate_json(&revision.body)?;
        validate_json(&revision.fields)?;
        validate_json(&revision.meta)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO revisions \
             (id, item_id, author_id, created_at, title, excerpt, body, fields, meta, note) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
        )
        .bind(revision.id.as_uuid())
        .bind(revision.item_id.as_uuid())
        .bind(revision.author_id.as_uuid())
        .bind(dt(revision.created_at))
        .bind(&revision.title)
        .bind(&revision.excerpt)
        .bind(Json(&revision.body))
        .bind(Json(&revision.fields))
        .bind(Json(&revision.meta))
        .bind(&revision.note)
        .execute(e))
        .map(|_| ())
    }

    async fn list(&self, id: ContentId, limit: u32) -> Result<Vec<Revision>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM revisions WHERE item_id = $1 \
             ORDER BY created_at DESC, id DESC LIMIT $2"
        )))
        .bind(id.as_uuid())
        .bind(i64::from(limit))
        .fetch_all(e))?;
        rows.iter().map(codec::revision).collect()
    }

    async fn get(&self, id: RevisionId) -> Result<Option<Revision>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM revisions WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::revision).transpose()
    }

    async fn prune(&self, id: ContentId, keep: u32) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM revisions WHERE item_id = $1 AND id NOT IN \
             (SELECT id FROM revisions WHERE item_id = $1 \
              ORDER BY created_at DESC, id DESC LIMIT $2)"
        )
        .bind(id.as_uuid())
        .bind(i64::from(keep))
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
