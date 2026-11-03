//! Reviewing and reverting change sets (M6-5 tasks 5–6, design 17 §7–§8).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use ferrite_core::{
    Actor, AuthService, CoreError,
    auth::SiteKey,
    changes::{
        ChangeSetFilter, ChangeSetService, ConflictReason, Images, RevertError, RevertRequest,
    },
    comments::{CommentInput, CommentService, Commenter, Submitted},
};
use ferrite_domain::{AuditQuery, ChangeSetId, CommentStatus, ContentStatus, Role, Timestamp};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

mod changes_common;
use changes_common::*;

fn service(s: &Site) -> ChangeSetService {
    ChangeSetService::new(s.store.clone(), s.rules.clone())
}

async fn only_set(s: &Site) -> ChangeSetId {
    let sets = s.sets().await;
    assert_eq!(sets.len(), 1, "{sets:?}");
    sets[0].id
}

fn all() -> RevertRequest {
    RevertRequest::default()
}

fn conflicts(e: RevertError) -> Vec<(u32, ConflictReason)> {
    match e {
        RevertError::Conflicts(c) => c.into_iter().map(|c| (c.seq, c.reason)).collect(),
        other => panic!("expected conflicts, got {other:?}"),
    }
}

backend_tests!(
    revert_create_trashes_the_item,
    revert_publish_unpublishes_and_keeps_published_at,
    revert_two_updates_newest_first_without_force,
    person_edit_between_is_changed_since_naming_the_revision_author,
    force_overwrites_and_marks_forced,
    later_change_in_same_set_refuses_single_revert,
    any_failure_rolls_back_every_change,
    dry_run_writes_nothing,
    revert_closes_the_set_and_audits_and_emits_event,
    media_upload_revert_trashes_media,
    comment_reply_revert_trashes_comment,
    only_owner_or_admin_may_revert,
    images_hidden_without_area_read,
    reverting_person_role_is_checked,
    revert_writes_no_change_set,
    scheduled_time_passed_leaves_draft_with_note,
    assistant_cannot_revert,
    bad_seq_lists_are_refused,
    revert_term_delete_reinserts_with_links,
    revert_term_create_refused_with_children,
    revert_menu_put_restores_tree_and_new_menu_is_deleted,
    revert_type_create_refused_while_it_has_items,
    revert_redirect_update_restores_source_and_origin,
    revert_settings_in_one_set_many_call,
    revert_permalink_change_takes_types_lock_and_enqueues_redirects,
    revert_plugin_settings_restores_sealed_secret,
    revert_theme_activation_requires_same_artifact,
    revert_plugin_disable_enables,
    assistant_term_then_tagging_reverts_without_force,
    revert_publish_emits_unpublished_webhook,
    revert_reschedule_restores_scheduled_time,
);

async fn revert_create_trashes_the_item(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.agent, draft("post", "Made by AI"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    let report = service(&s).revert(&s.admin, set, all()).await.unwrap();
    assert_eq!(report.reverted, [1]);
    let now = s.content.get(item.id).await.unwrap();
    assert_eq!(
        now.status,
        ContentStatus::Trash,
        "trashed, never deleted (D7)"
    );
}

async fn revert_publish_unpublishes_and_keeps_published_at(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "One"))
        .await
        .unwrap();
    let item = s
        .content
        .publish(&s.admin, item.id, item.version)
        .await
        .unwrap();
    let first = item.published_at;
    let item = s
        .content
        .unpublish(&s.admin, item.id, item.version)
        .await
        .unwrap();
    s.content
        .publish(&s.agent, item.id, item.version)
        .await
        .unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let now = s.content.get(item.id).await.unwrap();
    assert_eq!(now.status, ContentStatus::Draft);
    assert_eq!(now.published_at, first);
}

