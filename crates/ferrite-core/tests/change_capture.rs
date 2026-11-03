//! Change capture (M6-5 tasks 3–4, design 17 §3): writes by an agent-marked actor are
//! recorded in their own transaction; writes without an agent record nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use ferrite_core::{
    Actor, Agent, AuthService, ContentDraft, ContentService, CoreError, MediaPatch, MediaService,
    NewUpload, PermalinkRules,
    auth::SiteKey,
    changes::open_image,
    comments::{CommentInput, CommentService, Commenter, Submitted},
    permalink::Permalinks,
};
use ferrite_domain::{
    ApiTokenId, Artifact, ArtifactHash, ArtifactKind, Change, ChangeSet, ChangeSetQuery,
    CommentStatus, Image, NewUser, Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

pub struct Site {
    pub store: Arc<dyn Store>,
    pub content: Arc<ContentService>,
    pub rules: Arc<dyn Permalinks>,
    pub admin: Actor,
    /// `admin` acting through a personal token over MCP.
    pub agent: Actor,
}

pub async fn actor(store: &dyn Store, role: Role) -> Actor {
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

pub async fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let admin = actor(store.as_ref(), Role::Admin).await;
    let rules: Arc<dyn Permalinks> = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    Site {
        store,
        content,
        rules,
        agent: admin.with_agent(Agent::Token(ApiTokenId::new())),
        admin,
    }
}

impl Site {
    /// Every set of the admin's, newest first.
    pub async fn sets(&self) -> Vec<ChangeSet> {
        self.store
            .read()
            .change_sets()
            .list(&ChangeSetQuery {
                user: None,
                agent_id: None,
                open: None,
                idle_since: Timestamp::now(),
                limit: 200,
                cursor: None,
            })
            .await
            .unwrap()
            .items
    }

    /// Every change of every set, oldest first.
    pub async fn changes(&self) -> Vec<Change> {
        let mut out = Vec::new();
        for s in self.sets().await.into_iter().rev() {
            out.extend(self.store.read().change_sets().changes(s.id).await.unwrap());
        }
        out
    }

    pub fn media(&self) -> MediaService {
        MediaService::new(self.store.clone(), self.content.clone(), self.rules.clone())
    }
}

pub fn draft(kind: &str, title: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Body"}]}
        ]}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

pub fn png(seed: u8) -> NewUpload {
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

pub fn opened(img: &Image) -> Value {
    open_image(img).unwrap().expect("present image")
}

fn next_ip() -> IpAddr {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    IpAddr::V4(Ipv4Addr::from(0x0b00_0000 + n))
}

backend_tests!(
    agent_create_update_publish_records_three_changes,
    plain_actor_records_nothing,
    stale_version_conflict_records_nothing,
    media_upload_records_one_media_change_not_content_plus_media,
    media_update_captures_alt,
    trash_records_trashed_from,
    comment_reply_and_moderate_are_captured,
    before_and_after_hashes_chain_across_two_updates,
    translate_is_captured_as_a_create,
);

async fn agent_create_update_publish_records_three_changes(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.agent, draft("post", "One"))
        .await
        .unwrap();
    let mut d = draft("post", "One, edited");
    d.slug = Some(item.slug.clone());
    let item = s
        .content
        .update(&s.agent, item.id, item.version, d)
        .await
        .unwrap();
    s.content
        .publish(&s.agent, item.id, item.version)
        .await
        .unwrap();
    let sets = s.sets().await;
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].changes, 3);
    assert_eq!(sets[0].user_id, Some(s.admin.user));
    assert_eq!(sets[0].client, "AI assistant", "no token row");
    let ch = s.changes().await;
    let ops: Vec<&str> = ch.iter().map(|c| c.op.as_str()).collect();
    assert_eq!(ops, ["content.create", "content.update", "content.publish"]);
    assert_eq!(ch.iter().map(|c| c.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(ch.iter().all(|c| c.object_kind == "content"));
    assert!(ch.iter().all(|c| c.object_id == item.id.to_string()));
    assert_eq!(ch[0].before, Image::Absent);
    assert_eq!(opened(&ch[0].after)["title"], "One");
    assert_eq!(ch[1].label, "One, edited");
    assert_eq!(opened(&ch[2].before)["status"], "draft");
    assert_eq!(opened(&ch[2].after)["status"], "published");
}

async fn plain_actor_records_nothing(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "One"))
        .await
        .unwrap();
    s.content
        .publish(&s.admin, item.id, item.version)
        .await
        .unwrap();
    assert!(s.sets().await.is_empty());
}

async fn stale_version_conflict_records_nothing(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "One"))
        .await
        .unwrap();
    let err = s
        .content
        .update(&s.agent, item.id, item.version + 5, draft("post", "Two"))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Conflict { .. }), "{err:?}");
    assert!(
        s.sets().await.is_empty(),
        "a rolled-back write leaves no set"
    );
}

