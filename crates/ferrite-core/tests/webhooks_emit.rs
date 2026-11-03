//! `webhooks::emit` from the write paths (M6-6 task 3, design 18 §3–§4.1): each
//! catalogue group is emitted by its write, inside the write's `Tx`; payloads are thin.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use ferrite_core::{
    Actor, AuthService, ContentDraft, ContentService, MediaService, NewAccount, NewUpload,
    PermalinkRules, UserPatch, UserService,
    auth::SiteKey,
    comments::{CommentInput, CommentService, Commenter, Submitted},
    import::ImportService,
    permalink::Permalinks,
    webhooks::{NewWebhookEvent, UrlPolicy, WebhookInput, WebhookService, emit},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, CommentStatus, ImportRunState, JobQuery, NewUser, Role,
    Timestamp, UserId, UserStatus, WebhookEventRow,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

fn any_ip(_: IpAddr) -> bool {
    true
}

struct Site {
    store: Arc<dyn Store>,
    hooks: WebhookService,
    content: Arc<ContentService>,
    admin: Actor,
}

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    store
        .read()
        .users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    Actor::new(id, role)
}

async fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let hooks = WebhookService::new(
        store.clone(),
        SiteKey::from_hex(&"ab".repeat(32)).unwrap(),
        UrlPolicy {
            local_targets: false,
            allow_ip: any_ip,
        },
    );
    let admin = actor(store.as_ref(), Role::Admin).await;
    let content = Arc::new(ContentService::new(
        store.clone(),
        Arc::new(PermalinkRules::default()),
    ));
    Site {
        store,
        hooks,
        content,
        admin,
    }
}

impl Site {
    async fn hook(&self, events: &[&str], types: Option<&[&str]>) {
        self.hooks
            .create(
                &self.admin,
                WebhookInput {
                    name: format!("hook {}", events.join(",")),
                    url: "https://hooks.example.com/x".into(),
                    events: events.iter().map(|e| (*e).to_owned()).collect(),
                    content_types: types.map(|t| t.iter().map(|x| (*x).to_owned()).collect()),
                    ..WebhookInput::default()
                },
            )
            .await
            .unwrap();
    }

    /// Every event row with its delivery count, oldest first.
    async fn events(&self) -> Vec<WebhookEventRow> {
        let w = self.store.read().webhooks();
        let mut out = Vec::new();
        for e in w.list_endpoints().await.unwrap() {
            let page = w
                .list_deliveries(
                    e.id,
                    &ferrite_domain::DeliveryQuery {
                        limit: 200,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            for l in page.items {
                let ev = w.get_event(l.delivery.event_id).await.unwrap().unwrap();
                if !out.iter().any(|x: &WebhookEventRow| x.id == ev.id) {
                    out.push(ev);
                }
            }
        }
        out.sort_by_key(|e| (e.created_at, e.id));
        out
    }

    async fn kinds(&self) -> Vec<String> {
        self.events().await.into_iter().map(|e| e.kind).collect()
    }

    async fn deliver_jobs(&self) -> usize {
        self.store
            .read()
            .jobs()
            .list(&JobQuery {
                kind: Some("webhook.deliver".into()),
                limit: 200,
                ..JobQuery::default()
            })
            .await
            .unwrap()
            .items
            .len()
    }
}

fn draft(kind: &str, title: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: Some("An excerpt that must not leave".into()),
        body: json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "secret body text"}]}
        ]}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

async fn no_endpoint_no_rows(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "Hello"))
        .await
        .unwrap();
    s.content
        .publish(&s.admin, item.id, item.version)
        .await
        .unwrap();
    assert!(s.events().await.is_empty());
    assert_eq!(s.deliver_jobs().await, 0);
    // An endpoint for another group sees nothing either.
    s.hook(&["user.*"], None).await;
    s.content
        .create(&s.admin, draft("post", "Again"))
        .await
        .unwrap();
    assert!(s.events().await.is_empty());
}

