//! Migration contract tests (02 §7–§8).

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, IDEMPOTENCY_STALE, IdemBegin, IdempotencyKey,
    ImportMapKind, Media, Timestamp,
};
use ferrite_store::{
    Backend, Compat, MigrateReport, Phase, SCHEMA_META_INSTALL_ID, SCHEMA_META_LAST_BINARY,
    SCHEMA_META_MIN_BINARY, StoreError,
};
use semver::Version;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

pub(crate) fn binary() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver")
}

/// The migration versions this binary has, ascending (from the SQLite directory; both
/// backends have the same ones). Derived, so a new migration (or one landing out of order
/// from a parallel branch) needs no edit in the lists below.
pub(crate) fn versions() -> Vec<i64> {
    let mut v: Vec<i64> = crate::migration_files(&crate::migration_dirs()[0])
        .iter()
        .filter_map(|f| f.strip_suffix(".up.sql"))
        .map(|f| f.split('_').next().unwrap().parse().unwrap())
        .collect();
    v.sort_unstable();
    v
}

/// The newest migration version.
pub(crate) fn latest() -> i64 {
    *versions().last().unwrap()
}

/// The versions above `target`, newest first (what `migrate_down(target)` reverts).
pub(crate) fn down_from(target: i64) -> Vec<i64> {
    versions()
        .into_iter()
        .rev()
        .filter(|v| *v > target)
        .collect()
}

/// The versions above `target`, oldest first (what `migrate_up` then applies).
pub(crate) fn up_from(target: i64) -> Vec<i64> {
    versions().into_iter().filter(|v| *v > target).collect()
}

pub async fn fresh_db_is_up_to_date(db: &TestDb) {
    let store = db.store();
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::UpToDate);
    assert_eq!((s.db_version, s.binary_version), (latest(), latest()));
    assert!(s.pending.is_empty() && s.unknown_applied.is_empty());
    assert_eq!(s.min_binary, None);

    let meta = store.read().schema_meta();
    let site_id = meta.get("site_id").await.unwrap().expect("site_id written");
    uuid::Uuid::parse_str(&site_id).expect("site_id is a uuid");
    assert!(meta.get("created_by_version").await.unwrap().is_some());
    // R16: this database's own install id, never the site id (copies share that).
    let install = meta
        .get(SCHEMA_META_INSTALL_ID)
        .await
        .unwrap()
        .expect("install_id written");
    uuid::Uuid::parse_str(&install).expect("install_id is a uuid");
    assert_ne!(install, site_id);
    store.verify_migrations().await.unwrap();
}

pub async fn up_down_up_roundtrip(db: &TestDb) {
    let store = db.store();
    let site_id = store.read().schema_meta().get("site_id").await.unwrap();
    assert!(
        db.user_objects()
            .await
            .iter()
            .any(|o| o.ends_with(":content_items"))
    );

    assert!(
        db.user_objects()
            .await
            .iter()
            .any(|o| o.ends_with(":search_fts") || o.ends_with(":search_docs"))
    );

    let objects = db.user_objects().await;
    assert!(objects.iter().any(|o| o.ends_with(":jobs")), "{objects:?}");
    assert!(objects.iter().any(|o| o.ends_with(":nodes")), "{objects:?}");
    for table in [
        "sessions",
        "api_tokens",
        "rate_limits",
        "audit_log",
        "audit_log_actor_idx",
    ] {
        assert!(
            objects.iter().any(|o| o.ends_with(&format!(":{table}"))),
            "{table}: {objects:?}"
        );
    }

    // 0008's down loses autosaved SEO, 0007's autosaves (0006's idempotency keys,
    // 0005's sessions and the audit log): refused without allow_lossy, nothing reverted.
    let err = store.migrate_down(0, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        latest()
    );
    // 0005 goes down and up again (the SQLite `DROP COLUMN` included).
    assert_eq!(store.migrate_down(4, true).await.unwrap(), down_from(4));
    assert!(
        !db.user_objects()
            .await
            .iter()
            .any(|o| o.contains("sessions") || o.contains("audit_log")),
    );
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(4));
    assert_eq!(store.migrate_down(0, true).await.unwrap(), down_from(0));
    assert_eq!(db.user_objects().await, Vec::<String>::new());
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!((s.db_version, s.verdict), (0, Compat::NeedsUp));

    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(0));
    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        Vec::<i64>::new()
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::UpToDate);
    assert!(
        db.user_objects()
            .await
            .iter()
            .any(|o| o.ends_with(":content_items"))
    );
    // Down to 0 dropped schema_meta, so the re-created schema is a new site.
    let new_id = store.read().schema_meta().get("site_id").await.unwrap();
    assert!(new_id.is_some() && new_id != site_id);
}

pub async fn verdict_after_partial_down(db: &TestDb) {
    let store = db.store();
    assert_eq!(store.migrate_down(1, true).await.unwrap(), down_from(1));
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.verdict, Compat::NeedsUp);
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        up_from(1)
    );
    assert_eq!((s.db_version, s.binary_version), (1, latest()));
    let objects = db.user_objects().await;
    assert!(objects.iter().any(|o| o.ends_with(":content_items")));
    assert!(!objects.iter().any(|o| o.contains("search")), "{objects:?}");
    assert!(
        !objects.iter().any(|o| o.contains("media_variants")),
        "{objects:?}"
    );
    assert!(
        !objects
            .iter()
            .any(|o| o.ends_with(":jobs") || o.ends_with(":nodes")),
        "{objects:?}"
    );
    assert!(
        !objects.iter().any(|o| o.ends_with(":sessions")),
        "{objects:?}"
    );

    // Up to a target version stops there.
    assert_eq!(store.migrate_up(Some(2)).await.unwrap().applied, vec![2]);
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::NeedsUp
    );
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(2));
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

