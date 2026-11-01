//! ContentService admin reads (lists with filters and search, revisions) and revision
//! restore, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashSet, sync::Arc};

use ferrite_core::{
    Actor, AdminQuery, ContentDraft, ContentService, ContentTypeService, CoreError, PermalinkRules,
    Scopes, SiteSearch,
};
use ferrite_domain::{
    ContentId, ContentOrder, ContentStatus, NewUser, ParentFilter, Role, Term, TermId, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
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
    tx.commit().await.unwrap();
    Actor::new(id, role)
}

fn service(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
}

fn draft(kind: &str, title: &str, text: &str) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": text}]}
        ]}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

async fn term(store: &dyn Store, slug: &str) -> TermId {
    let id = TermId::new();
    store
        .read()
        .terms()
        .insert(&Term {
            id,
            taxonomy: "category".into(),
            slug: slug.into(),
            name: slug.into(),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();
    id
}

fn query(kind: &str) -> AdminQuery {
    AdminQuery::new(kind)
}

fn ids(items: &[ferrite_domain::ContentItem]) -> HashSet<ContentId> {
    items.iter().map(|i| i.id).collect()
}

pub async fn admin_list_filters_and_search(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let contributor = actor(store, Role::Contributor).await;
    let cat = term(store, "news").await;

    let drafted = svc
        .create(&admin, draft("post", "Drafted", "a quokka sighting"))
        .await
        .unwrap();
    let mut with_term = draft("post", "Termed", "plain words");
    with_term.terms = vec![cat];
    let termed = svc.create(&admin, with_term).await.unwrap();
    let published = svc
        .create(&admin, draft("post", "Published", "plain words"))
        .await
        .unwrap();
    let published = svc
        .publish(&admin, published.id, published.version)
        .await
        .unwrap();
    let trashed = svc
        .create(&admin, draft("post", "Trashed", "a quokka in the bin"))
        .await
        .unwrap();
    svc.trash(&admin, trashed.id, trashed.version)
        .await
        .unwrap();
    let by_contributor = svc
        .create(&contributor, draft("post", "Mine", "plain words"))
        .await
        .unwrap();
    let page = svc
        .create(&admin, draft("page", "A page", "a quokka page"))
        .await
        .unwrap();

    // Empty statuses: every status but trash; only the queried type.
    let all = svc.list(&admin, &query("post")).await.unwrap();
    assert_eq!(
        ids(&all.items),
        HashSet::from([drafted.id, termed.id, published.id, by_contributor.id])
    );
    assert!(all.next_cursor.is_none());
    // Default order: newest update first.
    assert_eq!(all.items[0].id, by_contributor.id);

    // Trash only when asked for.
    let mut q = query("post");
    q.statuses = vec![ContentStatus::Trash];
    let bin = svc.list(&admin, &q).await.unwrap();
    assert_eq!(ids(&bin.items), HashSet::from([trashed.id]));

    // Status, author and term filters.
    let mut q = query("post");
    q.statuses = vec![ContentStatus::Published];
    assert_eq!(
        ids(&svc.list(&admin, &q).await.unwrap().items),
        HashSet::from([published.id])
    );
    let mut q = query("post");
    q.author = Some(contributor.user);
    assert_eq!(
        ids(&svc.list(&admin, &q).await.unwrap().items),
        HashSet::from([by_contributor.id])
    );
    let mut q = query("post");
    q.term = Some(cat);
    assert_eq!(
        ids(&svc.list(&admin, &q).await.unwrap().items),
        HashSet::from([termed.id])
    );
    let mut q = query("page");
    q.parent = ParentFilter::Root;
    assert_eq!(
        ids(&svc.list(&admin, &q).await.unwrap().items),
        HashSet::from([page.id])
    );

    // `q` finds a draft by its body text; trash and other types stay out.
    let mut q = query("post");
    q.q = Some("quokka".into());
    let hits = svc.list(&admin, &q).await.unwrap();
    assert_eq!(ids(&hits.items), HashSet::from([drafted.id]));
    // A blank `q` is no search.
    let mut q = query("post");
    q.q = Some("  ".into());
    assert_eq!(svc.list(&admin, &q).await.unwrap().items.len(), 4);
    // `q` with author, term or parent is refused (F19).
    for edit in [
        (|q: &mut AdminQuery| q.author = Some(UserId::new())) as fn(&mut AdminQuery),
        |q| q.term = Some(TermId::new()),
        |q| q.parent = ParentFilter::Root,
    ] {
        let mut q = query("post");
        q.q = Some("quokka".into());
        edit(&mut q);
        assert!(matches!(
            svc.list(&admin, &q).await,
            Err(CoreError::Invalid(_))
        ));
    }

    // Cursor pages are disjoint and cover everything, in every order and in search.
    for order in [
        ContentOrder::UpdatedDesc,
        ContentOrder::PublishedDesc,
        ContentOrder::TitleAsc,
        ContentOrder::MenuOrderAsc,
    ] {
        let mut q = query("post");
        q.order = order;
        q.limit = 3;
        let first = svc.list(&admin, &q).await.unwrap();
        assert_eq!(first.items.len(), 3, "{order:?}");
        q.cursor = first.next_cursor.clone();
        assert!(q.cursor.is_some(), "{order:?}");
        let second = svc.list(&admin, &q).await.unwrap();
        assert_eq!(second.items.len(), 1, "{order:?}");
        assert!(second.next_cursor.is_none(), "{order:?}");
        let (a, b) = (ids(&first.items), ids(&second.items));
        assert!(a.is_disjoint(&b), "{order:?}");
        assert_eq!(a.len() + b.len(), 4, "{order:?}");
    }
    let mut q = query("post");
    q.q = Some("words".into());
    q.limit = 2;
    let first = svc.list(&admin, &q).await.unwrap();
    assert_eq!(first.items.len(), 2);
    q.cursor = first.next_cursor.clone();
    let second = svc.list(&admin, &q).await.unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(ids(&first.items).is_disjoint(&ids(&second.items)));
    // A cursor of another kind of list is Invalid.
    let mut q = query("post");
    q.cursor = first.next_cursor;
    assert!(matches!(
        svc.list(&admin, &q).await,
        Err(CoreError::Invalid(_))
    ));

    // A contributor lists published items and its own, nothing else (R17), in every
    // mode: plain, status filter, author filter, search; pages stay exact.
    assert_eq!(
        ids(&svc.list(&contributor, &query("post")).await.unwrap().items),
        HashSet::from([published.id, by_contributor.id])
    );
    let mut q = query("post");
    q.statuses = vec![ContentStatus::Draft, ContentStatus::Trash];
    assert_eq!(
        ids(&svc.list(&contributor, &q).await.unwrap().items),
        HashSet::from([by_contributor.id])
    );
    let mut q = query("post");
    q.author = Some(admin.user);
    assert_eq!(
        ids(&svc.list(&contributor, &q).await.unwrap().items),
        HashSet::from([published.id])
    );
    let mut q = query("post");
    q.q = Some("quokka".into());
    assert!(svc.list(&contributor, &q).await.unwrap().items.is_empty());
    let mut q = query("post");
    q.limit = 1;
    let first = svc.list(&contributor, &q).await.unwrap();
    assert_eq!(first.items.len(), 1);
    q.cursor = first.next_cursor;
    let second = svc.list(&contributor, &q).await.unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_cursor.is_none());
    // Single reads, terms and revisions of others' unpublished items: NotFound.
    for id in [drafted.id, termed.id, trashed.id] {
        assert!(matches!(
            svc.get_for(&contributor, id).await,
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            svc.terms_of(&contributor, id).await,
            Err(CoreError::NotFound)
        ));
        assert!(matches!(
            svc.revisions(&contributor, id, 50).await,
            Err(CoreError::NotFound)
        ));
    }
    let rid = svc.revisions(&admin, drafted.id, 1).await.unwrap()[0].id;
    assert!(matches!(
        svc.revision(&contributor, drafted.id, rid).await,
        Err(CoreError::NotFound)
    ));
    assert_eq!(
        svc.get_for(&contributor, published.id).await.unwrap(),
        published
    );
    assert_eq!(
        svc.get_for(&contributor, by_contributor.id).await.unwrap(),
        by_contributor
    );
    // An author likewise; an editor reads everything.
    let author = actor(store, Role::Author).await;
    assert_eq!(
        svc.list(&author, &query("post")).await.unwrap().items.len(),
        1
    );
    let editor = actor(store, Role::Editor).await;
    assert_eq!(
        svc.list(&editor, &query("post")).await.unwrap().items.len(),
        4
    );
    assert_eq!(svc.get_for(&editor, drafted.id).await.unwrap(), drafted);
    // The role decides, not a write scope: an admin's read token reads others' drafts.
    let reader = admin.with_scopes(Scopes::from_strs(&["content:read".into()]).unwrap());
    assert_eq!(
        svc.get_for(&reader, by_contributor.id).await.unwrap(),
        by_contributor
    );
    // A token without a content scope may not.
    let narrow = admin.with_scopes(Scopes::from_strs(&["users:read".into()]).unwrap());
    assert!(matches!(
        svc.list(&narrow, &query("post")).await,
        Err(CoreError::Forbidden("content:read"))
    ));
    assert!(matches!(
        svc.get_for(&narrow, drafted.id).await,
        Err(CoreError::Forbidden("content:read"))
    ));
    let reader = admin.with_scopes(Scopes::from_strs(&["content:read".into()]).unwrap());
    assert_eq!(svc.get_for(&reader, drafted.id).await.unwrap(), drafted);
    assert_eq!(svc.terms_of(&reader, termed.id).await.unwrap(), [cat]);
    assert!(matches!(
        svc.get_for(&reader, ContentId::new()).await,
        Err(CoreError::NotFound)
    ));

    // The type check (now `ContentTypeService::get`): known types pass, unknown ones
    // are NotFound.
    let types = ContentTypeService::new(db.shared_store());
    types.get(&reader, "post").await.unwrap();
    assert!(matches!(
        types.get(&reader, "nope").await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        types.get(&narrow, "post").await,
        Err(CoreError::Forbidden("content:read"))
    ));
}

pub async fn revisions_list_get_restore(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let admin = actor(store, Role::Admin).await;
    let cat = term(store, "kept").await;

    let mut d = draft("post", "First title", "first body");
    d.fields = json!({"featured_image": null});
    let v1 = svc.create(&admin, d).await.unwrap();
    let mut d = draft("post", "Second title", "second body");
    d.slug = Some("second-slug".into());
    d.terms = vec![cat];
    d.menu_order = 7;
    d.fields = json!({"x": 1});
    let v2 = svc.update(&admin, v1.id, v1.version, d).await.unwrap();
    store
        .read()
        .meta()
        .set(v1.id, "color", &json!("blue"))
        .await
        .unwrap();

    // Newest first, capped by the limit.
    let revs = svc.revisions(&admin, v1.id, 50).await.unwrap();
    assert_eq!(revs.len(), 2);
    assert_eq!(revs[0].title, "Second title");
    assert_eq!(revs[1].title, "First title");
    assert!(revs.iter().all(|r| r.note.is_none()));
    assert_eq!(svc.revisions(&admin, v1.id, 1).await.unwrap().len(), 1);
    let oldest = revs[1].clone();
    assert_eq!(
        svc.revision(&admin, v1.id, oldest.id).await.unwrap(),
        oldest
    );
    assert!(matches!(
        svc.revisions(&admin, ContentId::new(), 50).await,
        Err(CoreError::NotFound)
    ));

    // Restore: title, excerpt, body and fields come back; slug, terms, menu order and
    // meta stay as they are now; a new revision notes it; version +1.
    let restored = svc
        .restore_revision(&admin, v1.id, oldest.id, v2.version)
        .await
        .unwrap();
    assert_eq!(restored.version, v2.version + 1);
    assert_eq!(restored.title, "First title");
    assert_eq!(restored.body, v1.body);
    assert_eq!(restored.excerpt, v1.excerpt);
    assert_eq!(restored.fields, json!({"featured_image": null}));
    assert_eq!(restored.slug, "second-slug");
    assert_eq!(restored.menu_order, 7);
    assert_eq!(svc.terms_of(&admin, v1.id).await.unwrap(), [cat]);
    assert_eq!(
        store.read().meta().get(v1.id, "color").await.unwrap(),
        Some(json!("blue"))
    );
    let revs = svc.revisions(&admin, v1.id, 50).await.unwrap();
    assert_eq!(revs.len(), 3);
    assert_eq!(revs[0].title, "First title");
    assert_eq!(
        revs[0].note.as_deref(),
        Some(format!("restored from {}", oldest.id).as_str())
    );

    // Another item's revision is NotFound, as get and as restore; nothing written.
    let other = svc
        .create(&admin, draft("post", "Other", "other body"))
        .await
        .unwrap();
    let foreign = svc.revisions(&admin, other.id, 1).await.unwrap()[0].id;
    assert!(matches!(
        svc.revision(&admin, v1.id, foreign).await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        svc.restore_revision(&admin, v1.id, foreign, restored.version)
            .await,
        Err(CoreError::NotFound)
    ));
    // A stale version is a Conflict.
    assert!(matches!(
        svc.restore_revision(&admin, v1.id, oldest.id, v2.version).await,
        Err(CoreError::Conflict { current_version }) if current_version == restored.version
    ));
    assert_eq!(svc.get(v1.id).await.unwrap(), restored);

    // Same permissions as update: another author cannot restore; a read token cannot.
    let author = actor(store, Role::Author).await;
    assert!(matches!(
        svc.restore_revision(&author, v1.id, oldest.id, restored.version)
            .await,
        Err(CoreError::Forbidden("edit_others"))
    ));
    let reader = admin.with_scopes(Scopes::from_strs(&["content:read".into()]).unwrap());
    assert!(matches!(
        svc.restore_revision(&reader, v1.id, oldest.id, restored.version)
            .await,
        Err(CoreError::Forbidden("content:write"))
    ));
    // End on a committed operation.
    svc.create(&admin, draft("post", "Done", "done"))
        .await
        .unwrap();
}

/// R26: revisions need edit rights on the item, not only read rights. Another author's
/// published post is readable by everyone, but its revisions (which keep removed text)
/// are `NotFound` to contributors and authors; editors and the owner read them, also
/// through a read-only token. A contributor's own published post is no longer editable
/// by it, so its revisions are hidden from it too.
pub async fn revisions_need_edit_rights(db: &TestDb) {
    let store = db.store();
    let svc = service(db);
    let editor = actor(store, Role::Editor).await;
    let post = svc
        .create(&editor, draft("post", "Live", "removed by mistake"))
        .await
        .unwrap();
    let post = svc.publish(&editor, post.id, post.version).await.unwrap();
    let rid = svc.revisions(&editor, post.id, 1).await.unwrap()[0].id;

    for role in [Role::Contributor, Role::Author] {
        let other = actor(store, role).await;
        // The item itself stays readable (R17).
        assert_eq!(svc.get_for(&other, post.id).await.unwrap(), post);
        assert!(
            matches!(
                svc.revisions(&other, post.id, 50).await,
                Err(CoreError::NotFound)
            ),
            "{role:?}"
        );
        assert!(
            matches!(
                svc.revision(&other, post.id, rid).await,
                Err(CoreError::NotFound)
            ),
            "{role:?}"
        );
    }
    let read_scope = Scopes::from_strs(&["content:read".into()]).unwrap();
    for reader in [
        editor.with_scopes(read_scope),
        actor(store, Role::Editor).await.with_scopes(read_scope),
        actor(store, Role::Admin).await,
    ] {
        assert_eq!(svc.revisions(&reader, post.id, 50).await.unwrap().len(), 1);
        assert_eq!(svc.revision(&reader, post.id, rid).await.unwrap().id, rid);
    }

    // A contributor reads the revisions of its own draft, not of its published post.
    let cora = actor(store, Role::Contributor).await;
    let own = svc
        .create(&cora, draft("post", "Mine", "mine"))
        .await
        .unwrap();
    assert_eq!(svc.revisions(&cora, own.id, 50).await.unwrap().len(), 1);
    let own = svc.publish(&editor, own.id, own.version).await.unwrap();
    assert!(matches!(
        svc.revisions(&cora, own.id, 50).await,
        Err(CoreError::NotFound)
    ));
    // End on a committed operation.
    svc.create(&editor, draft("post", "Done", "done"))
        .await
        .unwrap();
}

/// `ContentService::search` (design 12 §5.2 `site_search`): across types, never media,
/// with the list's visibility rules and its refusals.
async fn site_search_across_types(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(db);
    let admin = actor(store.as_ref(), Role::Admin).await;
    let author = actor(store.as_ref(), Role::Author).await;
    let hidden = svc
        .create(&admin, draft("post", "Quokka draft", "quokka secrets"))
        .await
        .unwrap();
    let page = svc
        .create(&admin, draft("page", "Quokka page", "quokka facts"))
        .await
        .unwrap();
    svc.publish(&admin, page.id, page.version).await.unwrap();
    let own = svc
        .create(&author, draft("post", "My quokka", "quokka notes"))
        .await
        .unwrap();
    let search = |q: &str, kinds: &[&str]| SiteSearch {
        q: q.into(),
        kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
        limit: 50,
        cursor: None,
    };
    let ids = |items: Vec<ferrite_domain::ContentItem>| -> HashSet<ContentId> {
        items.into_iter().map(|i| i.id).collect()
    };
    let all = svc.search(&admin, &search("quokka", &[])).await.unwrap();
    assert_eq!(ids(all.items), HashSet::from([hidden.id, page.id, own.id]));
    // The author sees the published page and its own draft, not the admin's draft.
    let theirs = svc.search(&author, &search("quokka", &[])).await.unwrap();
    assert_eq!(ids(theirs.items), HashSet::from([page.id, own.id]));
    let pages = svc
        .search(&admin, &search("quokka", &["page"]))
        .await
        .unwrap();
    assert_eq!(ids(pages.items), HashSet::from([page.id]));
    // Refusals.
    for (s, field) in [
        (search("  ", &[]), "q"),
        (search("quokka", &["media"]), "types"),
        (search("quokka", &["nope"]), "types"),
        (
            SiteSearch {
                limit: 0,
                ..search("quokka", &[])
            },
            "limit",
        ),
    ] {
        match svc.search(&admin, &s).await {
            Err(CoreError::InvalidField { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{s:?}: {other:?}"),
        }
    }
    let no_scope = admin.with_scopes(Scopes::from_strs(&["media:read".into()]).unwrap());
    assert!(matches!(
        svc.search(&no_scope, &search("quokka", &[])).await,
        Err(CoreError::Forbidden("content:read"))
    ));
}

backend_tests!(
    admin_list_filters_and_search,
    revisions_list_get_restore,
    revisions_need_edit_rights,
    site_search_across_types,
);