async fn media_upload_records_one_media_change_not_content_plus_media(db: &TestDb) {
    let s = site(db).await;
    let up = s.media().record_upload(&s.agent, png(1)).await.unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1, "{ch:?}");
    assert_eq!(ch[0].op, "media.upload");
    assert_eq!(ch[0].object_kind, "media");
    assert_eq!(ch[0].object_id, up.item.id.to_string());
    let after = opened(&ch[0].after);
    assert_eq!(after["alt"], "An image");
    assert_eq!(after["status"], "published");
}

async fn media_update_captures_alt(db: &TestDb) {
    let s = site(db).await;
    let up = s.media().record_upload(&s.admin, png(2)).await.unwrap();
    s.media()
        .update(
            &s.agent,
            up.item.id,
            up.item.version,
            MediaPatch {
                title: None,
                alt: Some("A better description".into()),
            },
        )
        .await
        .unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].op, "media.update");
    assert_eq!(opened(&ch[0].before)["alt"], "An image");
    assert_eq!(opened(&ch[0].after)["alt"], "A better description");
}

async fn trash_records_trashed_from(db: &TestDb) {
    let s = site(db).await;
    let up = s.media().record_upload(&s.admin, png(3)).await.unwrap();
    s.media()
        .trash(&s.agent, up.item.id, up.item.version)
        .await
        .unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].op, "media.trash");
    let before = opened(&ch[0].before);
    let after = opened(&ch[0].after);
    assert!(before.get("trashed_from").is_none());
    assert_eq!(after["trashed_from"], "published");
    assert_eq!(after["status"], "trash");
}

async fn comment_reply_and_moderate_are_captured(db: &TestDb) {
    let s = site(db).await;
    let auth = Arc::new(AuthService::new(
        s.store.clone(),
        SiteKey::generate().unwrap(),
    ));
    let comments = CommentService::new(s.store.clone(), auth, s.rules.clone());
    let item = s
        .content
        .create(&s.admin, draft("post", "Talk"))
        .await
        .unwrap();
    let item = s
        .content
        .publish(&s.admin, item.id, item.version)
        .await
        .unwrap();
    let input = |body: &str| CommentInput {
        item: item.id,
        kind: "post".into(),
        parent: None,
        author_name: "Ann".into(),
        author_email: "ann@example.com".into(),
        author_url: String::new(),
        body: body.into(),
        honeypot: String::new(),
    };
    let Submitted::Accepted(anon) = comments
        .submit(input("anonymous"), Commenter::Anonymous, next_ip())
        .await
        .unwrap()
    else {
        panic!("discarded")
    };
    assert!(
        s.sets().await.is_empty(),
        "an anonymous comment is never captured"
    );
    let Submitted::Accepted(reply) = comments
        .submit(
            input("from the assistant"),
            Commenter::User(s.agent),
            next_ip(),
        )
        .await
        .unwrap()
    else {
        panic!("discarded")
    };
    comments
        .moderate(&s.agent, anon.id, CommentStatus::Spam)
        .await
        .unwrap();
    let ch = s.changes().await;
    let ops: Vec<&str> = ch.iter().map(|c| c.op.as_str()).collect();
    assert_eq!(ops, ["comment.create", "comment.moderate"]);
    assert_eq!(ch[0].object_id, reply.id.to_string());
    let after = opened(&ch[0].after);
    assert!(after.get("body").is_none() && after.get("author_email").is_none());
    assert_eq!(opened(&ch[1].after)["status"], "spam");
}

async fn before_and_after_hashes_chain_across_two_updates(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "One"))
        .await
        .unwrap();
    let item = s
        .content
        .update(&s.agent, item.id, item.version, draft("post", "Two"))
        .await
        .unwrap();
    s.content
        .update(&s.agent, item.id, item.version, draft("post", "Three"))
        .await
        .unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 2);
    assert_eq!(ch[0].after.hash(), ch[1].before.hash());
    assert_ne!(ch[0].before.hash(), ch[0].after.hash());
}

async fn translate_is_captured_as_a_create(db: &TestDb) {
    let s = site(db).await;
    // A second language for the site (M6-1).
    ferrite_core::OptionService::new(s.store.clone())
        .set_many(
            &s.admin,
            &[(
                "site.locales".to_owned(),
                Some(json!([{"tag": "en"}, {"tag": "de"}])),
            )],
        )
        .await
        .unwrap();
    let rules = PermalinkRules::default()
        .with_locales(ferrite_core::locales::read(s.store.read()).await.unwrap());
    let content = ContentService::new(s.store.clone(), Arc::new(rules));
    let item = content
        .create(&s.admin, draft("post", "Hello"))
        .await
        .unwrap();
    let t = content.translate(&s.agent, item.id, "de").await.unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].op, "content.translate");
    assert_eq!(ch[0].object_id, t.item.id.to_string());
    assert_eq!(ch[0].before, Image::Absent);
    assert_eq!(opened(&ch[0].after)["locale"], "de");
}