pub async fn verify_detects_tampered_checksum(db: &TestDb) {
    let store = db.store();
    store.verify_migrations().await.unwrap();
    db.tamper_checksum(1).await;
    let err = store.verify_migrations().await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m)
            if m.contains("checksum mismatch for applied migration 1")),
        "unexpected error: {err:?}"
    );
}

/// An older binary must not erase or lower the min-binary a newer one recorded.
pub async fn min_binary_survives_older_binary(db: &TestDb) {
    let store = db.store();
    let meta = store.read().schema_meta();
    meta.set(SCHEMA_META_MIN_BINARY, "9.9.9").await.unwrap();
    db.insert_unknown_applied(9999).await;

    assert_eq!(
        store.migrate_up(None).await.unwrap().applied,
        Vec::<i64>::new()
    );
    assert_eq!(
        meta.get(SCHEMA_META_MIN_BINARY).await.unwrap().as_deref(),
        Some("9.9.9")
    );
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.unknown_applied, vec![9999]);
    assert_eq!(
        s.verdict,
        Compat::Incompatible {
            requires: Version::new(9, 9, 9)
        }
    );
}

/// Pending migrations carry their description and parsed directives.
pub async fn status_reports_pending_directives(db: &TestDb) {
    let store = db.store();
    assert_eq!(store.migrate_down(2, true).await.unwrap(), down_from(2));
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(s.pending.len(), up_from(2).len());
    let pending = &s.pending[0];
    assert_eq!(pending.version, 3);
    assert!(!pending.description.is_empty());
    assert_eq!(pending.directives.phase, Phase::Expand);
    assert_eq!(pending.directives.min_binary, None);
    assert!(!pending.directives.lossy_down);
    let cluster = &s.pending[1];
    assert_eq!(
        (cluster.version, cluster.description.as_str()),
        (4, "cluster")
    );
    assert_eq!(cluster.directives.phase, Phase::Expand);
    assert!(cluster.directives.lossy_down);
    let auth = &s.pending[2];
    assert_eq!((auth.version, auth.description.as_str()), (5, "auth"));
    assert_eq!(auth.directives.phase, Phase::Expand);
    assert!(auth.directives.lossy_down);
    let media = &s.pending[3];
    assert_eq!(
        (media.version, media.description.as_str()),
        (6, "media comments")
    );
    assert_eq!(media.directives.phase, Phase::Expand);
    assert!(media.directives.lossy_down);
    let autosaves = &s.pending[4];
    assert_eq!(
        (autosaves.version, autosaves.description.as_str()),
        (7, "autosaves")
    );
    assert_eq!(autosaves.directives.phase, Phase::Expand);
    assert_eq!(autosaves.directives.min_binary, None);
    assert!(autosaves.directives.lossy_down);
    let seo = &s.pending[5];
    assert_eq!((seo.version, seo.description.as_str()), (8, "autosave seo"));
    assert_eq!(seo.directives.phase, Phase::Expand);
    assert_eq!(seo.directives.min_binary, None);
    assert!(seo.directives.lossy_down);
    let import = &s.pending[6];
    assert_eq!((import.version, import.description.as_str()), (9, "import"));
    assert_eq!(import.directives.phase, Phase::Expand);
    assert_eq!(import.directives.min_binary, None);
    assert!(import.directives.lossy_down);
    let jobs = &s.pending[7];
    assert_eq!(
        (jobs.version, jobs.description.as_str()),
        (10, "import jobs")
    );
    assert_eq!(jobs.directives.phase, phase_0010(db));
    assert_eq!(jobs.directives.min_binary, None);
    assert!(jobs.directives.lossy_down);
    let gc = &s.pending[8];
    assert_eq!((gc.version, gc.description.as_str()), (11, "artifact gc"));
    assert_eq!(gc.directives.phase, Phase::Expand);
    assert_eq!(gc.directives.min_binary, None);
    assert!(!gc.directives.lossy_down);
    let metrics = &s.pending[9];
    assert_eq!(
        (metrics.version, metrics.description.as_str()),
        (12, "node metrics")
    );
    assert_eq!(metrics.directives.phase, Phase::Expand);
    assert_eq!(metrics.directives.min_binary, None);
    assert!(!metrics.directives.lossy_down);
    let oauth = &s.pending[10];
    assert_eq!((oauth.version, oauth.description.as_str()), (13, "oauth"));
    assert_eq!(oauth.directives.phase, Phase::Expand);
    assert_eq!(oauth.directives.min_binary, None);
    assert!(!oauth.directives.lossy_down);
    let locales = &s.pending[11];
    assert_eq!(
        (locales.version, locales.description.as_str()),
        (14, "locales")
    );
    assert_eq!(locales.directives.phase, Phase::Expand);
    assert!(locales.directives.lossy_down);
}

