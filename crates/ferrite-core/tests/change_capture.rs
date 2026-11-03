//! Change capture (M6-5 tasks 3–4, design 17 §3): writes by an agent-marked actor are
//! recorded in their own transaction; writes without an agent record nothing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use ferrite_core::{
    AuthService, ContentService, CoreError, MediaPatch, PermalinkRules,
    auth::SiteKey,
    comments::{CommentInput, CommentService, Commenter, Submitted},
};
use ferrite_domain::{Artifact, ArtifactHash, ArtifactKind, CommentStatus, Image, Timestamp};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

mod changes_common;
use changes_common::*;

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
    term_delete_image_keeps_links,
    menu_set_new_menu_is_created,
    type_update_under_types_lock_is_captured,
    redirect_create_over_existing_source_has_a_before_image,
    settings_update_records_one_change_per_key,
    permalink_change_captured_under_lock,
    plugin_secret_image_stays_sealed,
    theme_activate_records_previous_theme,
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

async fn term_delete_image_keeps_links(db: &TestDb) {
    let s = site(db).await;
    let tax = ferrite_core::TaxonomyService::new(s.store.clone());
    let term = tax
        .create_term(&s.admin, "category", "News", None, None, "d")
        .await
        .unwrap();
    let mut d = draft("post", "Linked");
    d.terms = vec![term.id];
    let item = s.content.create(&s.admin, d).await.unwrap();
    tax.delete_term(&s.agent, term.id).await.unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].op, "term.delete");
    assert_eq!(ch[0].after, Image::Absent);
    let before = opened(&ch[0].before);
    assert_eq!(before["name"], "News");
    assert_eq!(before["items"], json!([item.id.to_string()]));
    assert_eq!(before["items_total"], 1);
    assert_eq!(ch[0].label, "News");
}

async fn menu_set_new_menu_is_created(db: &TestDb) {
    let s = site(db).await;
    let menus = ferrite_core::MenuService::new(s.store.clone());
    let menu = ferrite_domain::Menu {
        name: "main".into(),
        label: "Main".into(),
        items: vec![ferrite_domain::MenuItem {
            id: ferrite_domain::MenuItemId::new(),
            parent_id: None,
            position: 0,
            kind: ferrite_domain::MenuItemKind::Url,
            target_id: None,
            url: Some("https://example.com/".into()),
            label: "Home".into(),
        }],
        locale: "en".into(),
        translation_of: None,
    };
    menus.put(&s.agent, &menu).await.unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1);
    assert_eq!(
        (ch[0].op.as_str(), ch[0].object_kind.as_str()),
        ("menu.put", "menu")
    );
    assert_eq!(ch[0].object_id, "main");
    assert_eq!(ch[0].before, Image::Absent);
    assert_eq!(opened(&ch[0].after)["items"][0]["label"], "Home");
}

async fn type_update_under_types_lock_is_captured(db: &TestDb) {
    let s = site(db).await;
    let types = ferrite_core::ContentTypeService::new(s.store.clone());
    types
        .create(
            &s.agent,
            ferrite_core::NewType {
                name: "book".into(),
                label: "Books".into(),
                hierarchical: false,
                has_archive: true,
                fields: None,
            },
        )
        .await
        .unwrap();
    types
        .update(
            &s.agent,
            "book",
            ferrite_core::TypePatch {
                label: Some("Library".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ch = s.changes().await;
    let ops: Vec<&str> = ch.iter().map(|c| c.op.as_str()).collect();
    assert_eq!(ops, ["type.create", "type.update"]);
    assert_eq!(ch[0].before, Image::Absent);
    assert_eq!(opened(&ch[1].before)["label"], "Books");
    assert_eq!(opened(&ch[1].after)["label"], "Library");
    assert!(opened(&ch[1].after).get("created_at").is_none());
}

async fn redirect_create_over_existing_source_has_a_before_image(db: &TestDb) {
    let s = site(db).await;
    let r = ferrite_core::RedirectService::new(s.store.clone());
    let first = r.upsert(&s.admin, "/old", "/new", 301).await.unwrap();
    let second = r.upsert(&s.agent, "/old", "/newer", 302).await.unwrap();
    assert_eq!(first.id, second.id);
    r.upsert(&s.agent, "/fresh", "/x", 301).await.unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 2);
    assert_eq!(ch[0].object_id, first.id.to_string());
    assert_eq!(opened(&ch[0].before)["target"], "/new");
    assert_eq!(opened(&ch[0].after)["code"], 302);
    assert!(opened(&ch[0].after).get("hits").is_none());
    assert_eq!(ch[1].before, Image::Absent);
    assert_eq!(opened(&ch[1].after)["source_path"], "/fresh");
    let id: ferrite_domain::RedirectId = ch[1].object_id.parse().unwrap();
    r.update(&s.agent, id, "/fresh2", "/y", 301).await.unwrap();
    r.delete(&s.agent, id).await.unwrap();
    let ch = s.changes().await;
    let ops: Vec<&str> = ch.iter().map(|c| c.op.as_str()).collect();
    assert_eq!(
        ops,
        [
            "redirect.create",
            "redirect.create",
            "redirect.update",
            "redirect.delete"
        ]
    );
    assert_eq!(ch[3].after, Image::Absent);
}

async fn settings_update_records_one_change_per_key(db: &TestDb) {
    let s = site(db).await;
    ferrite_core::OptionService::new(s.store.clone())
        .set_many(
            &s.agent,
            &[
                ("site.title".to_owned(), Some(json!("New title"))),
                ("site.description".to_owned(), Some(json!("A tagline"))),
            ],
        )
        .await
        .unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 2);
    assert!(
        ch.iter()
            .all(|c| c.op == "settings.update" && c.object_kind == "option")
    );
    let keys: Vec<&str> = ch.iter().map(|c| c.object_id.as_str()).collect();
    assert_eq!(keys, ["site.title", "site.description"]);
    assert_eq!(opened(&ch[0].after), json!({"value": "New title"}));
    assert_eq!(s.sets().await[0].changes, 2);
}

async fn permalink_change_captured_under_lock(db: &TestDb) {
    let s = site(db).await;
    ferrite_core::OptionService::new(s.store.clone())
        .set_many(
            &s.agent,
            &[("permalink.post".to_owned(), Some(json!("/blog/%slug%/")))],
        )
        .await
        .unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].object_id, "permalink.post");
    assert_eq!(ch[0].before, Image::Absent);
}