async fn rolled_back_write_leaves_nothing(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["*"], None).await;
    let tx = s.store.begin().await.unwrap();
    let ev = NewWebhookEvent {
        kind: "content.created",
        content_type: Some("post".into()),
        data: json!({"id": "x"}),
    };
    assert_eq!(emit(tx.as_ref(), ev).await.unwrap(), 1);
    tx.rollback().await.unwrap();
    assert!(s.events().await.is_empty());
    assert_eq!(s.deliver_jobs().await, 0);
    // A refused write rolls back its event: deleting a published item is refused.
    let item = s
        .content
        .create(&s.admin, draft("post", "Kept"))
        .await
        .unwrap();
    let before = s.events().await.len();
    let item = s
        .content
        .publish(&s.admin, item.id, item.version)
        .await
        .unwrap();
    let after_publish = s.events().await.len();
    assert_eq!(after_publish, before + 1);
    assert!(
        s.content
            .delete(&s.admin, item.id, item.version)
            .await
            .is_err()
    );
    assert_eq!(s.events().await.len(), after_publish);
}

async fn content_lifecycle_emits_each_type(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["content.*"], None).await;
    let c = &s.content;
    let a = &s.admin;
    let item = c.create(a, draft("post", "Hello")).await.unwrap();
    let item = c
        .update(a, item.id, item.version, draft("post", "Hello 2"))
        .await
        .unwrap();
    let item = c.submit(a, item.id, item.version).await.unwrap();
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_000).unwrap();
    let item = c.schedule(a, item.id, item.version, at).await.unwrap();
    let item = c.publish(a, item.id, item.version).await.unwrap();
    let item = c.unpublish(a, item.id, item.version).await.unwrap();
    let item = c.trash(a, item.id, item.version).await.unwrap();
    let item = c.restore(a, item.id, item.version).await.unwrap();
    let item = c.trash(a, item.id, item.version).await.unwrap();
    c.delete(a, item.id, item.version).await.unwrap();
    assert_eq!(
        s.kinds().await,
        [
            "content.created",
            "content.updated",
            "content.submitted",
            "content.scheduled",
            "content.published",
            "content.unpublished",
            "content.trashed",
            "content.restored",
            "content.trashed",
            "content.deleted",
        ]
    );
    let events = s.events().await;
    let scheduled = &events[3].data;
    assert!(scheduled["scheduled_at"].is_string(), "{scheduled}");
    let published = &events[4].data;
    assert_eq!(published["public"], true);
    assert_eq!(published["status"], "published");
    assert_eq!(published["actor"], a.user.to_string());
    assert!(published["path"].as_str().unwrap().contains("hello"));
    assert_eq!(events[0].data["public"], false);
}

async fn scheduled_publish_has_null_actor(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["content.published"], None).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "Later"))
        .await
        .unwrap();
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 1_000_000).unwrap();
    s.content
        .schedule(&s.admin, item.id, item.version, at)
        .await
        .unwrap();
    assert!(s.content.publish_scheduled(item.id, at).await.unwrap());
    let events = s.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data["actor"], Value::Null);
}

fn png(seed: u8) -> NewUpload {
    NewUpload {
        artifact: Artifact {
            hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
            kind: ArtifactKind::Media,
            bytes: 1234,
            mime: "image/png".into(),
            created_at: Timestamp::now(),
            orphaned_at: None,
        },
        mime: "image/png".into(),
        bytes: 1234,
        original_name: "photo.png".into(),
        title: None,
        alt: "An image".into(),
        sanitized: false,
        raster: true,
    }
}

async fn upload_emits_media_uploaded_only(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["*"], None).await;
    let rules: Arc<dyn Permalinks> = Arc::new(PermalinkRules::default());
    let media = MediaService::new(s.store.clone(), s.content.clone(), rules);
    let up = media.record_upload(&s.admin, png(3)).await.unwrap();
    assert_eq!(s.kinds().await, ["media.uploaded"]);
    let data = &s.events().await[0].data;
    assert_eq!(data["mime"], "image/png");
    assert_eq!(data["alt"], "An image");
    assert_eq!(data["bytes"], 1234);
    let keys: Vec<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for k in ["path", "url", "variants", "artifact_hash"] {
        assert!(!keys.contains(&k), "{k}: {keys:?}");
    }
    let item = media
        .update(
            &s.admin,
            up.item.id,
            up.item.version,
            ferrite_core::MediaPatch {
                title: None,
                alt: Some("New alt".into()),
            },
        )
        .await
        .unwrap()
        .item;
    let item = media.trash(&s.admin, item.id, item.version).await.unwrap();
    let item = media
        .restore(&s.admin, item.id, item.version)
        .await
        .unwrap()
        .item;
    let item = media.trash(&s.admin, item.id, item.version).await.unwrap();
    media.delete(&s.admin, item.id, item.version).await.unwrap();
    assert_eq!(
        s.kinds().await,
        [
            "media.uploaded",
            "media.updated",
            "media.trashed",
            "media.restored",
            "media.trashed",
            "media.deleted",
        ]
    );
}