/// The column names of `table` in the live schema.
async fn columns_of(db: &TestDb, table: &str) -> Vec<String> {
    db.live_tables()
        .await
        .into_iter()
        .find(|t| t.name == table)
        .map(|t| t.columns.into_iter().map(|(n, _)| n).collect())
        .unwrap_or_default()
}

/// A SQL literal of `id` for raw statements on `db`'s backend.
fn id_sql(db: &TestDb, id: uuid::Uuid) -> String {
    if db.is_sqlite() {
        format!("x'{}'", id.simple())
    } else {
        format!("'{id}'")
    }
}

/// `(slug, locale, whether translation_group = id)` of every content item, by slug.
async fn item_locales(db: &TestDb) -> Vec<(String, String, bool)> {
    let rows = db
        .query_text(
            "SELECT slug || '|' || locale || '|' || \
             CASE WHEN translation_group = id THEN 'own' \
                  WHEN translation_group IS NULL THEN 'null' ELSE 'other' END \
             FROM content_items ORDER BY slug",
        )
        .await;
    rows.into_iter()
        .flatten()
        .map(|r| {
            let mut p = r.split('|');
            let slug = p.next().unwrap_or_default().to_owned();
            let locale = p.next().unwrap_or_default().to_owned();
            (slug, locale, p.next() == Some("own"))
        })
        .collect()
}

async fn insert_post(store: &dyn ferrite_store::Store, user: &ferrite_domain::User, slug: &str) {
    let tx = store.begin().await.unwrap();
    tx.content().insert(&new_post(user, slug)).await.unwrap();
    tx.commit().await.unwrap();
}