async fn plugin_secret_image_stays_sealed(db: &TestDb) {
    let s = site(db).await;
    let svc = ferrite_core::ExtensionAdmin::new(s.store.clone(), SiteKey::generate().unwrap());
    let a = Artifact {
        hash: ArtifactHash::parse(&"0a".repeat(32)).unwrap(),
        kind: ArtifactKind::Plugin,
        bytes: 42,
        mime: "application/wasm".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    let now = Timestamp::now();
    let row = ferrite_domain::InstalledPlugin {
        name: "seo".into(),
        version: "0.1.0".into(),
        artifact_hash: a.hash.clone(),
        manifest: json!({
            "plugin": {"name": "seo", "version": "0.1.0"},
            "settings": {
                "limit": {"type": "number", "label": "Limit"},
                "api_key": {"type": "secret", "label": "API key"},
            },
        }),
        granted: json!({}),
        enabled: false,
        disabled_reason: None,
        installed_at: now,
        updated_at: now,
    };
    svc.install_plugin(&s.admin, &a, &row).await.unwrap();
    let plaintext = "sk-live-0123456789";
    let mut patch = serde_json::Map::new();
    patch.insert("api_key".into(), json!(plaintext));
    patch.insert("limit".into(), json!(5));
    svc.set_plugin_settings(&s.agent, "seo", patch)
        .await
        .unwrap();
    svc.set_plugin_enabled(&s.agent, "seo", true).await.unwrap();
    let ch = s.changes().await;
    let ops: Vec<&str> = ch.iter().map(|c| c.op.as_str()).collect();
    assert_eq!(ops, ["plugin.settings", "plugin.settings", "plugin.enable"]);
    let secret = ch
        .iter()
        .find(|c| c.object_id == "plugin.seo.settings.api_key")
        .unwrap();
    let text = opened(&secret.after).to_string();
    assert!(text.contains("\"sealed\":\"v1."), "{text}");
    assert!(!text.contains(plaintext), "{text}");
    assert_eq!(opened(&ch[2].before)["enabled"], false);
    assert_eq!(opened(&ch[2].after)["enabled"], true);
}

async fn theme_activate_records_previous_theme(db: &TestDb) {
    let s = site(db).await;
    let svc = ferrite_core::ExtensionAdmin::new(s.store.clone(), SiteKey::generate().unwrap());
    let art = |seed: u8| Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind: ArtifactKind::Theme,
        bytes: 42,
        mime: "application/octet-stream".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    let theme = |name: &str, a: &Artifact| ferrite_domain::InstalledTheme {
        name: name.into(),
        version: "1.0.0".into(),
        artifact_hash: a.hash.clone(),
        manifest: json!({"name": name}),
        active: false,
        installed_at: Timestamp::now(),
    };
    let (light, dark) = (art(1), art(2));
    svc.install_theme(&s.admin, &light, &theme("light", &light), true)
        .await
        .unwrap();
    svc.install_theme(&s.admin, &dark, &theme("dark", &dark), false)
        .await
        .unwrap();
    assert!(s.sets().await.is_empty());
    svc.activate_theme(&s.agent, "dark", &dark.hash)
        .await
        .unwrap();
    let ch = s.changes().await;
    assert_eq!(ch.len(), 1);
    assert_eq!(
        (ch[0].op.as_str(), ch[0].object_id.as_str()),
        ("theme.activate", "active")
    );
    assert_eq!(
        opened(&ch[0].before),
        json!({"name": "light", "artifact": light.hash.as_str()})
    );
    assert_eq!(opened(&ch[0].after)["name"], "dark");
}