fn next_ip() -> IpAddr {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + n))
}

async fn comment_submit_moderate_delete(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["comment.*"], None).await;
    let auth = Arc::new(AuthService::new(
        s.store.clone(),
        SiteKey::generate().unwrap(),
    ));
    let comments = CommentService::new(s.store.clone(), auth, Arc::new(PermalinkRules::default()));
    let item = s
        .content
        .create(&s.admin, draft("post", "Commented"))
        .await
        .unwrap();
    let item = s
        .content
        .publish(&s.admin, item.id, item.version)
        .await
        .unwrap();
    let long = "x".repeat(2500);
    let submitted = comments
        .submit(
            CommentInput {
                item: item.id,
                kind: "post".into(),
                parent: None,
                author_name: "Ann".into(),
                author_email: "ann@example.com".into(),
                author_url: "https://ann.example".into(),
                body: long,
                honeypot: String::new(),
            },
            Commenter::Anonymous,
            next_ip(),
        )
        .await
        .unwrap();
    let Submitted::Accepted(c) = submitted else {
        panic!("discarded")
    };
    comments
        .moderate(&s.admin, c.id, CommentStatus::Approved)
        .await
        .unwrap();
    // The same status again: no event.
    comments
        .moderate(&s.admin, c.id, CommentStatus::Approved)
        .await
        .unwrap();
    comments
        .moderate(&s.admin, c.id, CommentStatus::Spam)
        .await
        .unwrap();
    comments.delete(&s.admin, c.id).await.unwrap();
    assert_eq!(
        s.kinds().await,
        [
            "comment.created",
            "comment.moderated",
            "comment.moderated",
            "comment.deleted"
        ]
    );
    let events = s.events().await;
    let created = &events[0].data;
    assert_eq!(created["body"].as_str().unwrap().chars().count(), 2000);
    assert_eq!(created["item"]["type"], "post");
    assert_eq!(created["actor"], Value::Null);
    let all = format!("{events:?}");
    for leaked in ["ann@example.com", "ann.example", "ip_hash"] {
        assert!(!all.contains(leaked), "{leaked}");
    }
    assert_eq!(events[1].data["previous_status"], "pending");
    assert_eq!(events[2].data["previous_status"], "approved");
}

async fn user_create_update_delete(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["user.*"], None).await;
    let users = UserService::new(s.store.clone());
    let u = users
        .create(
            &s.admin,
            NewAccount {
                login: "bob".into(),
                email: "bob@private.example".into(),
                display_name: "Bob".into(),
                role: Role::Author,
                password: None,
                must_reset: false,
            },
        )
        .await
        .unwrap();
    users
        .update(
            &s.admin,
            u.id,
            UserPatch {
                role: Some(Role::Editor),
                display_name: Some("Bobby".into()),
                ..UserPatch::default()
            },
        )
        .await
        .unwrap();
    users.delete(&s.admin, u.id, None).await.unwrap();
    // The operator path emits too, with no actor.
    users
        .operator()
        .create(NewAccount {
            login: "carol".into(),
            email: "carol@private.example".into(),
            display_name: "Carol".into(),
            role: Role::Author,
            password: None,
            must_reset: false,
        })
        .await
        .unwrap();
    assert_eq!(
        s.kinds().await,
        [
            "user.created",
            "user.updated",
            "user.deleted",
            "user.created"
        ]
    );
    let events = s.events().await;
    assert_eq!(events[1].data["changed"], json!(["display_name", "role"]));
    assert_eq!(events[1].data["role"], "editor");
    assert_eq!(events[2].data["reassigned_to"], Value::Null);
    assert_eq!(events[3].data["actor"], Value::Null);
    let all = format!("{events:?}");
    assert!(!all.contains("private.example"), "{all}");
    for e in &events {
        let keys: Vec<&str> = e
            .data
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for k in ["email", "password_hash", "totp_secret"] {
            assert!(!keys.contains(&k), "{k}");
        }
    }
}

