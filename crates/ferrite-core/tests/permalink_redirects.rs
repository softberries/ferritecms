//! Redirects for `permalink.*` structure changes (M3-2 P30, M2a-2b P26): every write
//! enqueues a `permalink.redirects` job in its transaction, and the job redirects each
//! published item's old path to its new one, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use ferrite_core::{
    Actor, ContentDraft, ContentService, OptionService, PermalinkRedirectReport, PermalinkRules,
    RedirectService,
    events::INVALIDATE,
    import::ImportService,
    jobs::{PERMALINK_REDIRECTS_KIND, PermalinkRedirects, PermalinkStructures},
};
use ferrite_domain::{
    ContentId, ContentStatus, ImportRunState, Job, JobQuery, NewContent, NewUser, NodeId, Redirect,
    RedirectId, RedirectOrigin, RedirectQuery, Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

async fn actor(store: &dyn Store) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor::new(id, Role::Admin)
}

fn draft(kind: &str, slug: &str, parent_id: Option<ContentId>) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: slug.into(),
        slug: Some(slug.into()),
        parent_id,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

async fn published(svc: &ContentService, admin: &Actor, d: ContentDraft) -> ContentId {
    let item = svc.create(admin, d).await.unwrap();
    svc.publish(admin, item.id, item.version).await.unwrap().id
}

fn set(key: &str, value: &str) -> (String, Option<Value>) {
    (key.to_owned(), Some(json!(value)))
}

async fn redirect(store: &dyn Store, source: &str) -> Option<Redirect> {
    store
        .read()
        .redirects()
        .get_by_source(source)
        .await
        .unwrap()
}

async fn target(store: &dyn Store, source: &str) -> Option<String> {
    redirect(store, source).await.map(|r| r.target)
}

async fn all_redirects(store: &dyn Store) -> Vec<Redirect> {
    let mut all = Vec::new();
    let mut query = RedirectQuery {
        q: None,
        limit: 200,
        cursor: None,
    };
    loop {
        let page = store.read().redirects().list(&query).await.unwrap();
        all.extend(page.items);
        match page.next_cursor {
            Some(c) => query.cursor = Some(c),
            None => return all,
        }
    }
}

async fn queued(store: &dyn Store) -> Vec<Job> {
    store
        .read()
        .jobs()
        .list(&JobQuery {
            status: None,
            kind: Some(PERMALINK_REDIRECTS_KIND.into()),
            limit: 200,
            cursor: None,
        })
        .await
        .unwrap()
        .items
}

