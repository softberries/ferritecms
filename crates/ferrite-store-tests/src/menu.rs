//! Menu contract tests.

use ferrite_domain::{Menu, MenuItem, MenuItemId, MenuItemKind};
use ferrite_store::StoreError;
use serde_json::json;
use uuid::Uuid;

use crate::TestDb;

fn item(
    id: MenuItemId,
    parent: Option<MenuItemId>,
    position: i32,
    kind: MenuItemKind,
    label: &str,
) -> MenuItem {
    let (target_id, url) = match kind {
        MenuItemKind::Url => (None, Some(format!("/{label}"))),
        _ => (Some(Uuid::now_v7()), None),
    };
    MenuItem {
        id,
        parent_id: parent,
        position,
        kind,
        target_id,
        url,
        label: label.into(),
    }
}

/// Three levels in depth-first order (the order `get` returns):
/// home, about(team, people(ceo)), docs.
fn tree(name: &str) -> Menu {
    let [home, about, team, people, ceo, docs] = std::array::from_fn(|_| MenuItemId::new());
    Menu {
        name: name.into(),
        label: "Main ☃".into(),
        items: vec![
            item(home, None, 0, MenuItemKind::Url, "home"),
            item(about, None, 1, MenuItemKind::Item, "about"),
            item(team, Some(about), 0, MenuItemKind::Term, "team"),
            item(people, Some(about), 1, MenuItemKind::Url, "people"),
            item(ceo, Some(people), 0, MenuItemKind::Item, "ceo"),
            item(docs, None, 2, MenuItemKind::Url, "docs"),
        ],
    }
}

pub async fn menu_put_replaces_tree(db: &TestDb) {
    let store = db.store();
    let menus = store.read().menus();
    assert!(menus.get("main").await.unwrap().is_none());

    let full = tree("main");
    menus.put(&full).await.unwrap();
    assert_eq!(menus.get("main").await.unwrap().unwrap(), full);

    // Children listed before their parents are still stored; `get` returns tree order.
    let mut shuffled = full.clone();
    shuffled.items.reverse();
    menus.put(&shuffled).await.unwrap();
    assert_eq!(menus.get("main").await.unwrap().unwrap(), full);

    // A second put with fewer items removes the rest and updates the label.
    let small = Menu {
        name: "main".into(),
        label: "Small".into(),
        items: vec![full.items[1].clone(), full.items[2].clone()],
    };
    menus.put(&small).await.unwrap();
    assert_eq!(menus.get("main").await.unwrap().unwrap(), small);

    // Items of other menus are untouched; the same ids cannot be reused across menus.
    let other = tree("footer");
    menus.put(&other).await.unwrap();
    assert_eq!(menus.get("main").await.unwrap().unwrap(), small);
    let mut clash = tree("third");
    clash.items[0].id = other.items[0].id;
    assert!(matches!(
        menus.put(&clash).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    assert!(menus.get("third").await.unwrap().is_none());

    assert_eq!(
        menus.list().await.unwrap(),
        [
            ("footer".to_owned(), "Main ☃".to_owned()),
            ("main".to_owned(), "Small".to_owned())
        ]
    );

    menus
        .put(&Menu {
            name: "main".into(),
            label: "Empty".into(),
            items: vec![],
        })
        .await
        .unwrap();
    assert!(menus.get("main").await.unwrap().unwrap().items.is_empty());

    menus.delete("footer").await.unwrap();
    assert!(menus.get("footer").await.unwrap().is_none());
    menus.delete("footer").await.unwrap();
    assert_eq!(menus.list().await.unwrap().len(), 1);
}

pub async fn menu_rejects_bad_parents(db: &TestDb) {
    let menus = db.store().read().menus();
    let good = tree("main");
    menus.put(&good).await.unwrap();

    let mut orphan = tree("main");
    orphan.items[2].parent_id = Some(MenuItemId::new());
    assert!(matches!(
        menus.put(&orphan).await,
        Err(StoreError::Invalid(_))
    ));

    // A parent that lives in another menu is not "in the same menu".
    let other = tree("footer");
    menus.put(&other).await.unwrap();
    let mut cross = tree("main");
    cross.items[2].parent_id = Some(other.items[1].id);
    assert!(matches!(
        menus.put(&cross).await,
        Err(StoreError::Invalid(_))
    ));

    let mut cycle = tree("main");
    let (a, b) = (cycle.items[0].id, cycle.items[1].id);
    cycle.items[0].parent_id = Some(b);
    cycle.items[1].parent_id = Some(a);
    assert!(matches!(
        menus.put(&cycle).await,
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(menus.get("main").await.unwrap().unwrap(), good);
}

/// R11: a failing `put` inside a caller's `Tx` undoes only the replacement; the outer
/// transaction stays usable and commits its other work.
pub async fn menu_put_failure_inside_tx_keeps_tree_and_tx(db: &TestDb) {
    let store = db.store();
    let before = tree("main");
    store.read().menus().put(&before).await.unwrap();

    let tx = store.begin().await.unwrap();
    tx.options().set("before", &json!(1), false).await.unwrap();
    let mut dup = tree("main");
    dup.items[5].id = dup.items[0].id;
    assert!(matches!(
        tx.menus().put(&dup).await,
        Err(StoreError::UniqueViolation { .. })
    ));
    assert_eq!(tx.menus().get("main").await.unwrap().unwrap(), before);
    tx.options().set("after", &json!(2), false).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        store.read().menus().get("main").await.unwrap().unwrap(),
        before
    );
    let options = store.read().options();
    assert_eq!(
        options.get("before").await.unwrap().unwrap().value,
        json!(1)
    );
    assert_eq!(options.get("after").await.unwrap().unwrap().value, json!(2));

    // A successful put inside a Tx is part of it.
    let tx = store.begin().await.unwrap();
    let next = tree("main");
    tx.menus().put(&next).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        store.read().menus().get("main").await.unwrap().unwrap(),
        before
    );
}
