//! Content types, content items and meta.

use std::collections::HashMap;

use async_trait::async_trait;
use ferrite_domain::{
    ContentId, ContentItem, ContentQuery, ContentType, NewContent, Page, ParentFilter, Revision,
    RevisionId, Timestamp, UserId,
};
use ferrite_store::{
    ContentRepo, ContentTypeRepo, ListPart, MAX_ID_FILTER, MAX_META_KEYS, MAX_SLUG_PREFIX_RESULTS,
    MetaRepo, RevisionRepo, StoreError, check_content_filters, check_id_list, check_len,
    check_limit, content_cursor, content_list_plan, encode_content_cursor, media_kind_sql,
    slug_has_prefix, slug_prefix_bounds, validate_json, validate_text,
};
use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite};

use uuid::Uuid;

use super::SqliteRepos;
use crate::{
    codec::{self, CONTENT_COLUMNS, REVISION_COLUMNS, TYPE_COLUMNS},
    exec::run,
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

    async fn insert(&self, ty: &ContentType) -> Result<(), StoreError> {
        validate_text("content type name", &ty.name)?;
        validate_text("content type label", &ty.label)?;
        validate_json(&ty.fields)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO content_types \
             (name, label, hierarchical, has_archive, fields, built_in, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)"
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
        check_content_filters(query)?;
        let limit = check_limit(query.limit)?;
        let after = query
            .cursor
            .as_deref()
            .map(|c| content_cursor(query.order, c))
            .transpose()?;

        let plan = content_list_plan(query.order, after)?;
        // One extra row tells whether another page follows.
        let mut left = limit + 1;
        let mut rows = Vec::new();
        for part in plan {
            if left == 0 {
                break;
            }
            let mut qb = listing_base(query);
            push_part(&mut qb, part);
            qb.push(" LIMIT ").push_bind(i64::from(left));
            let batch = run!(&self.reader, |e| qb.build().fetch_all(e))?;
            left = left.saturating_sub(u32::try_from(batch.len()).unwrap_or(u32::MAX));
            rows.extend(batch);
        }
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

    async fn count(&self, query: &ContentQuery) -> Result<u64, StoreError> {
        check_content_filters(query)?;
        let mut qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM content_items WHERE 1 = 1");
        push_filters(&mut qb, query);
        let n: i64 = run!(&self.reader, |e| qb.build_query_scalar().fetch_one(e))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<ContentItem>, StoreError> {
        check_id_list("ids", ids)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Sqlite>::new(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items WHERE id IN ("
        ));
        let mut list = qb.separated(", ");
        for id in ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
        let rows = run!(&self.reader, |e| qb.build().fetch_all(e))?;
        let mut found = rows
            .iter()
            .map(codec::content_item)
            .map(|r| r.map(|i| (i.id, i)))
            .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
        Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
    }

    async fn slugs_with_prefix(
        &self,
        kind: &str,
        parent: Option<ContentId>,
        prefix: &str,
    ) -> Result<Vec<String>, StoreError> {
        validate_text("content kind", kind)?;
        validate_text("slug prefix", prefix)?;
        let (from, to) = slug_prefix_bounds(prefix);
        let slugs: Vec<String> = run!(&self.reader, |e| {
            sqlx::query_scalar(
            "SELECT slug FROM content_items WHERE type = ? AND parent_id IS ? AND (slug = ? OR (slug >= ? AND slug < ?)) \
             ORDER BY slug LIMIT ?"
        )
        .bind(kind)
        .bind(parent.map(|p| p.as_uuid()))
        .bind(prefix)
        .bind(&from)
        .bind(&to)
        .bind(i64::from(MAX_SLUG_PREFIX_RESULTS))
        .fetch_all(e)
        })?;
        Ok(slugs
            .into_iter()
            .filter(|s| slug_has_prefix(s, prefix))
            .collect())
    }

    async fn delete(&self, id: ContentId) -> Result<(), StoreError> {
        // Dependent rows cascade, the search row included (its triggers update FTS5).
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM content_items WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError> {
        // A new version, so a stale editor save is a Conflict rather than a write that
        // restores the old author; `updated_at` stays (the content did not change).
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE content_items SET author_id = ?1, version = version + 1 \
             WHERE author_id = ?2"
        )
        .bind(to.as_uuid())
        .bind(from.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected())
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

/// `SELECT ... WHERE <the query's filters>`, shared by every part of a listing.
fn listing_base(query: &ContentQuery) -> QueryBuilder<Sqlite> {
    let mut qb = QueryBuilder::<Sqlite>::new(format!(
        "SELECT {CONTENT_COLUMNS} FROM content_items WHERE 1 = 1"
    ));
    push_filters(&mut qb, query);
    qb
}

/// Appends the query's filters (everything but paging) to a `WHERE` clause; `list` and
/// `count` both use it so they cannot drift.
fn push_filters(qb: &mut QueryBuilder<Sqlite>, query: &ContentQuery) {
    if let Some(kind) = &query.kind {
        qb.push(" AND type = ").push_bind(kind.clone());
    }
    if !query.kinds.is_empty() {
        qb.push(" AND type IN (");
        let mut list = qb.separated(", ");
        for kind in &query.kinds {
            list.push_bind(kind.clone());
        }
        list.push_unseparated(")");
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
    if let Some(reader) = query.readable_by {
        // Publicly visible now (core's `is_public`), or the reader's own item.
        qb.push(" AND ((status = 'published' AND published_at <= ")
            .push_bind(reader.now.as_micros())
            .push(") OR author_id = ")
            .push_bind(reader.user.as_uuid())
            .push(")");
    }
    if let Some(term) = query.term {
        qb.push(" AND id IN (SELECT item_id FROM term_links WHERE term_id = ")
            .push_bind(term.as_uuid())
            .push(")");
    }
    if !query.ids.is_empty() {
        qb.push(" AND id IN (");
        let mut list = qb.separated(", ");
        for id in &query.ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
    }
    if !query.exclude.is_empty() {
        qb.push(" AND id NOT IN (");
        let mut list = qb.separated(", ");
        for id in &query.exclude {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
    }
    if let Some(t) = query.published_after {
        qb.push(" AND published_at >= ").push_bind(t.as_micros());
    }
    if let Some(t) = query.published_before {
        qb.push(" AND published_at < ").push_bind(t.as_micros());
    }
    if let Some(kind) = query.media_kind {
        qb.push(" AND id IN (SELECT item_id FROM media WHERE ")
            .push(media_kind_sql(kind))
            .push(")");
    }
}

/// Appends a listing part's keyset predicate (a row comparison, so the index range
/// starts at the cursor) and its ORDER BY, which matches a `content_items_*_list_idx`.
fn push_part(qb: &mut QueryBuilder<Sqlite>, part: ListPart) {
    match part {
        ListPart::Published(after) => {
            qb.push(" AND published_at IS NOT NULL");
            if let Some((k, id)) = after {
                qb.push(" AND (published_at, id) < (")
                    .push_bind(k)
                    .push(", ")
                    .push_bind(id)
                    .push(")");
            }
            qb.push(" ORDER BY published_at DESC, id DESC");
        }
        ListPart::Unpublished(after) => {
            qb.push(" AND published_at IS NULL");
            if let Some(id) = after {
                qb.push(" AND id < ").push_bind(id);
            }
            qb.push(" ORDER BY id DESC");
        }
        ListPart::Updated(after) => {
            if let Some((k, id)) = after {
                qb.push(" AND (updated_at, id) < (")
                    .push_bind(k)
                    .push(", ")
                    .push_bind(id)
                    .push(")");
            }
            qb.push(" ORDER BY updated_at DESC, id DESC");
        }
        ListPart::MenuOrder(after) => {
            if let Some((k, id)) = after {
                qb.push(" AND (menu_order, id) > (")
                    .push_bind(k)
                    .push(", ")
                    .push_bind(id)
                    .push(")");
            }
            qb.push(" ORDER BY menu_order ASC, id ASC");
        }
        ListPart::Title(after) => {
            if let Some((k, id)) = after {
                qb.push(" AND (title, id) > (")
                    .push_bind(k)
                    .push(", ")
                    .push_bind(id)
                    .push(")");
            }
            qb.push(" ORDER BY title ASC, id ASC");
        }
    }
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

    async fn get_many(
        &self,
        ids: &[ContentId],
        keys: &[&str],
    ) -> Result<Vec<(ContentId, String, Value)>, StoreError> {
        check_len("ids", ids.len(), MAX_ID_FILTER)?;
        check_len("keys", keys.len(), MAX_META_KEYS)?;
        for key in keys {
            validate_text("meta key", key)?;
        }
        if ids.is_empty() || keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Sqlite>::new(
            "SELECT item_id, key, value FROM content_meta WHERE item_id IN (",
        );
        let mut list = qb.separated(", ");
        for id in ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(") AND key IN (");
        let mut list = qb.separated(", ");
        for key in keys {
            list.push_bind(*key);
        }
        list.push_unseparated(")");
        let rows: Vec<(Uuid, String, String)> =
            run!(&self.reader, |e| qb.build_query_as().fetch_all(e))?;
        // Ordered in Rust, identically on both backends: input id order, then key bytes.
        let mut position = HashMap::new();
        for (i, id) in ids.iter().enumerate() {
            position.entry(id.as_uuid()).or_insert(i);
        }
        let mut out = Vec::with_capacity(rows.len());
        for (item, key, v) in rows {
            let at = position.get(&item).copied().unwrap_or(usize::MAX);
            out.push((
                at,
                ContentId::from_uuid(item),
                key,
                codec::json_from_db(&v)?,
            ));
        }
        out.sort_by(|a, b| (a.0, a.2.as_bytes()).cmp(&(b.0, b.2.as_bytes())));
        Ok(out.into_iter().map(|(_, id, k, v)| (id, k, v)).collect())
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
        let limit = check_limit(limit)?;
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

    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE revisions SET author_id = ?1 WHERE author_id = ?2"
        )
        .bind(to.as_uuid())
        .bind(from.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
