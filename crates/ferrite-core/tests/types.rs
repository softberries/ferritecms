//! Content types and taxonomies registered through core (P18/P19, F17, F26), and the
//! validation of item fields against a type's schema, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    Actor, ContentDraft, ContentService, ContentTypeService, CoreError, NewType, OptionService,
    PermalinkRules, Scopes, TaxonomyService, TypePatch,
    events::{ALL_KEY, TYPES_CHANGED},
    fields::{FieldDef, FieldKind},
};
use ferrite_domain::{NewUser, Role, Taxonomy, UserId, UserStatus};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests, backend_tests_spawning};
use serde_json::{Value, json};

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

fn content(db: &TestDb) -> ContentService {
    ContentService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
}

fn types(db: &TestDb) -> ContentTypeService {
    ContentTypeService::new(db.shared_store())
}

fn field(name: &str, kind: FieldKind) -> FieldDef {
    FieldDef {
        name: name.into(),
        label: name.into(),
        kind,
        required: false,
        options: Vec::new(),
        fields: Vec::new(),
        types: Vec::new(),
    }
}

fn relation(name: &str, targets: &[&str]) -> FieldDef {
    let mut f = field(name, FieldKind::Relation);
    f.types = targets.iter().map(|t| (*t).to_owned()).collect();
    f
}

fn new_type(name: &str, fields: Vec<FieldDef>) -> NewType {
    NewType {
        name: name.into(),
        label: format!("{name} label"),
        hierarchical: false,
        has_archive: true,
        fields: Some(fields),
    }
}

fn draft(kind: &str, title: &str, fields: Value) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: None,
        parent_id: None,
        excerpt: None,
        body: json!({"type": "doc", "content": []}),
        fields,
        menu_order: 0,
        terms: Vec::new(),
        seo: None,
    }
}

/// The message of an `InvalidField { field, .. }` (or the text of an `Invalid`).
fn invalid<T: std::fmt::Debug>(r: Result<T, CoreError>, want_field: &str) -> String {
    match r {
        Err(CoreError::InvalidField { field, message }) if field == want_field => message,
        other => panic!("expected InvalidField({want_field}), got {other:?}"),
    }
}

/// A name its own registry already holds: the primary key's unique violation (409).
fn conflict<T: std::fmt::Debug>(r: Result<T, CoreError>, constraint: &str) {
    match r {
        Err(CoreError::Store(ferrite_store::StoreError::UniqueViolation { constraint: c }))
            if c == constraint => {}
        other => panic!("expected UniqueViolation({constraint}), got {other:?}"),
    }
}

