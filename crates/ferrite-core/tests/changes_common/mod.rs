//! Shared helpers of the change-set tests (`change_capture`, `change_revert`).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, Agent, ContentDraft, ContentService, MediaService, NewUpload, PermalinkRules,
    changes::open_image, permalink::Permalinks,
};
use ferrite_domain::{
    ApiTokenId, Artifact, ArtifactHash, ArtifactKind, Change, ChangeSet, ChangeSetQuery, Image,
    NewUser, Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::TestDb;
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