/// Claims every due `permalink.redirects` job and runs it the way the handler does,
/// oldest first; the reports in run order.
async fn run_jobs(db: &TestDb) -> Vec<PermalinkRedirectReport> {
    let store = db.store();
    let svc = RedirectService::new(db.shared_store());
    let mut reports = Vec::new();
    loop {
        let jobs = store
            .read()
            .jobs()
            .claim(
                NodeId::new(),
                &[PERMALINK_REDIRECTS_KIND.to_owned()],
                1,
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        let Some(job) = jobs.into_iter().next() else {
            return reports;
        };
        let payload: PermalinkRedirects = serde_json::from_value(job.payload.clone()).unwrap();
        reports.push(svc.structure_change(&payload).await.unwrap());
        assert!(
            store
                .read()
                .jobs()
                .complete(&job.lease().unwrap())
                .await
                .unwrap()
        );
    }
}

/// Changing the post structure redirects every published post (origin `slug-change`,
/// `301`); drafts and pages get nothing, and the payload holds both structures. An
/// option write that leaves the rules as they were enqueues nothing.
pub async fn post_structure_change_redirects_published_posts(db: &TestDb) {
    let store = db.store();
    let admin = actor(store).await;
    let content = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    published(&content, &admin, draft("post", "hello", None)).await;
    published(&content, &admin, draft("post", "world", None)).await;
    content
        .create(&admin, draft("post", "unfinished", None))
        .await
        .unwrap();
    published(&content, &admin, draft("page", "about", None)).await;

    let options = OptionService::new(db.shared_store());
    options
        .set_many(&admin, &[set("permalink.post", "/blog/%slug%/")])
        .await
        .unwrap();
    let jobs = queued(store).await;
    assert_eq!(jobs.len(), 1);
    let payload: PermalinkRedirects = serde_json::from_value(jobs[0].payload.clone()).unwrap();
    assert_eq!(payload.v, 1);
    assert_eq!(
        payload.prev,
        PermalinkStructures {
            post: "/%slug%/".into(),
            page: "/%parent_path%/%slug%/".into(),
        }
    );
    assert_eq!(
        payload.next,
        PermalinkStructures {
            post: "/blog/%slug%/".into(),
            page: "/%parent_path%/%slug%/".into(),
        }
    );
    assert_eq!(
        jobs[0].unique_key.as_deref(),
        Some(format!("permalink.redirects:{}", payload.version).as_str())
    );

    let reports = run_jobs(db).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].moved, 2);
    let hello = redirect(store, "/hello/").await.unwrap();
    assert_eq!(hello.target, "/blog/hello/");
    assert_eq!(hello.code, 301);
    assert_eq!(hello.origin, RedirectOrigin::SlugChange);
    assert_eq!(
        target(store, "/world/").await.as_deref(),
        Some("/blog/world/")
    );
    assert_eq!(target(store, "/unfinished/").await, None);
    assert_eq!(target(store, "/about/").await, None);
    assert_eq!(all_redirects(store).await.len(), 2);

    // The same value again, and a non-permalink setting: no new job.
    options
        .set_many(&admin, &[set("permalink.post", "/blog/%slug%/")])
        .await
        .unwrap();
    options
        .set_many(&admin, &[set("site.title", "Elsewhere")])
        .await
        .unwrap();
    assert_eq!(queued(store).await.len(), 1);
    // `set` and `delete` enqueue too.
    options
        .set(&admin, "permalink.post", &json!("/b/%slug%/"), true)
        .await
        .unwrap();
    options.delete(&admin, "permalink.post").await.unwrap();
    let jobs = queued(store).await;
    assert_eq!(jobs.len(), 3);
    let keys: std::collections::HashSet<_> = jobs.iter().map(|j| j.unique_key.clone()).collect();
    assert_eq!(keys.len(), 3, "one key per options version");
    run_jobs(db).await;
    // Back on the default: the live paths carry no redirect, the others lead there.
    assert_eq!(target(store, "/hello/").await, None);
    assert_eq!(
        target(store, "/blog/hello/").await.as_deref(),
        Some("/hello/")
    );
    assert_eq!(target(store, "/b/hello/").await.as_deref(), Some("/hello/"));
}

/// Every redirect target is a path no redirect starts at.
async fn assert_single_hops(store: &dyn Store) {
    let all = all_redirects(store).await;
    for r in &all {
        assert!(
            all.iter().all(|other| other.source_path != r.target),
            "{} -> {} is followed by another hop",
            r.source_path,
            r.target
        );
    }
}

/// Two changes leave one hop from each earlier path to the live one, whether the first
/// job ran before the second change or both ran after it.
pub async fn two_changes_leave_single_hops(db: &TestDb) {
    let store = db.store();
    let admin = actor(store).await;
    let content = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    let id = published(&content, &admin, draft("post", "hello", None)).await;
    let year = content
        .get(id)
        .await
        .unwrap()
        .published_at
        .unwrap()
        .as_datetime()
        .format("%Y")
        .to_string();
    let dated = format!("/{year}/hello/");
    let options = OptionService::new(db.shared_store());

    options
        .set_many(&admin, &[set("permalink.post", "/blog/%slug%/")])
        .await
        .unwrap();
    run_jobs(db).await;
    options
        .set_many(&admin, &[set("permalink.post", "/%year%/%slug%/")])
        .await
        .unwrap();
    run_jobs(db).await;
    assert_eq!(
        target(store, "/hello/").await.as_deref(),
        Some(dated.as_str())
    );
    assert_eq!(
        target(store, "/blog/hello/").await.as_deref(),
        Some(dated.as_str())
    );
    assert_eq!(target(store, &dated).await, None);
    assert_single_hops(store).await;

    // Two more changes before any job runs, ending on the default again.
    options
        .set_many(&admin, &[set("permalink.post", "/n/%slug%/")])
        .await
        .unwrap();
    options
        .set_many(&admin, &[("permalink.post".to_owned(), None)])
        .await
        .unwrap();
    assert_eq!(run_jobs(db).await.len(), 2);
    assert_eq!(target(store, "/hello/").await, None);
    for old in ["/blog/hello/", dated.as_str(), "/n/hello/"] {
        assert_eq!(
            target(store, old).await.as_deref(),
            Some("/hello/"),
            "{old}"
        );
    }
    assert_single_hops(store).await;
}