async fn revert_two_updates_newest_first_without_force(db: &TestDb) {
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
    let set = only_set(&s).await;
    let report = service(&s).revert(&s.admin, set, all()).await.unwrap();
    assert_eq!(report.reverted, [2, 1]);
    assert!(report.forced.is_empty());
    let now = s.content.get(item.id).await.unwrap();
    assert_eq!(now.title, "One");
    let revs = s.store.read().revisions().list(item.id, 1).await.unwrap();
    assert_eq!(revs[0].note.as_deref(), Some("Reverted an AI change"));
    assert_eq!(
        revs[0].author_id, s.admin.user,
        "written as the reverting person"
    );
}

async fn person_edit_between_is_changed_since_naming_the_revision_author(db: &TestDb) {
    let s = site(db).await;
    let editor = actor(s.store.as_ref(), Role::Editor).await;
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
        .update(
            &editor,
            item.id,
            item.version,
            draft("post", "Edited by a person"),
        )
        .await
        .unwrap();
    let set = only_set(&s).await;
    let err = service(&s).revert(&s.admin, set, all()).await.unwrap_err();
    let got = conflicts(err);
    assert_eq!(got.len(), 1);
    match &got[0] {
        (1, ConflictReason::ChangedSince { by, at }) => {
            assert_eq!(*by, Some(editor.user));
            assert!(at.is_some());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        s.content.get(item.id).await.unwrap().title,
        "Edited by a person"
    );
}

async fn force_overwrites_and_marks_forced(db: &TestDb) {
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
        .update(&s.admin, item.id, item.version, draft("post", "Person"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    let report = service(&s)
        .revert(
            &s.admin,
            set,
            RevertRequest {
                force: true,
                ..all()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.forced, [1]);
    assert_eq!(s.content.get(item.id).await.unwrap().title, "One");
    let c = s
        .store
        .read()
        .change_sets()
        .change(set, 1)
        .await
        .unwrap()
        .unwrap();
    assert!(c.revert_forced);
    assert_eq!(c.reverted_by, Some(s.admin.user));
}

async fn later_change_in_same_set_refuses_single_revert(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.agent, draft("post", "One"))
        .await
        .unwrap();
    s.content
        .update(&s.agent, item.id, item.version, draft("post", "Two"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    let err = service(&s)
        .revert(
            &s.admin,
            set,
            RevertRequest {
                changes: Some(vec![1]),
                force: true,
                ..all()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(conflicts(err), [(1, ConflictReason::LaterChange(2))]);
    // Reverting the later one alone is fine.
    let report = service(&s)
        .revert(
            &s.admin,
            set,
            RevertRequest {
                changes: Some(vec![2]),
                ..all()
            },
        )
        .await
        .unwrap();
    assert_eq!(report.reverted, [2]);
    assert_eq!(s.content.get(item.id).await.unwrap().title, "One");
}

async fn any_failure_rolls_back_every_change(db: &TestDb) {
    let s = site(db).await;
    ferrite_core::OptionService::new(s.store.clone())
        .set_many(
            &s.agent,
            &[("site.title".to_owned(), Some(json!("AI title")))],
        )
        .await
        .unwrap();
    let item = s
        .content
        .create(&s.agent, draft("post", "AI post"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    // The same user, now an editor: the post may be trashed, the setting not.
    let demoted = Actor::new(s.admin.user, Role::Editor);
    let err = service(&s).revert(&demoted, set, all()).await.unwrap_err();
    match err {
        RevertError::Failed { seq, error } => {
            assert_eq!(seq, 1);
            assert!(
                matches!(error, CoreError::Forbidden("manage_settings")),
                "{error:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        s.content.get(item.id).await.unwrap().status,
        ContentStatus::Draft,
        "the trash of the post rolled back with the rest"
    );
    let c = s
        .store
        .read()
        .change_sets()
        .change(set, 2)
        .await
        .unwrap()
        .unwrap();
    assert!(c.reverted_at.is_none());
}

async fn dry_run_writes_nothing(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.agent, draft("post", "AI post"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    let report = service(&s)
        .revert(
            &s.admin,
            set,
            RevertRequest {
                dry_run: true,
                ..all()
            },
        )
        .await
        .unwrap();
    assert!(report.dry_run);
    assert_eq!(report.reverted, [1]);
    assert_eq!(
        s.content.get(item.id).await.unwrap().status,
        ContentStatus::Draft
    );
    let got = s
        .store
        .read()
        .change_sets()
        .get(set)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.reverted_changes, 0);
    assert!(got.open);
}

async fn revert_closes_the_set_and_audits_and_emits_event(db: &TestDb) {
    let s = site(db).await;
    let hooks = ferrite_core::webhooks::WebhookService::new(
        s.store.clone(),
        SiteKey::from_hex(&"ab".repeat(32)).unwrap(),
        ferrite_core::webhooks::UrlPolicy {
            local_targets: false,
            allow_ip: |_| true,
        },
    );
    hooks
        .create(
            &s.admin,
            ferrite_core::webhooks::WebhookInput {
                name: "reverts".into(),
                url: "https://hooks.example.com/x".into(),
                events: vec!["change_set.reverted".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    s.content
        .create(&s.agent, draft("post", "AI post"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let got = s
        .store
        .read()
        .change_sets()
        .get(set)
        .await
        .unwrap()
        .unwrap();
    assert!(!got.open);
    assert_eq!(got.reverted_by, Some(s.admin.user));
    assert!(got.reverted_at.is_some());
    let audit = s
        .store
        .read()
        .audit()
        .list(&AuditQuery {
            action: Some("change_set.revert".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap()
        .items;
    assert_eq!(audit.len(), 1);
    assert_eq!(
        audit[0].target_id.as_deref(),
        Some(set.to_string().as_str())
    );
    assert_eq!(audit[0].details["changes"], json!([1]));
    let w = s.store.read().webhooks();
    let endpoint = &w.list_endpoints().await.unwrap()[0];
    let page = w
        .list_deliveries(endpoint.id, &Default::default())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].event_type, "change_set.reverted");
    // Later writes of the assistant open a new set.
    s.content
        .create(&s.agent, draft("post", "Later"))
        .await
        .unwrap();
    assert_eq!(s.sets().await.len(), 2);
}

async fn media_upload_revert_trashes_media(db: &TestDb) {
    let s = site(db).await;
    let up = s.media().record_upload(&s.agent, png(4)).await.unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let now = s.content.get(up.item.id).await.unwrap();
    assert_eq!(now.status, ContentStatus::Trash);
    let from = s
        .store
        .read()
        .meta()
        .get(up.item.id, ferrite_core::TRASHED_FROM_META)
        .await
        .unwrap();
    assert_eq!(from, Some(json!("published")));
}

fn next_ip() -> IpAddr {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    IpAddr::V4(Ipv4Addr::from(0x0c00_0000 + n))
}

async fn comment_reply_revert_trashes_comment(db: &TestDb) {
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
    let Submitted::Accepted(reply) = comments
        .submit(
            CommentInput {
                item: item.id,
                kind: "post".into(),
                parent: None,
                author_name: "Ann".into(),
                author_email: "ann@example.com".into(),
                author_url: String::new(),
                body: "Hello".into(),
                honeypot: String::new(),
            },
            Commenter::User(s.agent),
            next_ip(),
        )
        .await
        .unwrap()
    else {
        panic!("discarded")
    };
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let c = s
        .store
        .read()
        .comments()
        .get(reply.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c.status, CommentStatus::Trash);
}

async fn only_owner_or_admin_may_revert(db: &TestDb) {
    let s = site(db).await;
    let editor = actor(s.store.as_ref(), Role::Editor).await;
    let other = actor(s.store.as_ref(), Role::Admin).await;
    let mine = editor.with_agent(ferrite_core::Agent::Token(ferrite_domain::ApiTokenId::new()));
    s.content
        .create(&mine, draft("post", "Editor's AI"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    let editor2 = actor(s.store.as_ref(), Role::Editor).await;
    let svc = service(&s);
    assert!(matches!(
        svc.revert(&editor2, set, all()).await,
        Err(RevertError::Core(CoreError::NotFound))
    ));
    assert!(matches!(
        svc.get(&editor2, set).await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        svc.list(
            &editor2,
            &ChangeSetFilter {
                all_users: true,
                ..Default::default()
            }
        )
        .await,
        Err(CoreError::Forbidden("manage_users"))
    ));
    assert_eq!(svc.get(&editor, set).await.unwrap().changes.len(), 1);
    let listed = svc
        .list(
            &other,
            &ChangeSetFilter {
                all_users: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    // A contributor whose assistant created drafts can revert them (trash own drafts).
    let contributor = actor(s.store.as_ref(), Role::Contributor).await;
    let theirs =
        contributor.with_agent(ferrite_core::Agent::Token(ferrite_domain::ApiTokenId::new()));
    let item = s
        .content
        .create(&theirs, draft("post", "Contributor's AI"))
        .await
        .unwrap();
    let csets = svc
        .list(&contributor, &ChangeSetFilter::default())
        .await
        .unwrap()
        .items;
    assert_eq!(csets.len(), 1);
    svc.revert(&contributor, csets[0].id, all()).await.unwrap();
    assert_eq!(
        s.content.get(item.id).await.unwrap().status,
        ContentStatus::Trash
    );
    svc.revert(&other, set, all()).await.unwrap();
}

async fn images_hidden_without_area_read(db: &TestDb) {
    let s = site(db).await;
    s.content
        .create(&s.agent, draft("post", "AI post"))
        .await
        .unwrap();
    ferrite_core::OptionService::new(s.store.clone())
        .set_many(
            &s.agent,
            &[("site.title".to_owned(), Some(json!("AI title")))],
        )
        .await
        .unwrap();
    let set = only_set(&s).await;
    let svc = service(&s);
    let detail = svc.change(&s.admin, set, 1).await.unwrap();
    match detail.images {
        Images::Shown { before, after } => {
            assert_eq!(before, None);
            assert_eq!(after.unwrap()["title"], "AI post");
        }
        Images::Hidden => panic!("hidden for an admin"),
    }
    // The same user demoted to editor: the content stays readable, settings do not.
    let demoted = Actor::new(s.admin.user, Role::Editor);
    assert!(matches!(
        svc.change(&demoted, set, 1).await.unwrap().images,
        Images::Shown { .. }
    ));
    let hidden = svc.change(&demoted, set, 2).await.unwrap();
    assert_eq!(hidden.images, Images::Hidden);
    assert_eq!(hidden.change.op, "settings.update");
}

async fn reverting_person_role_is_checked(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "One"))
        .await
        .unwrap();
    s.content
        .publish(&s.agent, item.id, item.version)
        .await
        .unwrap();
    let set = only_set(&s).await;
    let demoted = Actor::new(s.admin.user, Role::Contributor);
    match service(&s).revert(&demoted, set, all()).await {
        Err(RevertError::Failed {
            seq: 1,
            error: CoreError::Forbidden(cap),
        }) => assert!(cap.starts_with("publish_"), "{cap}"),
        other => panic!("{other:?}"),
    }
}

async fn revert_writes_no_change_set(db: &TestDb) {
    let s = site(db).await;
    s.content
        .create(&s.agent, draft("post", "AI post"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    assert_eq!(s.sets().await.len(), 1, "a revert is a person's write");
    assert_eq!(s.changes().await.len(), 1);
}

async fn scheduled_time_passed_leaves_draft_with_note(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.admin, draft("post", "Soon"))
        .await
        .unwrap();
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 1_000_000).unwrap();
    let item = s
        .content
        .schedule(&s.admin, item.id, item.version, at)
        .await
        .unwrap();
    s.content
        .publish(&s.agent, item.id, item.version)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let set = only_set(&s).await;
    let report = service(&s).revert(&s.admin, set, all()).await.unwrap();
    assert_eq!(report.notes.len(), 1, "{report:?}");
    assert!(report.notes[0].1.contains("scheduled time has passed"));
    assert_eq!(
        s.content.get(item.id).await.unwrap().status,
        ContentStatus::Draft
    );
}

async fn assistant_cannot_revert(db: &TestDb) {
    let s = site(db).await;
    s.content
        .create(&s.agent, draft("post", "AI post"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    assert!(matches!(
        service(&s).revert(&s.agent, set, all()).await,
        Err(RevertError::Core(CoreError::Forbidden(_)))
    ));
}

async fn bad_seq_lists_are_refused(db: &TestDb) {
    let s = site(db).await;
    let item = s
        .content
        .create(&s.agent, draft("post", "AI post"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    let svc = service(&s);
    let ask = |changes: Vec<u32>| RevertRequest {
        changes: Some(changes),
        ..all()
    };
    for bad in [vec![9], (1..=501).collect::<Vec<u32>>()] {
        assert!(matches!(
            svc.revert(&s.admin, set, ask(bad)).await,
            Err(RevertError::Core(CoreError::InvalidField { .. }))
        ));
    }
    // Duplicates fold.
    let r = svc.revert(&s.admin, set, ask(vec![1, 1])).await.unwrap();
    assert_eq!(r.reverted, [1]);
    // Already reverted: skipped.
    let r = svc.revert(&s.admin, set, ask(vec![1])).await.unwrap();
    assert_eq!((r.reverted.len(), r.skipped.clone()), (0, vec![1]));
    assert_eq!(
        s.content.get(item.id).await.unwrap().status,
        ContentStatus::Trash
    );
}

fn taxonomy(s: &Site) -> ferrite_core::TaxonomyService {
    ferrite_core::TaxonomyService::new(s.store.clone())
}

async fn revert_term_delete_reinserts_with_links(db: &TestDb) {
    let s = site(db).await;
    let tax = taxonomy(&s);
    let term = tax
        .create_term(&s.admin, "category", "News", None, None, "About news")
        .await
        .unwrap();
    let mut d = draft("post", "Linked");
    d.terms = vec![term.id];
    let item = s.content.create(&s.admin, d).await.unwrap();
    tax.delete_term(&s.agent, term.id).await.unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let back = s.store.read().terms().get(term.id).await.unwrap().unwrap();
    assert_eq!(
        (back.name.as_str(), back.slug.as_str()),
        ("News", term.slug.as_str())
    );
    let linked = s.store.read().terms().for_item(item.id).await.unwrap();
    assert_eq!(linked.iter().map(|t| t.id).collect::<Vec<_>>(), [term.id]);
}

async fn revert_term_create_refused_with_children(db: &TestDb) {
    let s = site(db).await;
    let tax = taxonomy(&s);
    let parent = tax
        .create_term(&s.agent, "category", "Parent", None, None, "")
        .await
        .unwrap();
    // A person adds a child under the assistant's term.
    tax.create_term(&s.admin, "category", "Child", None, Some(parent.id), "")
        .await
        .unwrap();
    let set = only_set(&s).await;
    match service(&s).revert(&s.admin, set, all()).await {
        Err(RevertError::Failed {
            seq: 1,
            error: CoreError::Invalid(m),
        }) => assert!(m.contains("children"), "{m}"),
        other => panic!("{other:?}"),
    }
}

fn menu(items: &[&str]) -> ferrite_domain::Menu {
    ferrite_domain::Menu {
        name: "main".into(),
        label: "Main".into(),
        items: items
            .iter()
            .enumerate()
            .map(|(i, label)| ferrite_domain::MenuItem {
                id: ferrite_domain::MenuItemId::new(),
                parent_id: None,
                position: i as i32,
                kind: ferrite_domain::MenuItemKind::Url,
                target_id: None,
                url: Some(format!("https://example.com/{label}")),
                label: (*label).into(),
            })
            .collect(),
        locale: "en".into(),
        translation_of: None,
    }
}

async fn revert_menu_put_restores_tree_and_new_menu_is_deleted(db: &TestDb) {
    let s = site(db).await;
    let menus = ferrite_core::MenuService::new(s.store.clone());
    menus.put(&s.admin, &menu(&["a", "b"])).await.unwrap();
    menus.put(&s.agent, &menu(&["c"])).await.unwrap();
    let mut other = menu(&["x"]);
    other.name = "footer".into();
    menus.put(&s.agent, &other).await.unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let main = s.store.read().menus().get("main").await.unwrap().unwrap();
    let labels: Vec<&str> = main.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["a", "b"]);
    assert!(
        s.store
            .read()
            .menus()
            .get("footer")
            .await
            .unwrap()
            .is_none()
    );
}

async fn revert_type_create_refused_while_it_has_items(db: &TestDb) {
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
    s.content
        .create(&s.admin, draft("book", "A book"))
        .await
        .unwrap();
    let set = only_set(&s).await;
    match service(&s).revert(&s.admin, set, all()).await {
        Err(RevertError::Failed {
            error: CoreError::Invalid(m),
            ..
        }) => assert!(m.contains("has content"), "{m}"),
        other => panic!("{other:?}"),
    }
    // A type update reverts to the stored label.
    let s2 = site(db).await;
    types
        .update(
            &s2.agent,
            "book",
            ferrite_core::TypePatch {
                label: Some("Library".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let sets = service(&s2)
        .list(&s2.admin, &ChangeSetFilter::default())
        .await
        .unwrap()
        .items;
    service(&s2)
        .revert(&s2.admin, sets[0].id, all())
        .await
        .unwrap();
    let ty = s.store.read().types().get("book").await.unwrap().unwrap();
    assert_eq!(ty.label, "Books");
}

async fn revert_redirect_update_restores_source_and_origin(db: &TestDb) {
    let s = site(db).await;
    let r = ferrite_core::RedirectService::new(s.store.clone());
    let made = r.upsert(&s.admin, "/old", "/new", 301).await.unwrap();
    r.update(&s.agent, made.id, "/elsewhere", "/target", 302)
        .await
        .unwrap();
    let created = r.upsert(&s.agent, "/fresh", "/x", 301).await.unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let back = s
        .store
        .read()
        .redirects()
        .get(made.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (back.source_path.as_str(), back.target.as_str(), back.code),
        ("/old", "/new", 301)
    );
    assert_eq!(back.origin, made.origin);
    assert!(
        s.store
            .read()
            .redirects()
            .get(created.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.store
            .read()
            .redirects()
            .get_by_source("/elsewhere")
            .await
            .unwrap()
            .is_none()
    );
}

async fn revert_settings_in_one_set_many_call(db: &TestDb) {
    let s = site(db).await;
    let options = ferrite_core::OptionService::new(s.store.clone());
    options
        .set_many(&s.admin, &[("site.title".to_owned(), Some(json!("Mine")))])
        .await
        .unwrap();
    options
        .set_many(
            &s.agent,
            &[
                ("site.title".to_owned(), Some(json!("AI title"))),
                ("site.description".to_owned(), Some(json!("AI description"))),
            ],
        )
        .await
        .unwrap();
    let set = only_set(&s).await;
    let report = service(&s).revert(&s.admin, set, all()).await.unwrap();
    assert_eq!(report.reverted, [2, 1]);
    let get = |k: &'static str| {
        let store = s.store.clone();
        async move {
            store
                .read()
                .options()
                .get(k)
                .await
                .unwrap()
                .map(|e| e.value)
        }
    };
    assert_eq!(get("site.title").await, Some(json!("Mine")));
    assert_eq!(
        get("site.description").await,
        None,
        "absent before: back to the default"
    );
}

async fn revert_permalink_change_takes_types_lock_and_enqueues_redirects(db: &TestDb) {
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
    ferrite_core::OptionService::new(s.store.clone())
        .set_many(
            &s.agent,
            &[("permalink.post".to_owned(), Some(json!("/blog/%slug%/")))],
        )
        .await
        .unwrap();
    let jobs_before = s
        .store
        .read()
        .jobs()
        .list(&ferrite_domain::JobQuery {
            kind: Some("permalink.redirects".into()),
            limit: 200,
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    assert!(
        s.store
            .read()
            .options()
            .get("permalink.post")
            .await
            .unwrap()
            .is_none()
    );
    let jobs_after = s
        .store
        .read()
        .jobs()
        .list(&ferrite_domain::JobQuery {
            kind: Some("permalink.redirects".into()),
            limit: 200,
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len();
    assert!(jobs_after > jobs_before, "{jobs_before} -> {jobs_after}");
}

fn plugin_artifact() -> ferrite_domain::Artifact {
    ferrite_domain::Artifact {
        hash: ferrite_domain::ArtifactHash::parse(&"0b".repeat(32)).unwrap(),
        kind: ferrite_domain::ArtifactKind::Plugin,
        bytes: 42,
        mime: "application/wasm".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    }
}

async fn installed_plugin(s: &Site, key: &SiteKey) -> ferrite_core::ExtensionAdmin {
    let svc = ferrite_core::ExtensionAdmin::new(s.store.clone(), key.clone());
    let a = plugin_artifact();
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
    svc
}

fn patch(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

async fn revert_plugin_settings_restores_sealed_secret(db: &TestDb) {
    let s = site(db).await;
    let key = SiteKey::generate().unwrap();
    let svc = installed_plugin(&s, &key).await;
    svc.set_plugin_settings(&s.admin, "seo", patch(json!({"api_key": "sk-person"})))
        .await
        .unwrap();
    let sealed = s
        .store
        .read()
        .options()
        .get("plugin.seo.settings.api_key")
        .await
        .unwrap()
        .unwrap()
        .value;
    svc.set_plugin_settings(
        &s.agent,
        "seo",
        patch(json!({"api_key": "sk-ai", "limit": 3})),
    )
    .await
    .unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let back = s
        .store
        .read()
        .options()
        .get("plugin.seo.settings.api_key")
        .await
        .unwrap()
        .unwrap()
        .value;
    assert_eq!(back, sealed, "the person's secret, still sealed");
    assert!(
        s.store
            .read()
            .options()
            .get("plugin.seo.settings.limit")
            .await
            .unwrap()
            .is_none()
    );
    let view = svc.plugin_settings(&s.admin, "seo").await.unwrap();
    assert!(view.secrets["api_key"], "the plugin still opens it");
    // The change detail shows the secret as set, never sealed or plain.
    let detail = service(&s).change(&s.admin, set, 1).await.unwrap();
    let text = format!("{:?}", detail.images);
    assert!(!text.contains("sealed") && !text.contains("sk-"), "{text}");
}

async fn revert_theme_activation_requires_same_artifact(db: &TestDb) {
    let s = site(db).await;
    let svc = ferrite_core::ExtensionAdmin::new(s.store.clone(), SiteKey::generate().unwrap());
    let art = |seed: u8| ferrite_domain::Artifact {
        hash: ferrite_domain::ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind: ferrite_domain::ArtifactKind::Theme,
        bytes: 42,
        mime: "application/octet-stream".into(),
        created_at: Timestamp::now(),
        orphaned_at: None,
    };
    let theme = |name: &str, a: &ferrite_domain::Artifact| ferrite_domain::InstalledTheme {
        name: name.into(),
        version: "1.0.0".into(),
        artifact_hash: a.hash.clone(),
        manifest: json!({"name": name}),
        active: false,
        installed_at: Timestamp::now(),
    };
    let (light, dark, light2) = (art(1), art(2), art(3));
    svc.install_theme(&s.admin, &light, &theme("light", &light), true)
        .await
        .unwrap();
    svc.install_theme(&s.admin, &dark, &theme("dark", &dark), false)
        .await
        .unwrap();
    svc.activate_theme(&s.agent, "dark", &dark.hash)
        .await
        .unwrap();
    let set = only_set(&s).await;
    // A person replaced `light` with another bundle since: refused.
    svc.install_theme(&s.admin, &light2, &theme("light", &light2), false)
        .await
        .unwrap();
    assert!(matches!(
        service(&s).revert(&s.admin, set, all()).await,
        Err(RevertError::Failed {
            error: CoreError::Invalid(_),
            ..
        })
    ));
    svc.install_theme(&s.admin, &light, &theme("light", &light), false)
        .await
        .unwrap();
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let active = s.store.read().themes().active().await.unwrap().unwrap();
    assert_eq!(active.name, "light");
}

async fn revert_plugin_disable_enables(db: &TestDb) {
    let s = site(db).await;
    let key = SiteKey::generate().unwrap();
    let svc = installed_plugin(&s, &key).await;
    svc.set_plugin_enabled(&s.admin, "seo", true).await.unwrap();
    svc.set_plugin_enabled(&s.agent, "seo", false)
        .await
        .unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let row = s.store.read().plugins().get("seo").await.unwrap().unwrap();
    assert!(row.enabled);
}

/// Review fix 1: a term the assistant created and then linked to a post changes the
/// term's links without a term change; the links stay out of the hash, so no false
/// `changed_since`.
async fn assistant_term_then_tagging_reverts_without_force(db: &TestDb) {
    let s = site(db).await;
    let tax = taxonomy(&s);
    let item = s
        .content
        .create(&s.admin, draft("post", "Filed"))
        .await
        .unwrap();
    let term = tax
        .create_term(&s.agent, "category", "AI", None, None, "")
        .await
        .unwrap();
    let mut d = draft("post", "Filed");
    d.slug = Some(item.slug.clone());
    d.terms = vec![term.id];
    s.content
        .update(&s.agent, item.id, item.version, d)
        .await
        .unwrap();
    let set = only_set(&s).await;
    let report = service(&s).revert(&s.admin, set, all()).await.unwrap();
    assert_eq!(report.reverted, [2, 1]);
    assert!(report.forced.is_empty());
    assert!(s.store.read().terms().get(term.id).await.unwrap().is_none());
    assert!(
        s.store
            .read()
            .terms()
            .for_item(item.id)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Review fix 2: a reverse status change emits the public transition's webhook event.
async fn revert_publish_emits_unpublished_webhook(db: &TestDb) {
    let s = site(db).await;
    let hooks = ferrite_core::webhooks::WebhookService::new(
        s.store.clone(),
        SiteKey::from_hex(&"ab".repeat(32)).unwrap(),
        ferrite_core::webhooks::UrlPolicy {
            local_targets: false,
            allow_ip: |_| true,
        },
    );
    hooks
        .create(
            &s.admin,
            ferrite_core::webhooks::WebhookInput {
                name: "content".into(),
                url: "https://hooks.example.com/x".into(),
                events: vec!["content.unpublished".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let item = s
        .content
        .create(&s.admin, draft("post", "Live"))
        .await
        .unwrap();
    s.content
        .publish(&s.agent, item.id, item.version)
        .await
        .unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let w = s.store.read().webhooks();
    let endpoint = &w.list_endpoints().await.unwrap()[0];
    let page = w
        .list_deliveries(endpoint.id, &Default::default())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].event_type, "content.unpublished");
}

/// Review fix 3: a reschedule (unpublish, schedule at another time) reverts to the
/// original time, though the status is the same.
async fn revert_reschedule_restores_scheduled_time(db: &TestDb) {
    let s = site(db).await;
    let hour = 3_600_000_000;
    let t0 = Timestamp::from_micros(Timestamp::now().as_micros() + hour).unwrap();
    let t1 = Timestamp::from_micros(Timestamp::now().as_micros() + 2 * hour).unwrap();
    let item = s
        .content
        .create(&s.admin, draft("post", "Later"))
        .await
        .unwrap();
    let item = s
        .content
        .schedule(&s.admin, item.id, item.version, t0)
        .await
        .unwrap();
    let item = s
        .content
        .unpublish(&s.agent, item.id, item.version)
        .await
        .unwrap();
    s.content
        .schedule(&s.agent, item.id, item.version, t1)
        .await
        .unwrap();
    let set = only_set(&s).await;
    service(&s).revert(&s.admin, set, all()).await.unwrap();
    let now = s.content.get(item.id).await.unwrap();
    assert_eq!(now.status, ContentStatus::Scheduled);
    assert_eq!(now.scheduled_at, Some(t0));
}
