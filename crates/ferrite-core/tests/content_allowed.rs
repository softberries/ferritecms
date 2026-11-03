//! `allowed_actions` against the real lifecycle, and `counts` / `authors`, on both
//! backends (M2b P13, P14; Review Focus 4).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use ferrite_core::{
    Actor, ContentDraft, ContentService, CoreError, PermalinkRules,
    content::{AllowedActions, AuthorRef, ContentRef, StatusCounts, allowed_actions},
};
use ferrite_domain::{
    ContentId, ContentItem, ContentStatus, NewUser, Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

const STATUSES: [ContentStatus; 6] = [
    ContentStatus::Draft,
    ContentStatus::Pending,
    ContentStatus::Scheduled,
    ContentStatus::Published,
    ContentStatus::Private,
    ContentStatus::Trash,
];
const ROLES: [Role; 4] = [Role::Admin, Role::Editor, Role::Author, Role::Contributor];
const HOUR: i64 = 3_600_000_000;

static TITLES: AtomicUsize = AtomicUsize::new(0);

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    store
        .read()
        .users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: format!("Name {id}"),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    Actor::new(id, role)
}

fn service(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
}

fn draft(title: &str) -> ContentDraft {
    ContentDraft {
        kind: "post".into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields: json!({}),
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

fn at(offset: i64) -> Timestamp {
    Timestamp::from_micros(Timestamp::now().as_micros() + offset).unwrap()
}

/// A fresh item by `owner` in `status`. Statuses no transition reaches cheaply (private
/// above all, pre-flight F7) are written through the store.
async fn item(
    db: &TestDb,
    svc: &ContentService,
    owner: &Actor,
    status: ContentStatus,
) -> ContentItem {
    let n = TITLES.fetch_add(1, Ordering::Relaxed);
    let created = svc
        .create(owner, draft(&format!("Item {n}")))
        .await
        .unwrap();
    if status == ContentStatus::Draft {
        return created;
    }
    let mut next = created;
    next.status = status;
    match status {
        ContentStatus::Scheduled => next.scheduled_at = Some(at(HOUR)),
        ContentStatus::Published => next.published_at = Some(at(-HOUR)),
        _ => {}
    }
    db.store().read().content().update(&next).await.unwrap()
}

/// The expected table: 07 §1 "Roles & capabilities" and M1c-1 R8 (contributors get
/// nothing on a trashed item, not even their own).
fn expected(role: Role, own: bool, status: ContentStatus) -> AllowedActions {
    use ContentStatus::*;
    let full = matches!(role, Role::Admin | Role::Editor) || (role == Role::Author && own);
    if full {
        return AllowedActions {
            edit: status != Trash,
            publish: matches!(status, Draft | Pending | Scheduled),
            unpublish: matches!(status, Published | Scheduled | Private),
            schedule: matches!(status, Draft | Pending),
            submit: status == Draft,
            trash: status != Trash,
            restore: status == Trash,
            delete: status == Trash,
            trash_undoable: status != Trash,
            ..AllowedActions::default()
        };
    }
    if role == Role::Contributor && own {
        let open = matches!(status, Draft | Pending);
        return AllowedActions {
            edit: open,
            submit: status == Draft,
            trash: open,
            ..AllowedActions::default()
        };
    }
    AllowedActions::default()
}

/// Runs action `name` on `item` through the real service method.
async fn run(
    svc: &ContentService,
    actor: &Actor,
    item: &ContentItem,
    name: &str,
) -> Result<(), CoreError> {
    let (id, v) = (item.id, item.version);
    match name {
        "edit" => {
            let mut d = draft(&format!("{} edited", item.title));
            d.slug = Some(item.slug.clone());
            svc.update(actor, id, v, d).await.map(drop)
        }
        "publish" => svc.publish(actor, id, v).await.map(drop),
        "unpublish" => svc.unpublish(actor, id, v).await.map(drop),
        "schedule" => svc.schedule(actor, id, v, at(HOUR)).await.map(drop),
        "submit" => svc.submit(actor, id, v).await.map(drop),
        "trash" => svc.trash(actor, id, v).await.map(drop),
        "restore" => svc.restore(actor, id, v).await.map(drop),
        "delete" => svc.delete(actor, id, v).await,
        // Trash, then restore with the trashed version (the admin's Undo).
        "trash_undoable" => {
            let trashed = svc.trash(actor, id, v).await?;
            svc.restore(actor, id, trashed.version).await.map(drop)
        }
        other => panic!("unknown action {other}"),
    }
}

fn flags(a: AllowedActions) -> [(&'static str, bool); 9] {
    [
        ("edit", a.edit),
        ("publish", a.publish),
        ("unpublish", a.unpublish),
        ("schedule", a.schedule),
        ("submit", a.submit),
        ("trash", a.trash),
        ("restore", a.restore),
        ("delete", a.delete),
        ("trash_undoable", a.trash_undoable),
    ]
}

pub async fn allowed_actions_matrix(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let other = actor(store, Role::Author).await;
    for role in ROLES {
        let me = actor(store, role).await;
        for own in [true, false] {
            let owner = if own { &me } else { &other };
            for status in STATUSES {
                let cell = format!("{role:?} own={own} {status:?}");
                let probe = item(db, &svc, owner, status).await;
                let got = allowed_actions(&me, &probe, &Default::default());
                assert_eq!(got, expected(role, own, status), "{cell}");
                for (name, allowed) in flags(got) {
                    // A fresh item per action: each run changes it.
                    let target = item(db, &svc, owner, status).await;
                    let result = run(&svc, &me, &target, name).await;
                    if allowed {
                        assert!(result.is_ok(), "{cell} {name}: {result:?}");
                    } else {
                        assert!(
                            matches!(result, Err(CoreError::Forbidden(_) | CoreError::Invalid(_))),
                            "{cell} {name}: {result:?}"
                        );
                    }
                }
            }
        }
    }
}

pub async fn allowed_actions_follow_token_scopes(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let reader =
        admin.with_scopes(ferrite_core::Scopes::from_strs(&["content:read".to_owned()]).unwrap());
    let it = item(db, &svc, &admin, ContentStatus::Draft).await;
    assert_eq!(allowed_actions(&reader, &it, &Default::default()), AllowedActions::default());
    assert!(matches!(
        svc.publish(&reader, it.id, it.version).await,
        Err(CoreError::Forbidden(_))
    ));
}

pub async fn counts_respect_can_read(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let author = actor(store, Role::Author).await;
    let other = actor(store, Role::Author).await;
    let editor = actor(store, Role::Editor).await;
    let contributor = actor(store, Role::Contributor).await;
    for _ in 0..2 {
        item(db, &svc, &author, ContentStatus::Draft).await;
    }
    let gone = item(db, &svc, &author, ContentStatus::Draft).await;
    svc.trash(&author, gone.id, gone.version).await.unwrap();
    for _ in 0..3 {
        item(db, &svc, &other, ContentStatus::Draft).await;
    }
    // Others' public items count for everyone; others' private ones do not.
    item(db, &svc, &other, ContentStatus::Published).await;
    item(db, &svc, &other, ContentStatus::Private).await;
    item(db, &svc, &other, ContentStatus::Pending).await;
    item(db, &svc, &author, ContentStatus::Scheduled).await;
    // Another author's published item dated in the future is not public yet (R23).
    let mut future = item(db, &svc, &other, ContentStatus::Published).await;
    future.published_at = Some(at(HOUR));
    store.read().content().update(&future).await.unwrap();

    assert_eq!(
        svc.counts(&author, "post", None).await.unwrap(),
        StatusCounts {
            draft: 2,
            pending: 0,
            scheduled: 1,
            published: 1,
            private: 0,
            trash: 1,
        }
    );
    assert_eq!(
        svc.counts(&editor, "post", None).await.unwrap(),
        StatusCounts {
            draft: 5,
            pending: 1,
            scheduled: 1,
            published: 2,
            private: 1,
            trash: 1,
        }
    );
    // A contributor with nothing of its own sees only the publicly visible item.
    assert_eq!(
        svc.counts(&contributor, "post", None).await.unwrap(),
        StatusCounts {
            published: 1,
            ..StatusCounts::default()
        }
    );
    assert_eq!(
        svc.counts(&editor, "page", None).await.unwrap(),
        StatusCounts::default()
    );
    assert!(matches!(
        svc.counts(&editor, "nope", None).await,
        Err(CoreError::NotFound)
    ));
    let no_scope =
        editor.with_scopes(ferrite_core::Scopes::from_strs(&["users:read".to_owned()]).unwrap());
    assert!(matches!(
        svc.counts(&no_scope, "post", None).await,
        Err(CoreError::Forbidden(_))
    ));
}

pub async fn authors_resolve_logins(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let a = actor(store, Role::Author).await;
    let b = actor(store, Role::Editor).await;
    // More than one `get_many` chunk (250 unknown ids), with repeats.
    let mut ids = vec![a.user, b.user, a.user];
    ids.extend((0..250).map(|_| UserId::new()));
    ids.push(b.user);
    let map = svc.authors(&a, &ids).await.unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(
        map[&a.user],
        AuthorRef {
            id: a.user,
            login: format!("user-{}", a.user),
            display_name: format!("Name {}", a.user),
        }
    );
    assert_eq!(map[&b.user].login, format!("user-{}", b.user));
    assert!(svc.authors(&a, &[]).await.unwrap().is_empty());
}

pub async fn refs_respect_can_read(db: &TestDb) {
    let svc = service(db);
    let store = db.store();
    let author = actor(store, Role::Author).await;
    let other = actor(store, Role::Author).await;
    let editor = actor(store, Role::Editor).await;
    let own_draft = item(db, &svc, &author, ContentStatus::Draft).await;
    let others_draft = item(db, &svc, &other, ContentStatus::Draft).await;
    let others_public = item(db, &svc, &other, ContentStatus::Published).await;
    let media = svc
        .create(
            &other,
            ContentDraft {
                kind: "media".into(),
                ..draft("A picture")
            },
        )
        .await
        .unwrap();
    let mut media = media;
    media.status = ContentStatus::Published;
    media.published_at = Some(at(-HOUR));
    let media = store.read().content().update(&media).await.unwrap();
    let missing = ContentId::new();
    let ids = [
        own_draft.id,
        others_draft.id,
        others_public.id,
        media.id,
        missing,
        own_draft.id,
    ];

    let refs = |list: Vec<ContentRef>| {
        let mut ids: Vec<ContentId> = list.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids
    };
    let sorted = |mut v: Vec<ContentId>| {
        v.sort_unstable();
        v
    };

    // An author gets its own draft and others' public items, never others' drafts.
    let got = svc.refs(&author, &ids).await.unwrap();
    assert_eq!(
        refs(got.clone()),
        sorted(vec![own_draft.id, others_public.id, media.id])
    );
    let public = got.iter().find(|r| r.id == others_public.id).unwrap();
    assert_eq!(public.kind, "post");
    assert_eq!(public.title, others_public.title);
    assert_eq!(public.status, ContentStatus::Published);
    assert_eq!(
        public.permalink.as_deref(),
        Some(format!("/{}/", others_public.slug).as_str())
    );
    // Not public: no permalink. Media are files, never pages.
    let own = got.iter().find(|r| r.id == own_draft.id).unwrap();
    assert_eq!(own.permalink, None);
    let pic = got.iter().find(|r| r.id == media.id).unwrap();
    assert_eq!(
        (pic.kind.as_str(), pic.permalink.as_deref()),
        ("media", None)
    );

    // An editor reads everything that exists.
    assert_eq!(
        refs(svc.refs(&editor, &ids).await.unwrap()),
        sorted(vec![
            own_draft.id,
            others_draft.id,
            others_public.id,
            media.id
        ])
    );
    // Media need `media:read` too.
    let content_only =
        editor.with_scopes(ferrite_core::Scopes::from_strs(&["content:read".to_owned()]).unwrap());
    assert_eq!(
        refs(svc.refs(&content_only, &ids).await.unwrap()),
        sorted(vec![own_draft.id, others_draft.id, others_public.id])
    );
    // 1..=200 ids, else `ids`.
    let many: Vec<ContentId> = (0..201).map(|_| ContentId::new()).collect();
    for bad in [&[][..], &many[..]] {
        assert!(matches!(
            svc.refs(&editor, bad).await,
            Err(CoreError::InvalidField { field: "ids", .. })
        ));
    }
    // Without `content:read`, nothing.
    let no_scope =
        editor.with_scopes(ferrite_core::Scopes::from_strs(&["users:read".to_owned()]).unwrap());
    assert!(matches!(
        svc.refs(&no_scope, &ids).await,
        Err(CoreError::Forbidden(_))
    ));
}

backend_tests!(
    allowed_actions_matrix,
    allowed_actions_follow_token_scopes,
    counts_respect_can_read,
    refs_respect_can_read,
    authors_resolve_logins,
);