/// A `manual` or `import` redirect on an old path is kept as it is.
pub async fn manual_redirect_on_old_path_survives(db: &TestDb) {
    let store = db.store();
    let admin = actor(store).await;
    let content = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    published(&content, &admin, draft("post", "hello", None)).await;
    published(&content, &admin, draft("post", "world", None)).await;
    published(&content, &admin, draft("post", "third", None)).await;
    let redirects = RedirectService::new(db.shared_store());
    redirects
        .upsert(&admin, "/hello", "/elsewhere/", 302)
        .await
        .unwrap();
    store
        .read()
        .redirects()
        .upsert(&Redirect {
            id: RedirectId::new(),
            source_path: "/world/".into(),
            target: "/imported/".into(),
            code: 301,
            hits: 0,
            origin: RedirectOrigin::Import,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .unwrap();

    OptionService::new(db.shared_store())
        .set_many(&admin, &[set("permalink.post", "/blog/%slug%/")])
        .await
        .unwrap();
    let reports = run_jobs(db).await;
    assert_eq!(reports[0].moved, 1);
    assert_eq!(reports[0].kept, 2);
    // The manual row sits on the slash variant of the old path.
    let manual = redirect(store, "/hello").await.unwrap();
    assert_eq!(manual.target, "/elsewhere/");
    assert_eq!(manual.code, 302);
    assert_eq!(manual.origin, RedirectOrigin::Manual);
    assert_eq!(redirect(store, "/hello/").await, None);
    let imported = redirect(store, "/world/").await.unwrap();
    assert_eq!(imported.target, "/imported/");
    assert_eq!(imported.origin, RedirectOrigin::Import);
    assert_eq!(
        target(store, "/third/").await.as_deref(),
        Some("/blog/third/")
    );
}

/// The page structure moves pages (children included) and leaves posts alone.
pub async fn page_structure_follows_page_rule(db: &TestDb) {
    let store = db.store();
    let admin = actor(store).await;
    let content = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    let about = published(&content, &admin, draft("page", "about", None)).await;
    published(&content, &admin, draft("page", "team", Some(about))).await;
    published(&content, &admin, draft("post", "hello", None)).await;

    OptionService::new(db.shared_store())
        .set_many(&admin, &[set("permalink.page", "/p/%parent_path%/%slug%/")])
        .await
        .unwrap();
    let reports = run_jobs(db).await;
    assert_eq!(reports[0].moved, 2);
    assert_eq!(target(store, "/about/").await.as_deref(), Some("/p/about/"));
    assert_eq!(
        target(store, "/about/team/").await.as_deref(),
        Some("/p/about/team/")
    );
    assert_eq!(target(store, "/hello/").await, None);
    assert_eq!(all_redirects(store).await.len(), 2);
}

/// An old path that is now another published item's path gets no redirect.
pub async fn source_that_is_a_live_path_is_refused(db: &TestDb) {
    let store = db.store();
    let admin = actor(store).await;
    let options = OptionService::new(db.shared_store());
    options
        .set_many(&admin, &[set("permalink.page", "/p/%parent_path%/%slug%/")])
        .await
        .unwrap();
    run_jobs(db).await;
    let rules = PermalinkRules::from_options(None, Some("/p/%parent_path%/%slug%/")).unwrap();
    let content = ContentService::new(db.shared_store(), Arc::new(rules));
    published(&content, &admin, draft("post", "hello", None)).await;
    published(&content, &admin, draft("page", "hello", None)).await;

    // The page moves to `/hello/`, where the post was.
    options
        .set_many(
            &admin,
            &[
                set("permalink.post", "/blog/%slug%/"),
                ("permalink.page".to_owned(), None),
            ],
        )
        .await
        .unwrap();
    let reports = run_jobs(db).await;
    assert_eq!(reports[0].moved, 1);
    assert_eq!(reports[0].refused, 1);
    assert_eq!(target(store, "/hello/").await, None);
    assert_eq!(target(store, "/p/hello/").await.as_deref(), Some("/hello/"));
    assert_eq!(all_redirects(store).await.len(), 1);
}

/// The importer's `permalink.*` write enqueues the job in its batch; a rolled-back
/// batch enqueues nothing.
pub async fn import_permalink_write_enqueues_too(db: &TestDb) {
    let store = db.store();
    let admin = actor(store).await;
    let content = ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    published(&content, &admin, draft("post", "hello", None)).await;
    let svc = ImportService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    let run = svc
        .start(&admin, "https://wp.example", false, false)
        .await
        .unwrap();

    let mut batch = run.schema_batch(&svc).await.unwrap();
    batch
        .options(&[set("permalink.post", "/x/%slug%/")])
        .await
        .unwrap();
    batch.rollback().await.unwrap();
    assert!(queued(store).await.is_empty());

    let mut batch = run.schema_batch(&svc).await.unwrap();
    batch
        .options(&[set("permalink.post", "/%year%/%month%/%slug%/")])
        .await
        .unwrap();
    batch.commit().await.unwrap();
    run.finish(&svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();
    let jobs = queued(store).await;
    assert_eq!(jobs.len(), 1);
    let payload: PermalinkRedirects = serde_json::from_value(jobs[0].payload.clone()).unwrap();
    assert_eq!(payload.prev.post, "/%slug%/");
    assert_eq!(payload.next.post, "/%year%/%month%/%slug%/");
    assert_eq!(run_jobs(db).await[0].moved, 1);
    assert!(target(store, "/hello/").await.unwrap().ends_with("/hello/"));
}

/// 1 200 published posts: every one is redirected, in three pages of at most 500, each
/// page with its own `invalidate` event.
pub async fn job_pages_through_1200_items(db: &TestDb) {
    let store = db.store();
    let admin = actor(store).await;
    let tx = store.begin().await.unwrap();
    let now = Timestamp::now();
    for i in 0..1200 {
        tx.content()
            .insert(&NewContent {
                id: ContentId::new(),
                kind: "post".into(),
                status: ContentStatus::Published,
                slug: format!("p{i:04}"),
                parent_id: None,
                author_id: admin.user,
                title: format!("Post {i}"),
                excerpt: String::new(),
                body: json!({"type": "doc", "content": []}),
                body_html: None,
                fields: json!({}),
                menu_order: 0,
                published_at: Some(now),
                scheduled_at: None,
                created_at: None,
                updated_at: None,
            })
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    OptionService::new(db.shared_store())
        .set_many(&admin, &[set("permalink.post", "/blog/%slug%/")])
        .await
        .unwrap();
    let seq = store.read().events().max_seq().await.unwrap();
    let reports = run_jobs(db).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].moved, 1200);
    assert_eq!(reports[0].pages, 3);
    let all = all_redirects(store).await;
    assert_eq!(all.len(), 1200);
    assert!(
        all.iter()
            .all(|r| r.target == format!("/blog{}", r.source_path))
    );
    let pages = store
        .read()
        .events()
        .since(seq, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == INVALIDATE)
        .count();
    assert_eq!(pages, 3);
}

backend_tests!(
    post_structure_change_redirects_published_posts,
    two_changes_leave_single_hops,
    manual_redirect_on_old_path_survives,
    page_structure_follows_page_rule,
    source_that_is_a_live_path_is_refused,
    import_permalink_write_enqueues_too,
    job_pages_through_1200_items,
);
