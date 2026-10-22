//! Builders for contract tests.

use ferrite_domain::{
    ContentId, ContentStatus, NewContent, NewUser, Role, User, UserId, UserStatus,
};
use ferrite_store::Store;
use serde_json::json;

/// Inserts a fresh admin user (unique login and e-mail) and returns the stored row.
pub async fn fixture_user(store: &dyn Store) -> User {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    let user = tx
        .users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Fixture User".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    user
}

/// A draft `post` by `author` with the given slug and a small block body.
pub fn new_post(author: &User, slug: &str) -> NewContent {
    NewContent {
        id: ContentId::new(),
        kind: "post".into(),
        status: ContentStatus::Draft,
        slug: slug.into(),
        parent_id: None,
        author_id: author.id,
        title: format!("Title of {slug}"),
        excerpt: String::new(),
        body: json!([{"type": "paragraph", "text": slug}]),
        body_html: None,
        fields: json!({}),
        menu_order: 0,
        published_at: None,
        scheduled_at: None,
    }
}
