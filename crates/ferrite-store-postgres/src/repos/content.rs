//! Content types, content items and meta.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    ContentId, ContentItem, ContentQuery, ContentType, NewContent, Page, ParentFilter, Revision,
    RevisionId, Timestamp, UserId,
};
use ferrite_store::{
    ContentRepo, ContentTypeRepo, FileBound, ListPart, LocaleFilter, MAX_ID_FILTER, MAX_META_KEYS,
    MAX_SLUG_PREFIX_RESULTS, MetaRepo, RevisionRepo, StoreError, TranslationRef,
    check_content_filters, check_file_bounds, check_groups, check_id_list, check_len, check_limit,
    content_cursor, content_list_plan, encode_content_cursor, file_bounds_from_rows, locale_filter,
    media_kind_sql, slug_has_prefix, slug_prefix_bounds, validate_json, validate_text,
};
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, types::Json};

use uuid::Uuid;

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
             (name, label, hierarchical, has_archive, fields, built_in, created_at, quality_gate) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (name) DO UPDATE SET label = excluded.label, \
             hierarchical = excluded.hierarchical, has_archive = excluded.has_archive, \
             fields = excluded.fields, quality_gate = excluded.quality_gate"
        )
        .bind(&ty.name)
        .bind(&ty.label)
        .bind(ty.hierarchical)
        .bind(ty.has_archive)
        .bind(Json(&ty.fields))
        .bind(ty.built_in)
        .bind(dt(ty.created_at))
        .bind(ty.quality_gate.as_str())
        .execute(e))
        .map(|_| ())
    }

    async fn insert(&self, ty: &ContentType) -> Result<(), StoreError> {
        validate_text("content type name", &ty.name)?;
        validate_text("content type label", &ty.label)?;
        validate_json(&ty.fields)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO content_types \
             (name, label, hierarchical, has_archive, fields, built_in, created_at, quality_gate) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        )
        .bind(&ty.name)
        .bind(&ty.label)
        .bind(ty.hierarchical)
        .bind(ty.has_archive)
        .bind(Json(&ty.fields))
        .bind(ty.built_in)
        .bind(dt(ty.created_at))
        .bind(ty.quality_gate.as_str())
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, name: &str) -> Result<Option<ContentType>, StoreError> {
        validate_text("content type name", name)?;
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
        validate_text("content type name", name)?;
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
        validate_text("content locale", &item.locale)?;
        if let Some(html) = &item.body_html {
            validate_text("content body_html", html)?;
        }
        validate_json(&item.body)?;
        validate_json(&item.fields)?;
        let now = Timestamp::now();
        let created = dt(item.created_at.unwrap_or(now));
        let updated = dt(item.updated_at.unwrap_or(now));
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO content_items (id, type, status, slug, parent_id, author_id, title, \
             excerpt, body, body_html, fields, menu_order, published_at, scheduled_at, \
             created_at, updated_at, version, locale, translation_group) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, 1, \
             $17, $18) RETURNING {CONTENT_COLUMNS}"
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
        .bind(created)
        .bind(updated)
        .bind(&item.locale)
        .bind(item.translation_group.unwrap_or(item.id).as_uuid())
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
        locale: Option<&str>,
        parent: Option<ContentId>,
        slug: &str,
    ) -> Result<Option<ContentItem>, StoreError> {
        validate_text("content kind", kind)?;
        validate_text("content slug", slug)?;
        if let Some(l) = locale {
            validate_text("content locale", l)?;
        }
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items \
             WHERE type = $1 AND parent_id IS NOT DISTINCT FROM $2 AND slug = $3 \
             AND ($4::text IS NULL OR locale = $4) ORDER BY id LIMIT 1"
        )))
        .bind(kind)
        .bind(parent.map(|p| p.as_uuid()))
        .bind(slug)
        .bind(locale)
        .fetch_optional(e))?;
        row.as_ref().map(codec::content_item).transpose()
    }

    async fn group_members(&self, groups: &[ContentId]) -> Result<Vec<TranslationRef>, StoreError> {
        check_groups(groups.len())?;
        if groups.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Uuid> = groups.iter().map(|g| g.as_uuid()).collect();
        let rows = run!(&self.exec, |e| sqlx::query(
            "SELECT id, coalesce(translation_group, id) AS grp, type, locale, status, slug, \
             parent_id, published_at, title, author_id FROM content_items \
             WHERE translation_group = ANY($1) OR (translation_group IS NULL AND id = ANY($1)) \
             ORDER BY grp, locale, id"
        )
        .bind(&ids)
        .fetch_all(e))?;
        rows.iter().map(codec::translation_ref).collect()
    }

    async fn relocate(
        &self,
        id: ContentId,
        locale: &str,
        group: ContentId,
        slug: &str,
        expected_version: i64,
    ) -> Result<ContentItem, StoreError> {
        validate_text("content locale", locale)?;
        validate_text("content slug", slug)?;
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE content_items SET locale = $1, translation_group = $2, slug = $3, \
             updated_at = $4, version = version + 1 \
             WHERE id = $5 AND version = $6 RETURNING {CONTENT_COLUMNS}"
        )))
        .bind(locale)
        .bind(group.as_uuid())
        .bind(slug)
        .bind(dt(Timestamp::now()))
        .bind(id.as_uuid())
        .bind(expected_version)
        .fetch_optional(e))?;
        if let Some(row) = row {
            return codec::content_item(&row);
        }
        let current: Option<i64> = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT version FROM content_items WHERE id = $1"
        )
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        match current {
            Some(current_version) => Err(StoreError::Conflict { current_version }),
            None => Err(StoreError::NotFound),
        }
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

    async fn set_updated_at(&self, id: ContentId, at: Timestamp) -> Result<(), StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE content_items SET updated_at = $1 WHERE id = $2"
        )
        .bind(dt(at))
        .bind(id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
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
            push_part(&mut qb, part)?;
            qb.push(" LIMIT ").push_bind(i64::from(left));
            let batch = run!(&self.exec, |e| qb.build().fetch_all(e))?;
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
        let mut qb = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM content_items WHERE TRUE");
        push_filters(&mut qb, query);
        let n: i64 = run!(&self.exec, |e| qb.build_query_scalar().fetch_one(e))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    async fn file_bounds(
        &self,
        query: &ContentQuery,
        file_len: u32,
    ) -> Result<Vec<FileBound>, StoreError> {
        check_file_bounds(query, file_len)?;
        let len = i64::from(file_len);
        // Rows ranked in the `list` order (`content_items_published_list_idx`); one
        // aggregate per file (a hash aggregate, no second sort) gives its last row and
        // newest `updated_at`; only the boundary rows (every `len`-th and the very last)
        // reach the join, for their keys.
        let mut qb = QueryBuilder::<Postgres>::new(
            "WITH ranked AS (\
               SELECT published_at, id, updated_at, \
                 ROW_NUMBER() OVER (ORDER BY published_at DESC NULLS LAST, id DESC) AS rn \
               FROM content_items WHERE published_at IS NOT NULL",
        );
        push_filters(&mut qb, query);
        qb.push(
            "), files AS (\
               SELECT MAX(rn) AS last, MAX(updated_at) AS lastmod FROM ranked \
               GROUP BY (rn - 1) / ",
        )
        .push_bind(len)
        .push(
            ") SELECT r.published_at, r.id, f.lastmod \
             FROM ranked r JOIN files f ON f.last = r.rn \
             WHERE r.rn % ",
        )
        .push_bind(len)
        .push(" = 0 OR r.rn = (SELECT MAX(last) FROM files) ORDER BY f.last");
        let rows: Vec<(DateTime<Utc>, Uuid, DateTime<Utc>)> =
            run!(&self.exec, |e| qb.build_query_as().fetch_all(e))?;
        file_bounds_from_rows(rows.into_iter().map(|(published_at, id, lastmod)| {
            (
                Timestamp::from(published_at).as_micros(),
                id,
                Timestamp::from(lastmod).as_micros(),
            )
        }))
    }

    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<ContentItem>, StoreError> {
        check_id_list("ids", ids)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT {CONTENT_COLUMNS} FROM content_items WHERE id IN ("
        ));
        let mut list = qb.separated(", ");
        for id in ids {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(")");
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
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
        locale: Option<&str>,
        parent: Option<ContentId>,
        prefix: &str,
    ) -> Result<Vec<String>, StoreError> {
        validate_text("content kind", kind)?;
        validate_text("slug prefix", prefix)?;
        if let Some(l) = locale {
            validate_text("content locale", l)?;
        }
        let (from, to) = slug_prefix_bounds(prefix);
        let slugs: Vec<String> = run!(&self.exec, |e| {
            sqlx::query_scalar(
                "SELECT DISTINCT slug FROM content_items WHERE type = $1 \
             AND parent_id IS NOT DISTINCT FROM $2 AND ($7::text IS NULL OR locale = $7) \
             AND (slug = $3 OR (slug >= $4 AND slug < $5)) \
             ORDER BY slug LIMIT $6",
            )
            .bind(kind)
            .bind(parent.map(|p| p.as_uuid()))
            .bind(prefix)
            .bind(&from)
            .bind(&to)
            .bind(i64::from(MAX_SLUG_PREFIX_RESULTS))
            .bind(locale)
            .fetch_all(e)
        })?;
        Ok(slugs
            .into_iter()
            .filter(|s| slug_has_prefix(s, prefix))
            .collect())
    }

    async fn delete(&self, id: ContentId) -> Result<(), StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM content_items WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError> {
        // A new version, so a stale editor save is a Conflict rather than a write that
        // restores the old author; `updated_at` stays (the content did not change).
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE content_items SET author_id = $1, version = version + 1 \
             WHERE author_id = $2"
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

/// `SELECT ... WHERE <the query's filters>`, shared by every part of a listing.
fn listing_base(query: &ContentQuery) -> QueryBuilder<Postgres> {
    let mut qb = QueryBuilder::<Postgres>::new(format!(
        "SELECT {CONTENT_COLUMNS} FROM content_items WHERE TRUE"
    ));
    push_filters(&mut qb, query);
    qb
}

/// Appends the query's filters (everything but paging) to a `WHERE` clause; `list` and
/// `count` both use it so they cannot drift.
fn push_filters(qb: &mut QueryBuilder<Postgres>, query: &ContentQuery) {
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
            .push_bind(dt(reader.now))
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
        qb.push(" AND published_at >= ").push_bind(dt(t));
    }
    if let Some(t) = query.published_before {
        qb.push(" AND published_at < ").push_bind(dt(t));
    }
    if let Some(kind) = query.media_kind {
        qb.push(" AND id IN (SELECT item_id FROM media WHERE ")
            .push(media_kind_sql(kind))
            .push(")");
    }
    if let Some(locale) = &query.locale {
        match locale_filter(query) {
            LocaleFilter::None => {}
            LocaleFilter::Plain => {
                qb.push(" AND locale = ").push_bind(locale.clone());
            }
            LocaleFilter::ExceptMedia => {
                qb.push(" AND (locale = ")
                    .push_bind(locale.clone())
                    .push(" OR type = 'media')");
            }
        }
    }
}

/// Appends a listing part's keyset predicate (a row comparison, so the index range
/// starts at the cursor) and its ORDER BY, which matches a `content_items_*_list_idx`.
/// `title` is declared `COLLATE "C"` (byte order, like SQLite's BINARY).
fn push_part(qb: &mut QueryBuilder<Postgres>, part: ListPart) -> Result<(), StoreError> {
    match part {
        ListPart::Published(after) => {
            qb.push(" AND published_at IS NOT NULL");
            if let Some((k, id)) = after {
                qb.push(" AND (published_at, id) < (")
                    .push_bind(micros_dt(k)?)
                    .push(", ")
                    .push_bind(id)
                    .push(")");
            }
            qb.push(" ORDER BY published_at DESC NULLS LAST, id DESC");
        }
        ListPart::Unpublished(after) => {
            qb.push(" AND published_at IS NULL");
            if let Some(id) = after {
                qb.push(" AND id < ").push_bind(id);
            }
            qb.push(" ORDER BY published_at DESC NULLS LAST, id DESC");
        }
        ListPart::Updated(after) => {
            if let Some((k, id)) = after {
                qb.push(" AND (updated_at, id) < (")
                    .push_bind(micros_dt(k)?)
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
    Ok(())
}

#[async_trait]
impl MetaRepo for PgRepos {
    async fn get(&self, id: ContentId, key: &str) -> Result<Option<Value>, StoreError> {
        validate_text("meta key", key)?;
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
        validate_text("meta key", key)?;
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
        let mut qb = QueryBuilder::<Postgres>::new(
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
        let rows: Vec<(Uuid, String, Json<Value>)> =
            run!(&self.exec, |e| qb.build_query_as().fetch_all(e))?;
        // Ordered in Rust, identically on both backends: input id order, then key bytes.
        let mut position = HashMap::new();
        for (i, id) in ids.iter().enumerate() {
            position.entry(id.as_uuid()).or_insert(i);
        }
        let mut out = Vec::with_capacity(rows.len());
        for (item, key, v) in rows {
            let at = position.get(&item).copied().unwrap_or(usize::MAX);
            out.push((at, ContentId::from_uuid(item), key, v.0));
        }
        out.sort_by(|a, b| (a.0, a.2.as_bytes()).cmp(&(b.0, b.2.as_bytes())));
        Ok(out.into_iter().map(|(_, id, k, v)| (id, k, v)).collect())
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
        let limit = check_limit(limit)?;
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

    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE revisions SET author_id = $1 WHERE author_id = $2"
        )
        .bind(to.as_uuid())
        .bind(from.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected())
    }
}
