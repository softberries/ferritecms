//! Constraint errors name the violated constraint identically on every backend (I1).

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, InstalledTheme, Media, Menu, MenuItem,
    MenuItemId, MenuItemKind, NewUser, Redirect, RedirectId, RedirectOrigin, Revision, RevisionId,
    Role, Term, TermId, Timestamp, UserId, UserStatus,
};
use ferrite_store::StoreError;
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

#[track_caller]
fn assert_unique<T: std::fmt::Debug>(result: Result<T, StoreError>, expected: &str) {
    match result {
        Err(StoreError::UniqueViolation { constraint }) => {
            assert_eq!(constraint, expected);
        }
        other => panic!("expected UniqueViolation {{ {expected} }}, got {other:?}"),
    }
}

fn new_user(login: &str, email: &str) -> NewUser {
    NewUser {
        id: UserId::new(),
        login: login.into(),
        email: email.into(),
        display_name: "U".into(),
        password_hash: None,
        role: Role::Author,
        status: UserStatus::Active,
        must_reset: false,
    }
}

fn term(slug: &str) -> Term {
    Term {
        id: TermId::new(),
        taxonomy: "tag".into(),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        parent_id: None,
    }
}

/// Every unique index and primary key the API can collide with reports the same
/// constraint name on both backends.
pub async fn unique_violations_name_the_constraint(db: &TestDb) {
    let store = db.store();
    let r = store.read();
    let author = fixture_user(store).await;

    // users
    let user = r
        .users()
        .insert(&new_user("alice", "a@x.test"))
        .await
        .unwrap();
    assert_unique(
        r.users().insert(&new_user("alice", "b@x.test")).await,
        "users_login_uq",
    );
    assert_unique(
        r.users().insert(&new_user("bob", "A@X.test")).await,
        "users_email_uq",
    );
    let mut same_id = new_user("carol", "c@x.test");
    same_id.id = user.id;
    assert_unique(r.users().insert(&same_id).await, "users_pkey");

    // content_items
    let post = r.content().insert(&new_post(&author, "p")).await.unwrap();
    assert_unique(
        r.content().insert(&new_post(&author, "p")).await,
        "content_items_slug_uq",
    );
    let mut same_id = new_post(&author, "q");
    same_id.id = post.id;
    assert_unique(r.content().insert(&same_id).await, "content_items_pkey");

    // revisions
    let revision = Revision {
        id: RevisionId::new(),
        item_id: post.id,
        author_id: author.id,
        created_at: Timestamp::now(),
        title: "t".into(),
        excerpt: String::new(),
        body: json!([]),
        fields: json!({}),
        meta: json!({}),
        note: None,
    };
    r.revisions().insert(&revision).await.unwrap();
    assert_unique(r.revisions().insert(&revision).await, "revisions_pkey");

    // terms and term links
    let t = term("rust");
    r.terms().insert(&t).await.unwrap();
    assert_unique(
        r.terms().insert(&term("rust")).await,
        "terms_taxonomy_slug_uq",
    );
    let mut same_id = term("go");
    same_id.id = t.id;
    assert_unique(r.terms().insert(&same_id).await, "terms_pkey");
    assert_unique(
        r.terms().set_for_item(post.id, &[t.id, t.id]).await,
        "term_links_pkey",
    );

    // menu items
    let item = MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position: 0,
        kind: MenuItemKind::Url,
        target_id: None,
        url: Some("/".into()),
        label: "Home".into(),
    };
    let menu = Menu {
        name: "main".into(),
        label: "Main".into(),
        items: vec![item.clone(), item],
    };
    assert_unique(r.menus().put(&menu).await, "menu_items_pkey");

    // redirects: upsert is keyed by source path, so only the id can collide.
    let redirect = Redirect {
        id: RedirectId::new(),
        source_path: "/old".into(),
        target: "/new".into(),
        code: 301,
        hits: 0,
        origin: RedirectOrigin::Manual,
        created_at: Timestamp::now(),
        last_hit_at: None,
    };
    r.redirects().upsert(&redirect).await.unwrap();
    let moved = Redirect {
        source_path: "/other".into(),
        ..redirect
    };
    assert_unique(r.redirects().upsert(&moved).await, "redirects_pkey");

    // media and themes need an artifact
    let hash = ArtifactHash::parse(&"a".repeat(64)).unwrap();
    r.artifacts()
        .insert_if_absent(&Artifact {
            hash: hash.clone(),
            kind: ArtifactKind::Media,
            bytes: 1,
            mime: "image/png".into(),
            created_at: Timestamp::now(),
        })
        .await
        .unwrap();
    let media = Media {
        item_id: post.id,
        artifact_hash: hash.clone(),
        mime: "image/png".into(),
        bytes: 1,
        width: None,
        height: None,
        alt: String::new(),
        original_name: "a.png".into(),
        variants: json!({}),
    };
    r.media().insert(&media).await.unwrap();
    assert_unique(r.media().insert(&media).await, "media_pkey");

    let theme = |name: &str| InstalledTheme {
        name: name.into(),
        version: "1.0.0".into(),
        artifact_hash: hash.clone(),
        manifest: json!({}),
        active: true,
        installed_at: Timestamp::now(),
    };
    r.themes().upsert(&theme("one")).await.unwrap();
    assert_unique(
        r.themes().upsert(&theme("two")).await,
        "themes_one_active_uq",
    );
}

/// SQLite reports no constraint name for foreign-key errors, so the contract is an
/// empty `constraint` on every backend.
pub async fn foreign_key_violations_have_empty_constraint(db: &TestDb) {
    let store = db.store();
    let r = store.read();
    let author = fixture_user(store).await;

    let mut orphan = new_post(&author, "orphan");
    orphan.parent_id = Some(ContentId::new());
    let mut unknown_type = new_post(&author, "typed");
    unknown_type.kind = "nope".into();
    for result in [
        r.content().insert(&orphan).await,
        r.content().insert(&unknown_type).await,
    ] {
        match result {
            Err(StoreError::ForeignKey { constraint }) => assert_eq!(constraint, ""),
            other => panic!("expected ForeignKey, got {other:?}"),
        }
    }
    let parent = r
        .content()
        .insert(&new_post(&author, "parent"))
        .await
        .unwrap();
    let mut child = new_post(&author, "child");
    child.parent_id = Some(parent.id);
    r.content().insert(&child).await.unwrap();
    match r.content().delete(parent.id).await {
        Err(StoreError::ForeignKey { constraint }) => assert_eq!(constraint, ""),
        other => panic!("expected ForeignKey, got {other:?}"),
    }
}