fn plain_invalid<T: std::fmt::Debug>(r: Result<T, CoreError>) -> String {
    match r {
        Err(CoreError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn genre(types: &[&str]) -> Taxonomy {
    Taxonomy {
        name: "genre".into(),
        label: "Genres".into(),
        hierarchical: false,
        content_types: types.iter().map(|t| (*t).to_owned()).collect(),
        built_in: false,
    }
}

pub async fn field_values_validated_on_write(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = content(db);
    let mut title = field("subtitle", FieldKind::Text);
    title.required = true;
    let mut kind = field("format", FieldKind::Select);
    kind.options = vec!["hardcover".into(), "paperback".into()];
    let mut notes = field("notes", FieldKind::Repeater);
    notes.fields = vec![field("text", FieldKind::Text)];
    let mut heading = field("heading", FieldKind::Text);
    heading.required = true;
    let mut chapters = field("chapters", FieldKind::Repeater);
    chapters.fields = vec![heading, field("pages", FieldKind::Number), notes];
    let book = types(db)
        .create(
            &admin,
            new_type(
                "book",
                vec![title, field("pages", FieldKind::Number), kind, chapters],
            ),
        )
        .await
        .unwrap();
    assert_eq!(book.fields["fields"].as_array().unwrap().len(), 4);

    let ok = json!({
        "subtitle": "A tale",
        "pages": 320,
        "format": "paperback",
        "chapters": [
            {"heading": "One", "pages": 12, "notes": [{"text": "n"}]},
            {"heading": "Two"}
        ],
        "featured_image": null,
    });
    let item = svc
        .create(&admin, draft("book", "B", ok.clone()))
        .await
        .unwrap();
    assert_eq!(item.fields, ok);

    let refused = |fields: Value| {
        let svc = &svc;
        let admin = &admin;
        async move {
            invalid(
                svc.create(admin, draft("book", "B", fields)).await,
                "fields",
            )
        }
    };
    // Unknown key, wrong kind, missing required, a select value not offered.
    let m = refused(json!({"subtitle": "s", "colour": "red"})).await;
    assert_eq!(m, "fields.colour: unknown field");
    let m = refused(json!({"subtitle": "s", "pages": "320"})).await;
    assert!(m.starts_with("fields.pages:"), "{m}");
    assert_eq!(
        refused(json!({"pages": 3})).await,
        "fields.subtitle: required"
    );
    assert_eq!(
        refused(json!({"subtitle": null})).await,
        "fields.subtitle: required"
    );
    refused(json!({"subtitle": "s", "format": "scroll"})).await;
    // Repeater rows are validated recursively, with the path.
    let m = refused(json!({"subtitle": "s", "chapters": [{"heading": "a"}, {"pages": 1}]})).await;
    assert_eq!(m, "fields.chapters[1].heading: required");
    let m = refused(json!({"subtitle": "s", "chapters": [
        {"heading": "a", "notes": [{"text": 5}]}
    ]}))
    .await;
    assert!(m.starts_with("fields.chapters[0].notes[0].text:"), "{m}");
    refused(json!({"subtitle": "s", "chapters": {"heading": "a"}})).await;
    refused(json!({"subtitle": "s", "featured_image": "not-a-uuid"})).await;
    refused(json!(["not", "an", "object"])).await;

    // Update validates too (and keeps the stored item on refusal).
    let r = svc
        .update(
            &admin,
            item.id,
            item.version,
            draft("book", "B", json!({"pages": 1})),
        )
        .await;
    assert_eq!(invalid(r, "fields"), "fields.subtitle: required");
    assert_eq!(svc.get(item.id).await.unwrap(), item);

    // A registered type with an empty schema takes `{}` (and featured_image) only.
    types(db)
        .create(&admin, new_type("note", vec![]))
        .await
        .unwrap();
    svc.create(&admin, draft("note", "N", json!({})))
        .await
        .unwrap();
    let image = uuid::Uuid::now_v7().to_string();
    svc.create(&admin, draft("note", "N", json!({"featured_image": image})))
        .await
        .unwrap();
    invalid(
        svc.create(&admin, draft("note", "N", json!({"x": 1})))
            .await,
        "fields",
    );

    // A built-in type without a schema (`{}`, F17) takes any object, but only objects.
    let any = json!({"featured_image": image, "anything": [1, {"x": true}]});
    svc.create(&admin, draft("post", "P", any)).await.unwrap();
    invalid(
        svc.create(&admin, draft("post", "P", json!(3))).await,
        "fields",
    );

    // Giving `post` a schema makes its next save validate.
    types(db)
        .update(
            &admin,
            "post",
            TypePatch {
                fields: Some(Some(vec![field("mood", FieldKind::Text)])),
                ..TypePatch::default()
            },
        )
        .await
        .unwrap();
    invalid(
        svc.create(&admin, draft("post", "P", json!({"anything": 1})))
            .await,
        "fields",
    );
    svc.create(&admin, draft("post", "P", json!({"mood": "ok"})))
        .await
        .unwrap();
    // Removing the schema again (`Some(None)`) makes `post` schemaless: stored `{}`.
    let post = types(db)
        .update(
            &admin,
            "post",
            TypePatch {
                fields: Some(None),
                ..TypePatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(post.fields, json!({}));
    svc.create(&admin, draft("post", "P", json!({"anything": 1})))
        .await
        .unwrap();
    // A type registered without a schema takes any object too.
    let mut free = new_type("free", vec![]);
    free.fields = None;
    assert_eq!(
        types(db).create(&admin, free).await.unwrap().fields,
        json!({})
    );
    svc.create(&admin, draft("free", "F", json!({"x": [1]})))
        .await
        .unwrap();
}

pub async fn type_names_and_permalink_conflicts(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = types(db);
    let options = OptionService::new(db.shared_store());
    options
        .set(&admin, "permalink.post", &json!("/news/%slug%/"), true)
        .await
        .unwrap();
    // The new type's archive `/news/` would shadow every post.
    let m = invalid(svc.create(&admin, new_type("news", vec![])).await, "name");
    assert!(
        m.contains("\"/news/%slug%/\"") && m.contains("content type"),
        "{m}"
    );
    assert!(store.read().types().get("news").await.unwrap().is_none());
    // So would a taxonomy of that name.
    let mut tax = genre(&["post"]);
    tax.name = "news".into();
    let m = invalid(
        TaxonomyService::new(db.shared_store())
            .create_taxonomy(&admin, tax)
            .await,
        "name",
    );
    assert!(m.contains("taxonomy"), "{m}");

    // Router words, reserved words, years and malformed names.
    for bad in [
        "admin", "api", "feed", "author", "page", "search", "healthz", "Event", "1999", "a b",
        "_x", "",
    ] {
        invalid(svc.create(&admin, new_type(bad, vec![])).await, "name");
    }
    invalid(
        svc.create(&admin, new_type(&"a".repeat(33), vec![])).await,
        "name",
    );
    // Built-in and existing types, and taxonomies.
    for taken in ["post", "media"] {
        conflict(
            svc.create(&admin, new_type(taken, vec![])).await,
            "content_types_pkey",
        );
    }
    for taken in ["category", "tag"] {
        invalid(svc.create(&admin, new_type(taken, vec![])).await, "name");
    }
    // A top-level page (or post) slug (F26).
    let page = content(db)
        .create(&admin, {
            let mut d = draft("page", "About", json!({}));
            d.slug = Some("about".into());
            d
        })
        .await
        .unwrap();
    assert_eq!(page.slug, "about");
    let m = invalid(svc.create(&admin, new_type("about", vec![])).await, "name");
    assert_eq!(m, "name used by an existing item");
    // Labels and schemas are checked.
    let mut nameless = new_type("event", vec![]);
    nameless.label = "  ".into();
    invalid(svc.create(&admin, nameless).await, "label");
    invalid(
        svc.create(
            &admin,
            new_type("event", vec![field("Bad", FieldKind::Text)]),
        )
        .await,
        "fields",
    );
    // A fine name works, and is then taken.
    let event = svc.create(&admin, new_type("event", vec![])).await.unwrap();
    assert!(!event.built_in && event.has_archive);
    assert_eq!(event.fields, json!({"fields": []}));
    conflict(
        svc.create(&admin, new_type("event", vec![])).await,
        "content_types_pkey",
    );
    // The losing create changed nothing.
    assert_eq!(
        store.read().types().get("event").await.unwrap().unwrap(),
        event
    );

    // Permissions: manage_settings for writes, content:read for reads.
    let editor = actor(store, Role::Editor).await;
    assert!(matches!(
        svc.create(&editor, new_type("film", vec![])).await,
        Err(CoreError::Forbidden("manage_settings"))
    ));
    let settings_read = admin.with_scopes(Scopes::from_strs(&["settings:read".into()]).unwrap());
    assert!(matches!(
        svc.create(&settings_read, new_type("film", vec![])).await,
        Err(CoreError::Forbidden("settings:write"))
    ));
    assert!(matches!(
        svc.list(&settings_read).await,
        Err(CoreError::Forbidden("content:read"))
    ));
    let names: Vec<String> = svc
        .list(&editor)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["event", "media", "page", "post"]);
    assert_eq!(svc.get(&editor, "event").await.unwrap(), event);
    assert!(matches!(
        svc.get(&editor, "nope").await,
        Err(CoreError::NotFound)
    ));
}

/// Stored permalink options that no longer parse are not the request's fault: plain
/// `Invalid` naming them, on create and update alike (review Minor 1).
pub async fn stored_permalink_errors_are_not_field_errors(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = types(db);
    svc.create(&admin, new_type("event", vec![])).await.unwrap();
    store
        .read()
        .options()
        .set("permalink.post", &json!(5), true)
        .await
        .unwrap();
    let m = plain_invalid(svc.create(&admin, new_type("film", vec![])).await);
    assert!(
        m.contains("stored permalink options") && m.contains("permalink.post"),
        "{m}"
    );
    let m = plain_invalid(
        svc.update(
            &admin,
            "event",
            TypePatch {
                has_archive: Some(false),
                ..TypePatch::default()
            },
        )
        .await,
    );
    assert!(m.contains("stored permalink options"), "{m}");
    assert!(store.read().types().get("film").await.unwrap().is_none());
    store
        .read()
        .options()
        .delete("permalink.post")
        .await
        .unwrap();
}

pub async fn type_delete_rules(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = types(db);
    assert_eq!(
        plain_invalid(svc.delete(&admin, "post").await),
        "built-in type"
    );
    assert!(matches!(
        svc.delete(&admin, "nope").await,
        Err(CoreError::NotFound)
    ));

    // Update: label, has_archive and schema change; hierarchical stays.
    let mut ev = new_type("event", vec![]);
    ev.hierarchical = true;
    let created = svc.create(&admin, ev).await.unwrap();
    let updated = svc
        .update(
            &admin,
            "event",
            TypePatch {
                label: Some(" Events ".into()),
                has_archive: Some(false),
                fields: Some(Some(vec![field("venue", FieldKind::Text)])),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.label, "Events");
    assert!(!updated.has_archive && updated.hierarchical);
    assert_eq!(updated.created_at, created.created_at);
    assert_eq!(updated.fields["fields"][0]["name"], "venue");
    assert_eq!(
        store.read().types().get("event").await.unwrap().unwrap(),
        updated
    );
    assert!(matches!(
        svc.update(&admin, "nope", TypePatch::default()).await,
        Err(CoreError::NotFound)
    ));
    invalid(
        svc.update(
            &admin,
            "event",
            TypePatch {
                label: Some(String::new()),
                ..TypePatch::default()
            },
        )
        .await,
        "label",
    );

    // With content (trash too): refused.
    let svc_content = content(db);
    let item = svc_content
        .create(&admin, draft("event", "E", json!({})))
        .await
        .unwrap();
    let trashed = svc_content
        .trash(&admin, item.id, item.version)
        .await
        .unwrap();
    assert_eq!(
        plain_invalid(svc.delete(&admin, "event").await),
        "type has content"
    );
    svc_content
        .delete(&admin, trashed.id, trashed.version)
        .await
        .unwrap();

    // Attached to a taxonomy: refused, naming it.
    let taxes = TaxonomyService::new(db.shared_store());
    taxes
        .create_taxonomy(&admin, genre(&["event"]))
        .await
        .unwrap();
    let m = plain_invalid(svc.delete(&admin, "event").await);
    assert!(m.contains("\"genre\""), "{m}");
    taxes
        .update_taxonomy(&admin, "genre", None, Some(vec!["post".into()]))
        .await
        .unwrap();

    // Editors cannot; an admin can, and the type is gone.
    let editor = actor(store, Role::Editor).await;
    assert!(matches!(
        svc.delete(&editor, "event").await,
        Err(CoreError::Forbidden("manage_settings"))
    ));
    svc.delete(&admin, "event").await.unwrap();
    assert!(store.read().types().get("event").await.unwrap().is_none());
    // Items of a removed type cannot be created.
    assert!(
        svc_content
            .create(&admin, draft("event", "E", json!({})))
            .await
            .is_err()
    );
}

pub async fn taxonomy_register_and_delete(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    types(db)
        .create(&admin, new_type("book", vec![]))
        .await
        .unwrap();
    let svc = TaxonomyService::new(db.shared_store());

    // Rules: types must exist, not built in, name free, label set.
    let m = invalid(
        svc.create_taxonomy(&admin, genre(&["nope"])).await,
        "content_types",
    );
    assert!(m.contains("\"nope\""), "{m}");
    let mut built = genre(&["book"]);
    built.built_in = true;
    invalid(svc.create_taxonomy(&admin, built).await, "built_in");
    conflict(
        svc.create_taxonomy(&admin, {
            let mut t = genre(&["book"]);
            t.name = "category".into();
            t
        })
        .await,
        "taxonomies_pkey",
    );
    for taken in ["book", "post", "page"] {
        let mut t = genre(&["book"]);
        t.name = taken.into();
        invalid(svc.create_taxonomy(&admin, t).await, "name");
    }
    let mut unlabeled = genre(&["book"]);
    unlabeled.label = String::new();
    invalid(svc.create_taxonomy(&admin, unlabeled).await, "label");

    let created = svc
        .create_taxonomy(&admin, genre(&["book", "post", "book"]))
        .await
        .unwrap();
    assert_eq!(created.content_types, ["book", "post"]);
    assert_eq!(
        store
            .read()
            .taxonomies()
            .get("genre")
            .await
            .unwrap()
            .unwrap(),
        created
    );
    conflict(
        svc.create_taxonomy(&admin, genre(&["book"])).await,
        "taxonomies_pkey",
    );
    let names: Vec<String> = svc
        .list_taxonomies(&admin)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["category", "genre", "tag"]);
    assert_eq!(svc.get_taxonomy(&admin, "genre").await.unwrap(), created);

    // Update: label and types; unknown types refused.
    let updated = svc
        .update_taxonomy(
            &admin,
            "genre",
            Some("Kinds".into()),
            Some(vec!["book".into()]),
        )
        .await
        .unwrap();
    assert_eq!(
        (updated.label.as_str(), updated.content_types.as_slice()),
        ("Kinds", &["book".to_owned()][..])
    );
    invalid(
        svc.update_taxonomy(&admin, "genre", None, Some(vec!["nope".into()]))
            .await,
        "content_types",
    );
    assert!(matches!(
        svc.update_taxonomy(&admin, "nope", None, None).await,
        Err(CoreError::NotFound)
    ));

    // Terms: reads need terms:read.
    let jazz = svc
        .create_term(&admin, "genre", "Jazz", None, None, "")
        .await
        .unwrap();
    assert_eq!(
        svc.list_terms_for(&admin, "genre").await.unwrap(),
        std::slice::from_ref(&jazz)
    );
    assert_eq!(svc.get_term(&admin, jazz.id).await.unwrap(), jazz);
    let content_only = admin.with_scopes(Scopes::from_strs(&["content:read".into()]).unwrap());
    assert!(matches!(
        svc.list_terms_for(&content_only, "genre").await,
        Err(CoreError::Forbidden("terms:read"))
    ));
    assert!(matches!(
        svc.get_term(&content_only, jazz.id).await,
        Err(CoreError::Forbidden("terms:read"))
    ));
    assert!(svc.list_taxonomies(&content_only).await.is_ok());

    // Delete: with terms, built in, missing, by an editor.
    assert_eq!(
        plain_invalid(svc.delete_taxonomy(&admin, "genre").await),
        "taxonomy has terms"
    );
    assert_eq!(
        plain_invalid(svc.delete_taxonomy(&admin, "category").await),
        "built-in taxonomy"
    );
    assert!(matches!(
        svc.delete_taxonomy(&admin, "nope").await,
        Err(CoreError::NotFound)
    ));
    let editor = actor(store, Role::Editor).await;
    assert!(matches!(
        svc.delete_taxonomy(&editor, "genre").await,
        Err(CoreError::Forbidden("manage_settings"))
    ));
    assert!(matches!(
        svc.create_taxonomy(&editor, genre(&["book"])).await,
        Err(CoreError::Forbidden("manage_settings"))
    ));
    svc.delete_term(&admin, jazz.id).await.unwrap();
    svc.delete_taxonomy(&admin, "genre").await.unwrap();
    assert!(
        store
            .read()
            .taxonomies()
            .get("genre")
            .await
            .unwrap()
            .is_none()
    );
    // Terms of a removed taxonomy cannot be created.
    assert!(
        svc.create_term(&admin, "genre", "Jazz", None, None, "")
            .await
            .is_err()
    );
}

pub async fn types_changed_emitted_with_all(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = types(db);
    let taxes = TaxonomyService::new(db.shared_store());
    let events = store.read().events();

    // Every write: `types.changed` with its key, `invalidate [all]`, an audit row.
    let steps: Vec<(&str, &str)> = vec![
        ("type:event", "type.create"),
        ("type:event", "type.update"),
        ("taxonomy:genre", "taxonomy.create"),
        ("taxonomy:genre", "taxonomy.update"),
        ("taxonomy:genre", "taxonomy.delete"),
        ("type:event", "type.delete"),
    ];
    for (i, (key, action)) in steps.iter().enumerate() {
        let before = events.max_seq().await.unwrap();
        match i {
            0 => {
                svc.create(&admin, new_type("event", vec![])).await.unwrap();
            }
            1 => {
                svc.update(
                    &admin,
                    "event",
                    TypePatch {
                        has_archive: Some(false),
                        ..TypePatch::default()
                    },
                )
                .await
                .unwrap();
            }
            2 => {
                taxes
                    .create_taxonomy(&admin, genre(&["event"]))
                    .await
                    .unwrap();
            }
            3 => {
                taxes
                    .update_taxonomy(&admin, "genre", Some("G".into()), None)
                    .await
                    .unwrap();
            }
            4 => taxes.delete_taxonomy(&admin, "genre").await.unwrap(),
            _ => svc.delete(&admin, "event").await.unwrap(),
        }
        let got: Vec<(String, Vec<String>)> = events
            .since(before, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.keys))
            .collect();
        assert_eq!(
            got,
            [
                (TYPES_CHANGED.to_owned(), vec![(*key).to_owned()]),
                ("invalidate".to_owned(), vec![ALL_KEY.to_owned()]),
            ],
            "{action}"
        );
        let audit = store
            .read()
            .audit()
            .list(&ferrite_domain::AuditQuery::first(1))
            .await
            .unwrap();
        let row = &audit.items[0];
        assert_eq!(row.action, *action);
        assert_eq!(row.actor_id, Some(admin.user));
        assert_eq!(row.target_id.as_deref(), key.split(':').nth(1));
    }

    // A refused write emits nothing.
    let before = events.max_seq().await.unwrap();
    assert!(svc.create(&admin, new_type("page", vec![])).await.is_err());
    assert!(svc.delete(&admin, "post").await.is_err());
    assert_eq!(events.max_seq().await.unwrap(), before);
}

/// Review Important 1: concurrent registrations on two nodes (Postgres `peer()`; on
/// SQLite one store, whose writes are serialised, takes the same conflict path).
/// Exactly one of two same-name creates wins, within a registry and across them.
pub async fn concurrent_registration_one_wins(db: Arc<TestDb>) {
    let store = db.shared_store();
    let other = db.peer().await.unwrap_or_else(|| db.shared_store());
    let admin = actor(store.as_ref(), Role::Admin).await;
    for round in 0..4 {
        let name = format!("race{round}");
        let both_types = round % 2 == 0;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let spawn = |store: Arc<dyn Store>, as_type: bool| {
            let (barrier, name) = (barrier.clone(), name.clone());
            tokio::spawn(async move {
                barrier.wait().await;
                if as_type {
                    ContentTypeService::new(store)
                        .create(&admin, new_type(&name, vec![]))
                        .await
                        .map(|_| ())
                } else {
                    let mut t = genre(&["post"]);
                    t.name = name;
                    TaxonomyService::new(store)
                        .create_taxonomy(&admin, t)
                        .await
                        .map(|_| ())
                }
            })
        };
        let a = spawn(store.clone(), true);
        let b = spawn(other.clone(), both_types);
        let results = [a.await.unwrap(), b.await.unwrap()];
        let wins = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(wins, 1, "round {round}: {results:?}");
        let lost = results.into_iter().find_map(Result::err).unwrap();
        match (&lost, both_types) {
            (CoreError::Store(ferrite_store::StoreError::UniqueViolation { constraint }), true) => {
                assert_eq!(constraint, "content_types_pkey");
            }
            (CoreError::InvalidField { field: "name", .. }, false) => {}
            other => panic!("round {round}: unexpected loser {other:?}"),
        }
        let rows = usize::from(store.read().types().get(&name).await.unwrap().is_some())
            + usize::from(
                store
                    .read()
                    .taxonomies()
                    .get(&name)
                    .await
                    .unwrap()
                    .is_some(),
            );
        assert_eq!(rows, 1, "round {round}: one registration persists");
    }
}

backend_tests_spawning!(concurrent_registration_one_wins);

/// A taxonomy applies to at most 20 types (a term archive lists at most that many);
/// duplicates do not count.
pub async fn taxonomy_content_types_capped(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    types(db)
        .create(&admin, new_type("book", vec![]))
        .await
        .unwrap();
    let svc = TaxonomyService::new(db.shared_store());
    let many: Vec<String> = (0..21).map(|i| format!("t{i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let m = invalid(
        svc.create_taxonomy(&admin, genre(&refs)).await,
        "content_types",
    );
    assert!(m.contains("20"), "{m}");
    let mut dups = vec!["book"; 25];
    dups.push("post");
    let created = svc.create_taxonomy(&admin, genre(&dups)).await.unwrap();
    assert_eq!(created.content_types, ["book", "post"]);
    invalid(
        svc.update_taxonomy(&admin, "genre", None, Some(many.clone()))
            .await,
        "content_types",
    );
    assert_eq!(
        svc.get_taxonomy(&admin, "genre")
            .await
            .unwrap()
            .content_types,
        ["book", "post"]
    );
}

/// The `key` of an `InvalidKey` (a field error at a data path) and its message.
fn invalid_at<T: std::fmt::Debug>(r: Result<T, CoreError>) -> (String, String) {
    match r {
        Err(CoreError::InvalidKey { key, message }) => (key, message),
        other => panic!("expected InvalidKey, got {other:?}"),
    }
}

/// P52: `types` belongs to relation fields only (sub-fields of a repeater too) and
/// names registered types other than `media`; a type named there cannot be deleted.
pub async fn relation_types_must_name_types(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = types(db);
    let refused = |r: Result<ferrite_domain::ContentType, CoreError>| invalid(r, "fields");

    let mut text = field("t", FieldKind::Text);
    text.types = vec!["page".into()];
    let m = refused(svc.create(&admin, new_type("book", vec![text])).await);
    assert!(m.contains("fields.t") && m.contains("only relation"), "{m}");
    let m = refused(
        svc.create(&admin, new_type("book", vec![relation("r", &["nope"])]))
            .await,
    );
    assert!(m.contains("fields.r") && m.contains("\"nope\""), "{m}");
    let m = refused(
        svc.create(&admin, new_type("book", vec![relation("r", &["media"])]))
            .await,
    );
    assert!(m.contains("media"), "{m}");
    refused(
        svc.create(
            &admin,
            new_type("book", vec![relation("r", &["page", "page"])]),
        )
        .await,
    );
    let mut chapters = field("chapters", FieldKind::Repeater);
    chapters.fields = vec![relation("related", &["nope"])];
    let m = refused(svc.create(&admin, new_type("book", vec![chapters])).await);
    assert!(m.starts_with("fields.chapters.related:"), "{m}");
    assert!(
        svc.list(&admin)
            .await
            .unwrap()
            .iter()
            .all(|t| t.name != "book")
    );

    // Registered names, the new type's own name included, are fine.
    let mut chapters = field("chapters", FieldKind::Repeater);
    chapters.fields = vec![relation("related", &["book", "post"])];
    let book = svc
        .create(
            &admin,
            new_type("book", vec![relation("r", &["page"]), chapters]),
        )
        .await
        .unwrap();
    assert_eq!(book.fields["fields"][0]["types"], json!(["page"]));
    assert_eq!(
        book.fields["fields"][1]["fields"][0]["types"],
        json!(["book", "post"])
    );
    // An update is checked the same way.
    refused(
        svc.update(
            &admin,
            "book",
            TypePatch {
                fields: Some(Some(vec![relation("r", &["gone"])])),
                ..TypePatch::default()
            },
        )
        .await,
    );
    // A type another type's relation names cannot be deleted.
    svc.create(&admin, new_type("author_bio", vec![]))
        .await
        .unwrap();
    svc.create(
        &admin,
        new_type("quote", vec![relation("by", &["author_bio"])]),
    )
    .await
    .unwrap();
    let m = plain_invalid(svc.delete(&admin, "author_bio").await);
    assert!(m.contains("quote"), "{m}");
    svc.delete(&admin, "quote").await.unwrap();

    // A stored schema that no longer parses (written past core) blocks the delete
    // instead of being skipped: it might name the type.
    let mut broken = svc.get(&admin, "book").await.unwrap();
    broken.fields = json!({"fields": [{"name": "r", "kind": "colour"}]});
    store.read().types().upsert(&broken).await.unwrap();
    let m = plain_invalid(svc.delete(&admin, "author_bio").await);
    assert!(m.contains("\"book\"") && m.contains("unreadable"), "{m}");
    broken.fields = json!({});
    store.read().types().upsert(&broken).await.unwrap();
    svc.delete(&admin, "author_bio").await.unwrap();
}

/// P52: a relation value whose item is of a type the field does not list is refused at
/// its path (repeater rows included), on create and update; a missing item is accepted
/// (as before: it may be deleted later anyway), and a relation without `types` takes
/// any item.
pub async fn relation_types_refuse_other_type(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = content(db);
    let mut chapters = field("chapters", FieldKind::Repeater);
    chapters.fields = vec![
        field("title", FieldKind::Text),
        relation("related", &["post"]),
    ];
    types(db)
        .create(
            &admin,
            new_type(
                "book",
                vec![
                    relation("related", &["page"]),
                    relation("any", &[]),
                    chapters,
                ],
            ),
        )
        .await
        .unwrap();
    let page = svc
        .create(&admin, draft("page", "A page", json!({})))
        .await
        .unwrap()
        .id
        .to_string();
    let post = svc
        .create(&admin, draft("post", "A post", json!({})))
        .await
        .unwrap()
        .id
        .to_string();
    let missing = uuid::Uuid::now_v7().to_string();

    let ok = json!({
        "related": page,
        "any": post,
        "chapters": [{"related": post}, {"title": "t", "related": missing}],
    });
    let item = svc
        .create(&admin, draft("book", "B", ok.clone()))
        .await
        .unwrap();
    assert_eq!(item.fields, ok);

    let (key, message) = invalid_at(
        svc.create(&admin, draft("book", "B", json!({"related": post})))
            .await,
    );
    assert_eq!(key, "fields.related");
    assert!(
        message.starts_with("fields.related:") && message.contains("page"),
        "{message}"
    );
    let (key, _) = invalid_at(
        svc.create(
            &admin,
            draft(
                "book",
                "B",
                json!({"chapters": [{"related": post}, {"related": page}]}),
            ),
        )
        .await,
    );
    assert_eq!(key, "fields.chapters[1].related");
    let (key, _) = invalid_at(
        svc.update(
            &admin,
            item.id,
            item.version,
            draft("book", "B", json!({"related": item.id.to_string()})),
        )
        .await,
    );
    assert_eq!(key, "fields.related");
    assert_eq!(svc.get(item.id).await.unwrap(), item);
}

backend_tests!(
    relation_types_must_name_types,
    relation_types_refuse_other_type,
    stored_permalink_errors_are_not_field_errors,
    field_values_validated_on_write,
    type_names_and_permalink_conflicts,
    type_delete_rules,
    taxonomy_register_and_delete,
    types_changed_emitted_with_all,
    taxonomy_content_types_capped,
);