async fn import_finished_once(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["import.*"], None).await;
    let svc = ImportService::new(s.store.clone(), Arc::new(PermalinkRules::default()));
    let run = svc
        .start(&s.admin, "https://wp.example", false, false, None)
        .await
        .unwrap();
    run.finish(&svc, ImportRunState::Done, json!({"content": 3}))
        .await
        .unwrap();
    let events = s.events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "import.finished");
    assert_eq!(events[0].data["state"], "done");
    assert_eq!(events[0].data["stats"]["content"], 3);
}

async fn content_types_filter(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["content.created"], Some(&["page"])).await;
    s.content
        .create(&s.admin, draft("post", "A post"))
        .await
        .unwrap();
    assert!(s.events().await.is_empty());
    s.content
        .create(&s.admin, draft("page", "A page"))
        .await
        .unwrap();
    assert_eq!(s.kinds().await, ["content.created"]);
}

async fn payload_has_no_body_fields_or_email(db: &TestDb) {
    let s = site(db).await;
    s.hook(&["content.*"], None).await;
    let long_title = "T".repeat(400);
    s.content
        .create(&s.admin, draft("post", &long_title))
        .await
        .unwrap();
    let data = &s.events().await[0].data;
    let mut keys: Vec<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "actor",
            "author",
            "id",
            "locale",
            "modified_at",
            "parent",
            "path",
            "public",
            "published_at",
            "slug",
            "status",
            "title",
            "translation_group",
            "type",
            "version"
        ]
    );
    // Titles are cut at 300 characters.
    assert_eq!(data["title"].as_str().unwrap().chars().count(), 300);
    let all = data.to_string();
    assert!(!all.contains("secret body text") && !all.contains("An excerpt"));
}

backend_tests!(
    no_endpoint_no_rows,
    rolled_back_write_leaves_nothing,
    content_lifecycle_emits_each_type,
    scheduled_publish_has_null_actor,
    upload_emits_media_uploaded_only,
    comment_submit_moderate_delete,
    user_create_update_delete,
    import_finished_once,
    content_types_filter,
    payload_has_no_body_fields_or_email,
    translations_emit_created_and_updated,
);

/// M6-1: making a translation is `content.created` (with its locale and group), moving
/// an item to another locale or group `content.updated`; a localized create emits too.
async fn translations_emit_created_and_updated(db: &TestDb) {
    let s = site(db).await;
    s.store
        .read()
        .options()
        .set(
            "site.locales",
            &json!([{"tag": "en", "label": "English"}, {"tag": "de", "label": "Deutsch"}]),
            true,
        )
        .await
        .unwrap();
    s.hook(&["content.*"], None).await;
    let en = s
        .content
        .create(&s.admin, draft("post", "Hello"))
        .await
        .unwrap();
    let de = s
        .content
        .translate(&s.admin, en.id, "de")
        .await
        .unwrap()
        .item;
    let other = s
        .content
        .create_localized(&s.admin, draft("post", "Andere"), Some("de".into()), None)
        .await
        .unwrap();
    s.content
        .relocate(
            &s.admin,
            other.id,
            other.version,
            ferrite_core::Relocation {
                locale: Some("en".into()),
                translation_of: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        s.kinds().await,
        [
            "content.created",
            "content.created",
            "content.created",
            "content.updated"
        ]
    );
    let events = s.events().await;
    assert_eq!(events[1].data["locale"], "de");
    assert_eq!(events[1].data["id"], de.id.to_string());
    assert_eq!(events[1].data["translation_group"], en.id.to_string());
    assert_eq!(events[3].data["locale"], "en");
}