/// `0014_locales` (M6-1): the new columns and indexes exist; the down (lossy) drops
/// them; up again restores them, the rows backfilled.
pub async fn migration_0014_roundtrip(db: &TestDb) {
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    for (table, cols) in [
        ("content_items", &["locale", "translation_group"][..]),
        ("terms", &["locale", "translation_group"][..]),
        ("menus", &["locale", "translation_of"][..]),
    ] {
        let live = columns_of(db, table).await;
        for c in cols {
            assert!(live.iter().any(|l| l == c), "{table}.{c}: {live:?}");
        }
    }
    let objects = db.user_objects().await;
    for name in [
        "content_items_translation_uq",
        "content_items_locale_list_idx",
        "terms_translation_uq",
        "menus_translation_uq",
        "terms_taxonomy_slug_uq",
    ] {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }
    let user = fixture_user(store).await;
    insert_post(store, &user, "kept").await;

    // The versions above 0014 go first (0015 is lossy too).
    store.migrate_down(14, true).await.unwrap();
    let err = store.migrate_down(13, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 14 has a lossy down")),
        "{err:?}"
    );
    assert_eq!(store.migrate_down(13, true).await.unwrap(), vec![14]);
    assert!(
        !columns_of(db, "content_items")
            .await
            .iter()
            .any(|c| c == "locale")
    );
    assert!(
        !columns_of(db, "menus")
            .await
            .iter()
            .any(|c| c == "translation_of")
    );
    let objects = db.user_objects().await;
    assert!(
        !has(&objects, "content_items_translation_uq"),
        "{objects:?}"
    );
    assert!(has(&objects, "terms_taxonomy_slug_uq"), "{objects:?}");
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(13));
    assert_eq!(
        item_locales(db).await,
        vec![("kept".to_owned(), "en".to_owned(), true)]
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// The backfill takes the locale from a `site.locale` option (`pt_BR.UTF-8` → `pt-BR`),
/// else `en`, and sets every group to the row's own id.
pub async fn backfill_uses_site_locale_option(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    insert_post(store, &user, "a").await;
    store.migrate_down(13, true).await.unwrap();
    store.migrate_up(None).await.unwrap();
    assert_eq!(
        item_locales(db).await,
        vec![("a".into(), "en".into(), true)]
    );

    store.migrate_down(13, true).await.unwrap();
    store
        .read()
        .options()
        .set("site.locale", &serde_json::json!("pt_BR.UTF-8"), true)
        .await
        .unwrap();
    store.migrate_up(None).await.unwrap();
    assert_eq!(
        item_locales(db).await,
        vec![("a".into(), "pt-BR".into(), true)]
    );
    let terms = db.query_text("SELECT locale FROM terms").await;
    assert!(
        terms.iter().all(|l| l.as_deref() == Some("pt-BR")),
        "{terms:?}"
    );
    let menus = db.query_text("SELECT locale FROM menus").await;
    assert!(
        menus.iter().all(|l| l.as_deref() == Some("pt-BR")),
        "{menus:?}"
    );

    store.migrate_down(13, true).await.unwrap();
    store
        .read()
        .options()
        .set("site.locale", &serde_json::json!(7), true)
        .await
        .unwrap();
    store.migrate_up(None).await.unwrap();
    assert_eq!(
        item_locales(db).await,
        vec![("a".into(), "en".into(), true)]
    );
}

/// An INSERT listing only the 0013 columns (what a v0.2.0 binary writes) succeeds: the
/// locale is `''`, the group `NULL`; two such rows may share nothing but still pass the
/// group index (NULLs are distinct).
pub async fn old_style_insert_without_new_columns_works(db: &TestDb) {
    let user = fixture_user(db.store()).await;
    let author = id_sql(db, user.id.as_uuid());
    let (now, body) = if db.is_sqlite() {
        ("0", "'[]'")
    } else {
        ("now()", "'[]'::jsonb")
    };
    for slug in ["old-a", "old-b"] {
        let id = id_sql(db, uuid::Uuid::now_v7());
        db.exec_raw(&format!(
            "INSERT INTO content_items (id, type, status, slug, parent_id, author_id, title, \
             excerpt, body, body_html, fields, menu_order, published_at, scheduled_at, \
             created_at, updated_at, version) VALUES ({id}, 'post', 'draft', '{slug}', NULL, \
             {author}, 't', '', {body}, NULL, {body}, 0, NULL, NULL, {now}, {now}, 1)"
        ))
        .await;
    }
    let rows = db
        .query_text(
            "SELECT locale || '|' || CASE WHEN translation_group IS NULL THEN 'null' ELSE 'set' END \
             FROM content_items ORDER BY slug",
        )
        .await;
    assert_eq!(
        rows,
        vec![Some("|null".to_owned()), Some("|null".to_owned())]
    );
}

/// Slugs are unique per (type, locale, parent): the same slug in two locales passes, a
/// second one in the same locale is refused.
pub async fn slug_unique_per_locale(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    insert_post(store, &user, "about").await;
    insert_post(store, &user, "about-2").await;
    db.exec_raw("UPDATE content_items SET locale = 'de', slug = 'about' WHERE slug = 'about-2'")
        .await;
    insert_post(store, &user, "about-3").await;
    let err = db
        .try_exec_raw("UPDATE content_items SET slug = 'about' WHERE slug = 'about-3'")
        .await;
    assert!(err.is_err(), "same slug, same locale");
    // Terms: unique per (taxonomy, locale, slug).
    let t = |id: uuid::Uuid, locale: &str| {
        format!(
            "INSERT INTO terms (id, taxonomy, slug, name, description, parent_id, locale, \
             translation_group) VALUES ({0}, 'category', 'news', 'News', '', NULL, '{locale}', {0})",
            id_sql(db, id)
        )
    };
    db.exec_raw(&t(uuid::Uuid::now_v7(), "en")).await;
    db.exec_raw(&t(uuid::Uuid::now_v7(), "de")).await;
    assert!(
        db.try_exec_raw(&t(uuid::Uuid::now_v7(), "de"))
            .await
            .is_err()
    );
}

/// A translation group holds at most one item per locale.
pub async fn translation_unique_per_group(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let original = new_post(&user, "orig");
    let group = id_sql(db, original.id.as_uuid());
    let tx = store.begin().await.unwrap();
    tx.content().insert(&original).await.unwrap();
    tx.content().insert(&new_post(&user, "de")).await.unwrap();
    tx.content().insert(&new_post(&user, "de-2")).await.unwrap();
    tx.commit().await.unwrap();
    db.exec_raw(&format!(
        "UPDATE content_items SET locale = 'de', translation_group = {group} WHERE slug = 'de'"
    ))
    .await;
    let second = db
        .try_exec_raw(&format!(
            "UPDATE content_items SET locale = 'de', translation_group = {group} WHERE slug = 'de-2'"
        ))
        .await;
    assert!(second.is_err(), "two members in one locale");
    db.exec_raw(&format!(
        "UPDATE content_items SET locale = 'fr', translation_group = {group} WHERE slug = 'de-2'"
    ))
    .await;
    // Menus: one translation per (translation_of, locale).
    db.exec_raw(
        "INSERT INTO menus (name, label, locale, translation_of) VALUES ('primary', 'P', 'en', NULL); \
         INSERT INTO menus (name, label, locale, translation_of) VALUES ('primary-de', 'P', 'de', 'primary')",
    )
    .await;
    assert!(
        db.try_exec_raw(
            "INSERT INTO menus (name, label, locale, translation_of) VALUES ('primary-de2', 'P', 'de', 'primary')"
        )
        .await
        .is_err()
    );
}

/// The lossy down deletes the rows whose locale is not the first tag of
/// `site.locales` (a blank locale counts as the default), detaching surviving children,
/// and restores the narrower unique indexes.
pub async fn down_deletes_non_default_rows_and_restores_indexes(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    store
        .read()
        .options()
        .set(
            "site.locales",
            &serde_json::json!([{"tag": "en"}, {"tag": "de"}]),
            true,
        )
        .await
        .unwrap();
    store
        .read()
        .options()
        .set(
            "site.locale_texts",
            &serde_json::json!({"de": {"title": "T"}}),
            true,
        )
        .await
        .unwrap();
    for slug in ["en-post", "de-post", "blank-post"] {
        insert_post(store, &user, slug).await;
    }
    db.exec_raw("UPDATE content_items SET locale = 'en' WHERE slug = 'en-post'")
        .await;
    db.exec_raw("UPDATE content_items SET locale = 'de' WHERE slug = 'de-post'")
        .await;
    db.exec_raw("UPDATE content_items SET locale = '' WHERE slug = 'blank-post'")
        .await;
    // A default-locale child of the German item (not possible through core) survives.
    db.exec_raw(
        "UPDATE content_items SET parent_id = (SELECT id FROM content_items WHERE slug = 'de-post') \
         WHERE slug = 'en-post'",
    )
    .await;
    db.exec_raw("UPDATE menus SET locale = 'en'").await;
    db.exec_raw(
        "INSERT INTO menus (name, label, locale, translation_of) VALUES ('m-de', 'M', 'de', 'm')",
    )
    .await;

    store.migrate_down(13, true).await.unwrap();
    let slugs = db
        .query_text("SELECT slug FROM content_items ORDER BY slug")
        .await;
    assert_eq!(
        slugs,
        vec![Some("blank-post".to_owned()), Some("en-post".to_owned())]
    );
    let parents = db
        .query_text("SELECT slug FROM content_items WHERE parent_id IS NOT NULL")
        .await;
    assert!(parents.is_empty(), "{parents:?}");
    assert!(
        store
            .read()
            .options()
            .get("site.locales")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .read()
            .options()
            .get("site.locale_texts")
            .await
            .unwrap()
            .is_none()
    );
    let menus = db
        .query_text("SELECT name FROM menus WHERE name = 'm-de'")
        .await;
    assert!(menus.is_empty());
    // The narrow slug index is back: the same slug twice at top level is refused.
    let dup = db
        .try_exec_raw("UPDATE content_items SET slug = 'en-post' WHERE slug = 'blank-post'")
        .await;
    assert!(dup.is_err(), "same type, both top level, same slug");
    store.migrate_up(None).await.unwrap();
}

/// `0013_oauth` (M5): the four OAuth tables and `api_tokens.oauth_grant_id` exist; the
/// down (not lossy) drops the tables and the OAuth access tokens and keeps personal
/// tokens; up again gives empty tables.
pub async fn migration_0013_roundtrip(db: &TestDb) {
    use ferrite_domain::{ApiToken, ApiTokenId, ClientKind, OAuthGrantId, Timestamp};
    const TABLES: [&str; 4] = [
        "oauth_clients",
        "oauth_requests",
        "oauth_codes",
        "oauth_grants",
    ];
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let has_column = |tables: &[crate::LiveTable]| {
        tables
            .iter()
            .find(|t| t.name == "api_tokens")
            .expect("api_tokens")
            .columns
            .iter()
            .any(|(n, _)| n == "oauth_grant_id")
    };
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }
    assert!(has(&objects, "api_tokens_oauth_grant_idx"), "{objects:?}");
    assert!(has_column(&db.live_tables().await));

    // A grant with an access token, and a personal token.
    let user = fixture_user(store).await;
    let now = Timestamp::now();
    let client = crate::oauth::client("ferc_migration_client_001", ClientKind::Registered, now);
    let oauth = store.read().oauth();
    oauth.upsert_client(&client).await.unwrap();
    let grant = ferrite_domain::OAuthGrant {
        id: OAuthGrantId::new(),
        client_id: client.id.clone(),
        user_id: user.id,
        scopes: vec!["content:read".into()],
        created_at: now,
        last_used_at: now,
        expires_at: Timestamp::from_micros(now.as_micros() + 3_600_000_000).unwrap(),
    };
    oauth.insert_grant(&grant, "refresh").await.unwrap();
    let token = |hash: &str, grant: Option<OAuthGrantId>| ApiToken {
        id: ApiTokenId::new(),
        user_id: user.id,
        name: hash.into(),
        token_hash: hash.into(),
        scopes: vec!["content:read".into()],
        created_at: now,
        last_used_at: None,
        expires_at: None,
        oauth_grant_id: grant,
    };
    let tokens = store.read().api_tokens();
    tokens
        .insert(&token("oauth", Some(grant.id)))
        .await
        .unwrap();
    let personal = token("personal", None);
    tokens.insert(&personal).await.unwrap();

    // Not lossy: no allow_lossy needed once the lossy 0015 above it is reverted.
    store.migrate_down(13, true).await.unwrap();
    assert_eq!(store.migrate_down(12, false).await.unwrap(), vec![13]);
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    assert!(!has(&objects, "api_tokens_oauth_grant_idx"), "{objects:?}");
    assert!(!has_column(&db.live_tables().await));
    // The index of the table that was rebuilt came back.
    assert!(has(&objects, "api_tokens_hash_uq"), "{objects:?}");
    assert!(has(&objects, "api_tokens_user_idx"), "{objects:?}");

    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(12));
    // The OAuth access token went with the down (it must not turn into a personal one);
    // the personal token stayed.
    let tokens = store.read().api_tokens();
    assert_eq!(tokens.get_by_hash("oauth").await.unwrap(), None);
    assert_eq!(
        tokens.get_by_hash("personal").await.unwrap(),
        Some(personal)
    );
    assert_eq!(
        store.read().oauth().grants_for_user(user.id).await.unwrap(),
        vec![]
    );
    assert!(has_column(&db.live_tables().await));
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0012_node_metrics` (M4-2 P35): the table exists; the down drops it with its rows
/// (transient data: not declared lossy, like the export skipping it); up again takes rows.
pub async fn migration_0012_roundtrip(db: &TestDb) {
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    assert!(has(&db.user_objects().await, "node_metrics"));
    let minute_micros = ferrite_domain::MINUTE_MICROS * 1000;
    let minute = Timestamp::from_micros(minute_micros).unwrap();
    let until = Timestamp::from_micros(minute_micros + ferrite_domain::MINUTE_MICROS).unwrap();
    let node = ferrite_domain::NodeId::new();
    let counts = ferrite_domain::MetricCounts::from_counters([7; 9], [1; 12]);
    let requests = || async {
        store
            .read()
            .node_metrics()
            .series(minute, until, ferrite_domain::SeriesStep::Minute, None)
            .await
            .unwrap()
            .iter()
            .map(|b| b.counts.requests)
            .sum::<u64>()
    };
    store
        .read()
        .node_metrics()
        .insert(node, minute, &counts)
        .await
        .unwrap();
    assert_eq!(requests().await, 7);
    // Not lossy: no allow_lossy needed once the versions above it are reverted (0015 is
    // lossy; 0013's own test checks it is not).
    store.migrate_down(12, true).await.unwrap();
    assert_eq!(store.migrate_down(11, false).await.unwrap(), vec![12]);
    assert!(!has(&db.user_objects().await, "node_metrics"));
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(11));
    // The down dropped the row: the table is empty again and takes new rows.
    assert_eq!(requests().await, 0, "the down drops the rows");
    store
        .read()
        .node_metrics()
        .insert(node, minute, &counts)
        .await
        .unwrap();
    assert_eq!(requests().await, 7);
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0010_import_jobs` is a contract migration on SQLite only: widening the
/// `artifacts.kind` CHECK there is a table rebuild (`DROP TABLE`), while Postgres
/// replaces the constraint in place.
fn phase_0010(db: &TestDb) -> Phase {
    if db.is_sqlite() {
        Phase::Contract
    } else {
        Phase::Expand
    }
}

