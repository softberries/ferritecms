//! Preview links and preview reads (M6-3 task 8, design 15 §3.1, §7) on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, AutosaveDraft, ContentDraft, ContentService, CoreError, PermalinkRules, PreviewSource,
    SiteReader,
    preview_link::{PreviewLinkService, is_token},
    release::{EntryInput, NewRelease, ReleaseService},
    site::Clock,
};
use ferrite_domain::{
    ContentItem, EntryTransition, NewUser, PreviewTarget, Role, StagedChange, Timestamp, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

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

fn body(text: &str) -> serde_json::Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

fn draft(kind: &str, title: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: body(title),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

struct Site {
    store: Arc<dyn Store>,
    content: Arc<ContentService>,
    releases: ReleaseService,
    links: PreviewLinkService,
    reader: SiteReader,
    editor: Actor,
}

async fn site(db: &TestDb) -> Site {
    let store = db.shared_store();
    let rules = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    let clock: Clock = Arc::new(Timestamp::now);
    Site {
        releases: ReleaseService::new(store.clone(), rules.clone(), content.clone()),
        links: PreviewLinkService::new(store.clone()),
        reader: SiteReader::new(
            store.clone(),
            rules,
            Arc::new(|_: &ferrite_domain::ArtifactHash, _: &str| String::new()),
            clock,
        ),
        editor: actor(store.as_ref(), Role::Editor).await,
        store,
        content,
    }
}

impl Site {
    async fn item(&self, title: &str) -> ContentItem {
        self.content
            .create(&self.editor, draft("post", title))
            .await
            .unwrap()
    }
}

async fn link_resolves_until_expiry(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("x").await;
    let (link, token) = s
        .links
        .create(&s.editor, PreviewTarget::Item(it.id), Some(1))
        .await
        .unwrap();
    assert!(is_token(&token), "{token}");
    let p = s.links.resolve(&token).await.unwrap().unwrap();
    assert_eq!(p.link.id, link.id);
    assert_eq!(p.actor.user, s.editor.user);
    // Expired.
    let mut stale = link.clone();
    stale.id = ferrite_domain::PreviewLinkId::new();
    stale.expires_at = Timestamp::from_micros(Timestamp::now().as_micros() - 1).unwrap();
    s.store
        .read()
        .preview_links()
        .insert(&stale, "hash-of-expired")
        .await
        .unwrap();
    assert!(s.links.resolve("ferp_short").await.unwrap().is_none());
    assert!(
        s.links
            .create(&s.editor, PreviewTarget::Item(it.id), Some(169))
            .await
            .is_err()
    );
}

async fn revoked_link_resolves_to_none(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("x").await;
    let (link, token) = s
        .links
        .create(&s.editor, PreviewTarget::Item(it.id), None)
        .await
        .unwrap();
    assert_eq!(
        s.links
            .list(&s.editor, PreviewTarget::Item(it.id))
            .await
            .unwrap()
            .len(),
        1
    );
    s.links.revoke(&s.editor, link.id).await.unwrap();
    assert!(s.links.resolve(&token).await.unwrap().is_none());
}

async fn disabled_creator_link_dead(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("x").await;
    let (_, token) = s
        .links
        .create(&s.editor, PreviewTarget::Item(it.id), None)
        .await
        .unwrap();
    let mut u = s
        .store
        .read()
        .users()
        .get(s.editor.user)
        .await
        .unwrap()
        .unwrap();
    u.status = UserStatus::Disabled;
    s.store.read().users().update(&u).await.unwrap();
    assert!(s.links.resolve(&token).await.unwrap().is_none());
}

async fn creator_losing_rights_link_dead(db: &TestDb) {
    let s = site(db).await;
    let author = actor(s.store.as_ref(), Role::Author).await;
    let theirs = s
        .content
        .create(&author, draft("post", "theirs"))
        .await
        .unwrap();
    let (_, token) = s
        .links
        .create(&s.editor, PreviewTarget::Item(theirs.id), None)
        .await
        .unwrap();
    let mut u = s
        .store
        .read()
        .users()
        .get(s.editor.user)
        .await
        .unwrap()
        .unwrap();
    u.role = Role::Author;
    s.store.read().users().update(&u).await.unwrap();
    assert!(s.links.resolve(&token).await.unwrap().is_none());
}

async fn link_actor_has_content_read_only(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("x").await;
    let (_, token) = s
        .links
        .create(&s.editor, PreviewTarget::Item(it.id), None)
        .await
        .unwrap();
    let p = s.links.resolve(&token).await.unwrap().unwrap();
    let err = s
        .content
        .update(&p.actor, it.id, it.version, draft("post", "hijack"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Forbidden("content:write")),
        "{err:?}"
    );
    assert!(s.reader.preview(&p.actor, it.id).await.is_ok());
}

async fn create_rate_limited(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("x").await;
    for _ in 0..30 {
        s.links
            .create(&s.editor, PreviewTarget::Item(it.id), Some(1))
            .await
            .unwrap();
    }
    assert!(matches!(
        s.links
            .create(&s.editor, PreviewTarget::Item(it.id), Some(1))
            .await
            .unwrap_err(),
        CoreError::RateLimited { .. }
    ));
}

async fn token_never_stored(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("x").await;
    let (link, token) = s
        .links
        .create(&s.editor, PreviewTarget::Item(it.id), None)
        .await
        .unwrap();
    let found = s
        .store
        .read()
        .preview_links()
        .get_by_hash(&token)
        .await
        .unwrap();
    assert!(found.is_none(), "the token itself is not the stored hash");
    assert!(
        s.store
            .read()
            .preview_links()
            .get_by_hash(&sha_hex(token.as_bytes()))
            .await
            .unwrap()
            .is_some_and(|l| l.id == link.id)
    );
}

async fn working_overlay_uses_title_body_fields_seo_only(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("Stored").await;
    s.content
        .put_autosave(
            &s.editor,
            it.id,
            AutosaveDraft {
                base_version: it.version,
                title: "Working title".into(),
                excerpt: None,
                slug: Some("other-slug".into()),
                parent_id: None,
                menu_order: Some(3),
                terms: None,
                body: body("Working body"),
                fields: json!({}),
                seo: None,
            },
        )
        .await
        .unwrap();
    let read = s.reader.preview_working(&s.editor, it.id).await.unwrap();
    assert_eq!(read.value.source(), PreviewSource::Working);
    let item = read.value.item();
    assert_eq!(item.item.title, "Working title");
    assert!(item.item.excerpt.contains("Working body"));
    assert!(item.url.contains(&it.slug), "slug as saved: {}", item.url);
}

async fn working_overlay_without_slot_is_stored_item(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("Stored").await;
    let read = s.reader.preview_working(&s.editor, it.id).await.unwrap();
    assert_eq!(read.value.source(), PreviewSource::Stored);
    assert_eq!(read.value.item().item.title, "Stored");
}

async fn invalid_working_copy_still_previews(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("Stored").await;
    s.content
        .put_autosave(
            &s.editor,
            it.id,
            AutosaveDraft {
                base_version: it.version,
                title: String::new(),
                excerpt: None,
                slug: None,
                parent_id: None,
                menu_order: None,
                terms: None,
                body: body("half"),
                fields: json!({"nope": 1}),
                seo: None,
            },
        )
        .await
        .unwrap();
    let read = s.reader.preview_working(&s.editor, it.id).await.unwrap();
    assert_eq!(read.value.item().item.title, "");
}

async fn staged_preview_shows_entry_change(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("Stored").await;
    let r = s
        .releases
        .create(
            &s.editor,
            NewRelease {
                title: "R".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    s.releases
        .stage(
            &s.editor,
            r.id,
            r.version,
            EntryInput {
                item: it.id,
                transition: EntryTransition::None,
                changes: Some(StagedChange {
                    title: "Staged".into(),
                    slug: None,
                    parent_id: None,
                    excerpt: None,
                    body: body("staged"),
                    fields: json!({}),
                    menu_order: 0,
                    terms: vec![],
                    seo: None,
                }),
            },
        )
        .await
        .unwrap();
    let read = s
        .reader
        .preview_staged(&s.editor, it.id, r.id)
        .await
        .unwrap();
    assert_eq!(read.value.source(), PreviewSource::Staged);
    assert_eq!(read.value.item().item.title, "Staged");
}

async fn with_store_reads_through_sandbox(db: &TestDb) {
    let s = site(db).await;
    let it = s.item("Release page").await;
    let r = s
        .releases
        .create(
            &s.editor,
            NewRelease {
                title: "R".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    s.releases
        .stage(
            &s.editor,
            r.id,
            r.version,
            EntryInput {
                item: it.id,
                transition: EntryTransition::Publish,
                changes: None,
            },
        )
        .await
        .unwrap();
    assert!(s.reader.by_id(it.id).await.unwrap().value.is_none());
    let (sandbox, report, _) = s.releases.sandbox(&s.editor, r.id).await.unwrap();
    assert_eq!(report.applied, 1);
    let sandbox: Arc<dyn Store> = Arc::new(sandbox);
    let preview = s.reader.with_store(sandbox.clone());
    assert!(preview.by_id(it.id).await.unwrap().value.is_some());
    assert!(preview.front_page(1).await.is_ok());
    drop(preview);
    drop(sandbox);
    // Live is untouched.
    let now = s.store.read().content().get(it.id).await.unwrap().unwrap();
    assert_eq!(now.status, ferrite_domain::ContentStatus::Draft);
}

backend_tests!(
    link_resolves_until_expiry,
    revoked_link_resolves_to_none,
    disabled_creator_link_dead,
    creator_losing_rights_link_dead,
    link_actor_has_content_read_only,
    create_rate_limited,
    token_never_stored,
    working_overlay_uses_title_body_fields_seo_only,
    working_overlay_without_slot_is_stored_item,
    invalid_working_copy_still_previews,
    staged_preview_shows_entry_change,
    with_store_reads_through_sandbox,
);

fn sha_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    ferrite_domain::to_hex(&Sha256::digest(bytes))
}