/// `0006_media_comments` (M2a-2b P2): its objects exist; the down is lossy (refused
/// without `allow_lossy`) and drops idempotency keys and the sanitised marker; up again
/// restores the schema with the marker back at its default.
pub async fn migration_0006_roundtrip(db: &TestDb) {
    let store = db.store();
    const OBJECTS: [&str; 9] = [
        "idempotency_keys",
        "idempotency_keys_created_idx",
        "comments_status_created_idx",
        "comments_item_status_idx",
        "comments_created_idx",
        "comments_author_email_idx",
        "comments_user_idx",
        "jobs_status_created_idx",
        "jobs_created_idx",
    ];
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }

    // A marked media row and a pending key, to see what the down loses.
    let author = fixture_user(store).await;
    let mut item = new_post(&author, "marked");
    item.kind = "media".into();
    store.read().content().insert(&item).await.unwrap();
    let artifact = Artifact {
        hash: ArtifactHash::parse(&"a".repeat(64)).unwrap(),
        kind: ArtifactKind::Media,
        bytes: 5,
        mime: "image/svg+xml".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    store
        .read()
        .artifacts()
        .insert_if_absent(&artifact)
        .await
        .unwrap();
    let media = Media {
        item_id: item.id,
        artifact_hash: artifact.hash.clone(),
        mime: "image/svg+xml".into(),
        bytes: 5,
        width: None,
        height: None,
        alt: String::new(),
        original_name: "a.svg".into(),
        variants: serde_json::json!({}),
        sanitized: true,
    };
    store.read().media().insert(&media).await.unwrap();
    let key = IdempotencyKey {
        user: author.id,
        key_hash: "b".repeat(64),
        method: "POST".into(),
        path: "/api/v1/media".into(),
    };
    store
        .read()
        .idempotency()
        .begin(&key, IDEMPOTENCY_STALE)
        .await
        .unwrap();

    // 0007 and 0008 sit above it and are lossy too; refused first, nothing reverted.
    let err = store.migrate_down(5, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        latest()
    );
    assert_eq!(store.migrate_down(5, true).await.unwrap(), down_from(5));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        up_from(5)
    );
    assert!(s.pending[0].directives.lossy_down);

    // Up again re-adds the column (so the down really dropped it) at its default.
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(5));
    let got = store.read().media().get(item.id).await.unwrap().unwrap();
    assert_eq!(
        got,
        Media {
            sanitized: false,
            ..media
        }
    );
    // The key is gone: a new use starts.
    assert!(matches!(
        store
            .read()
            .idempotency()
            .begin(&key, IDEMPOTENCY_STALE)
            .await
            .unwrap(),
        IdemBegin::Started { .. }
    ));
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0007_autosaves` (M2b P15): its objects exist; the down is lossy (refused without
/// `allow_lossy`, nothing reverted) and drops every autosave; up again restores an
/// empty slot table that takes new rows.
pub async fn migration_0007_roundtrip(db: &TestDb) {
    let store = db.store();
    const OBJECTS: [&str; 2] = ["autosaves", "autosaves_user_idx"];
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }

    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, "autosaved"))
        .await
        .unwrap();
    let slot = crate::autosave::full_autosave(item.id, author.id);
    store.read().autosaves().put(&slot).await.unwrap();

    let err = store.migrate_down(6, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("has a lossy down")),
        "{err:?}"
    );
    assert!(has(&db.user_objects().await, "autosaves"));
    assert_eq!(store.migrate_down(6, true).await.unwrap(), down_from(6));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        up_from(6)
    );
    assert!(s.pending[0].directives.lossy_down);
    assert_eq!(s.pending[0].directives.phase, Phase::Expand);

    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(6));
    // The row did not survive; the table takes it again.
    let repos = store.read();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        None
    );
    repos.autosaves().put(&slot).await.unwrap();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        Some(slot)
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0008_autosave_seo` (M2b-2 Task 1): the down is lossy (refused without
/// `allow_lossy`, nothing reverted) and drops `autosaves.seo`, keeping the slot rows;
/// up again re-adds the column empty (so the down really dropped it).
pub async fn migration_0008_roundtrip(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&author, "seo-autosaved"))
        .await
        .unwrap();
    let slot = crate::autosave::full_autosave(item.id, author.id);
    assert!(slot.seo.is_some());
    store.read().autosaves().put(&slot).await.unwrap();

    let err = store.migrate_down(7, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("has a lossy down")),
        "{err:?}"
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().db_version,
        latest()
    );
    assert_eq!(store.migrate_down(7, true).await.unwrap(), down_from(7));
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        up_from(7)
    );
    assert_eq!(
        (
            s.pending[0].description.as_str(),
            s.pending[0].directives.phase
        ),
        ("autosave seo", Phase::Expand)
    );
    assert!(s.pending[0].directives.lossy_down);

    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(7));
    // The slot survived without its SEO member.
    let repos = store.read();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        Some(ferrite_domain::Autosave {
            seo: None,
            ..slot.clone()
        })
    );
    repos.autosaves().put(&slot).await.unwrap();
    assert_eq!(
        repos.autosaves().get(item.id, author.id).await.unwrap(),
        Some(slot)
    );
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0009_import` (M3-1 Task 1): its tables exist; the down is lossy (refused without
/// `allow_lossy`, nothing reverted) and drops every run and map row; up again restores
/// empty tables that take new rows.
pub async fn migration_0009_roundtrip(db: &TestDb) {
    let store = db.store();
    const OBJECTS: [&str; 5] = [
        "import_runs",
        "import_map",
        "import_runs_running_uq",
        "import_runs_started_idx",
        "import_runs_started_by_idx",
    ];
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }

    let admin = fixture_user(store).await;
    let run = crate::import::new_run(admin.id, "https://wp.example");
    store.read().imports().create_run(&run).await.unwrap();
    let map = crate::import::map_row("https://wp.example", ImportMapKind::Content, 7);
    store.read().imports().map_put(&map).await.unwrap();

    let err = store.migrate_down(8, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("has a lossy down")),
        "{err:?}"
    );
    assert!(has(&db.user_objects().await, "import_runs"));
    assert_eq!(store.migrate_down(8, true).await.unwrap(), down_from(8));
    let objects = db.user_objects().await;
    for name in OBJECTS {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        up_from(8)
    );
    assert_eq!(
        (
            s.pending[0].description.as_str(),
            s.pending[0].directives.phase
        ),
        ("import", Phase::Expand)
    );
    assert!(s.pending[0].directives.lossy_down);

    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(8));
    // The rows did not survive; the tables take them again.
    let repos = store.read();
    assert_eq!(repos.imports().get_run(run.id).await.unwrap(), None);
    assert_eq!(
        repos
            .imports()
            .map_get(&map.source, map.kind, map.wp_id)
            .await
            .unwrap(),
        None
    );
    repos.imports().create_run(&run).await.unwrap();
    repos.imports().map_put(&map).await.unwrap();
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

/// `0010_import_jobs` (M3-2 Task 1, P23, P28): `artifacts.kind` takes `import` (on
/// SQLite through a table rebuild that keeps the rows and the foreign keys pointing at
/// them), `import_runs` gains `job_id`, `step` and `report_hash` with
/// `import_runs_job_idx`. The down is lossy (refused without `allow_lossy`): it drops the
/// three columns and every `import` artifact row; up again restores the schema.
pub async fn migration_0010_roundtrip(db: &TestDb) {
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    assert!(has(&db.user_objects().await, "import_runs_job_idx"));

    // A media artifact referenced by a media row survives the rebuild.
    let admin = fixture_user(store).await;
    let post = new_post(&admin, "pic");
    store.read().content().insert(&post).await.unwrap();
    let kept = artifact_0010('c', ArtifactKind::Media);
    let import = artifact_0010('d', ArtifactKind::Import);
    let artifacts = store.read().artifacts();
    assert!(artifacts.insert_if_absent(&kept).await.unwrap());
    assert!(artifacts.insert_if_absent(&import).await.unwrap());
    store
        .read()
        .media()
        .insert(&Media {
            item_id: post.id,
            artifact_hash: kept.hash.clone(),
            mime: "image/png".into(),
            bytes: 1,
            width: None,
            height: None,
            alt: String::new(),
            original_name: "pic.png".into(),
            variants: serde_json::json!({}),
            sanitized: false,
        })
        .await
        .unwrap();
    let mut run = crate::import::new_run(admin.id, "https://wp.example");
    run.job_id = Some(ferrite_domain::JobId::new());
    run.step = Some("pass2".into());
    store.read().imports().create_run(&run).await.unwrap();

    let err = store.migrate_down(9, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("has a lossy down")),
        "{err:?}"
    );
    assert_eq!(store.migrate_down(9, true).await.unwrap(), down_from(9));
    assert!(!has(&db.user_objects().await, "import_runs_job_idx"));
    let s = store.migration_status(&binary()).await.unwrap();
    assert_eq!(
        s.pending.iter().map(|p| p.version).collect::<Vec<_>>(),
        up_from(9)
    );
    assert_eq!(
        (
            s.pending[0].description.as_str(),
            s.pending[0].directives.phase
        ),
        ("import jobs", phase_0010(db))
    );
    assert!(s.pending[0].directives.lossy_down);
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(9));
    // The import artifact went with the down; the media artifact and its row stayed.
    // (Read after the up: the artifact queries need 0011's column.)
    let artifacts = store.read().artifacts();
    assert_eq!(artifacts.get(&import.hash).await.unwrap(), None);
    assert_eq!(artifacts.get(&kept.hash).await.unwrap(), Some(kept.clone()));
    assert!(store.read().media().get(post.id).await.unwrap().is_some());
    // The run survived without its new columns; they take values again.
    let repos = store.read();
    let back = repos.imports().get_run(run.id).await.unwrap().unwrap();
    assert_eq!(
        (back.job_id, back.step, back.report_hash),
        (None, None, None)
    );
    assert!(repos.artifacts().insert_if_absent(&import).await.unwrap());
    assert!(
        repos
            .imports()
            .set_step(run.id, &run.token, "pass3")
            .await
            .unwrap()
    );
    // The rebuilt table still guards the media foreign key.
    assert!(repos.artifacts().delete(&kept.hash).await.is_err());
    assert_eq!(
        store.migration_status(&binary()).await.unwrap().verdict,
        Compat::UpToDate
    );
}

fn artifact_0010(c: char, kind: ArtifactKind) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&c.to_string().repeat(64)).unwrap(),
        kind,
        bytes: 10,
        mime: "application/octet-stream".into(),
        created_at: Timestamp::from_micros(1_700_000_000_000_000).unwrap(),
        orphaned_at: None,
    }
}

/// M4-2 P30: `migrate_up` reports what it applied and, on SQLite, the pre-migration
/// snapshot it wrote (Postgres takes none). `last_binary_version` is an ordinary
/// `schema_meta` key the migrations' bookkeeping leaves alone.
pub async fn migrate_up_reports_snapshot(db: &TestDb) {
    let store = db.store();
    let meta = store.read().schema_meta();
    meta.set(SCHEMA_META_LAST_BINARY, "0.0.1").await.unwrap();

    let none = store.migrate_up(None).await.unwrap();
    assert_eq!(none, MigrateReport::default(), "nothing pending");

    // Derived, so a new migration needs no edit here.
    let l = store
        .migration_status(&binary())
        .await
        .unwrap()
        .binary_version;
    let v = versions();
    let (prev, before) = (v[v.len() - 2], v[v.len() - 3]);
    assert_eq!(l, latest());
    assert_eq!(
        store.migrate_down(before, true).await.unwrap(),
        vec![l, prev]
    );
    let report = store.migrate_up(None).await.unwrap();
    assert_eq!(report.applied, vec![prev, l]);
    match store.backend() {
        Backend::Sqlite => {
            let snap = report
                .snapshot
                .expect("SQLite writes a pre-migration snapshot");
            assert!(snap.is_file(), "{}", snap.display());
            let name = snap.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                name.starts_with(&format!("pre-{before}-to-{l}-")) && name.ends_with(".db"),
                "{name}"
            );
        }
        Backend::Postgres => assert_eq!(report.snapshot, None),
    }
    assert_eq!(
        meta.get(SCHEMA_META_LAST_BINARY).await.unwrap().as_deref(),
        Some("0.0.1")
    );
}

/// `0015_plugin_publishers` (M6-2): the three trust tables exist, go with the down (lossy,
/// M6-2 review: the bindings and revocations go; plugin rows and data stay) and come back
/// empty.
pub async fn migration_0015_roundtrip(db: &TestDb) {
    use ferrite_domain::plugin_trust::KeyId;
    const TABLES: [&str; 3] = ["plugin_publishers", "plugin_owners", "plugin_registry"];
    let store = db.store();
    let has =
        |objects: &[String], name: &str| objects.iter().any(|o| o.ends_with(&format!(":{name}")));
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(has(&objects, name), "{name}: {objects:?}");
    }
    assert!(has(&objects, "plugin_owners_signer_idx"), "{objects:?}");
    assert!(
        has(&objects, "plugin_publishers_public_key_uq"),
        "{objects:?}"
    );
    let now = Timestamp::now();
    let trust = store.read().plugin_trust();
    trust
        .put_owner(&crate::plugin_trust::owner("seo", Some(1), now))
        .await
        .unwrap();
    store
        .read()
        .options()
        .set("plugin.seo.settings.x", &serde_json::json!(1), false)
        .await
        .unwrap();

    // Lossy: refused without allow_lossy, nothing reverted (newer lossy migrations are
    // reverted first, so the refusal names 15).
    store.migrate_down(15, true).await.unwrap();
    let err = store.migrate_down(14, false).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::Migration(m) if m.contains("migration 15 has a lossy down")),
        "{err:?}"
    );
    assert!(has(&db.user_objects().await, "plugin_owners"));
    assert_eq!(store.migrate_down(14, true).await.unwrap(), vec![15]);
    let objects = db.user_objects().await;
    for name in TABLES {
        assert!(!has(&objects, name), "{name} left: {objects:?}");
    }
    assert_eq!(store.migrate_up(None).await.unwrap().applied, up_from(14));
    let trust = store.read().plugin_trust();
    assert_eq!(trust.get_owner("seo").await.unwrap(), None);
    assert!(trust.list_keys().await.unwrap().is_empty());
    assert!(
        trust
            .owners_signed_by(&KeyId::parse("0000000000000001").unwrap())
            .await
            .unwrap()
            .is_empty()
    );
    // The plugin's data was never touched.
    assert!(
        store
            .read()
            .options()
            .get("plugin.seo.settings.x")
            .await
            .unwrap()
            .is_some()
    );
}
