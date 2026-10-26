//! `GET /media?q&kind&trash` and `POST /media/{id}/restore` (M2b-2 Task 2, P33).

use axum::http::Method;
use ferrite_domain::Role;
use serde_json::Value;

use crate::{
    api_tests,
    harness::{Api, Bearer, Part, Resp},
};

api_tests!(media_filters_query_params, media_restore_needs_if_match);

fn png(tag: &str) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(b"\0\0\0\rIHDR");
    v.extend_from_slice(tag.as_bytes());
    v.resize(200, 0);
    v
}

fn pdf(tag: &str) -> Vec<u8> {
    let mut v = b"%PDF-1.7\n%".to_vec();
    v.extend_from_slice(tag.as_bytes());
    v.resize(64, b' ');
    v
}

async fn admin(api: &Api) -> Bearer {
    let u = api.user("admin", Role::Admin, false).await;
    api.bearer(&u, &["*"]).await
}

fn created(r: &Resp) -> Value {
    assert_eq!(r.status.as_u16(), 201, "{r:?}");
    r.json()
}

/// The titles of a `200` list, in order.
async fn titles(api: &Api, who: &Bearer, query: &str) -> Vec<String> {
    let r = who.get(api, &format!("/api/v1/media?{query}")).await;
    assert_eq!(r.status.as_u16(), 200, "{query}: {r:?}");
    r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_owned())
        .collect()
}

fn field(p: &Value) -> &str {
    p["errors"][0]["field"].as_str().unwrap_or_default()
}

async fn media_filters_query_params(api: Api) -> Api {
    let admin_user = api.user("admin", Role::Admin, false).await;
    let admin = api.bearer(&admin_user, &["*"]).await;
    created(
        &admin
            .upload(
                &api,
                &[
                    Part::text("title", "Quokka photo"),
                    Part::file("zebra.png", "image/png", &png("q")),
                ],
            )
            .await,
    );
    created(
        &admin
            .upload(
                &api,
                &[
                    Part::text("title", "Quokka report"),
                    Part::file("report.pdf", "application/pdf", &pdf("r")),
                ],
            )
            .await,
    );
    let old = created(
        &admin
            .upload(&api, &[Part::file("old.pdf", "application/pdf", &pdf("o"))])
            .await,
    );
    let r = admin
        .send(
            &api,
            Method::DELETE,
            &format!("/api/v1/media/{}", old["id"].as_str().unwrap()),
            None,
            old["version"].as_i64(),
        )
        .await;
    assert_eq!(r.status.as_u16(), 204, "{r:?}");

    assert_eq!(
        titles(&api, &admin, "").await,
        ["Quokka report", "Quokka photo"]
    );
    assert_eq!(titles(&api, &admin, "kind=image").await, ["Quokka photo"]);
    assert_eq!(
        titles(&api, &admin, "kind=document").await,
        ["Quokka report"]
    );
    assert!(titles(&api, &admin, "kind=video").await.is_empty());
    assert!(titles(&api, &admin, "kind=audio").await.is_empty());
    assert_eq!(titles(&api, &admin, "q=quokka").await.len(), 2);
    assert_eq!(
        titles(&api, &admin, "q=quokka&kind=image").await,
        ["Quokka photo"]
    );
    // The original file name is not searched.
    assert!(titles(&api, &admin, "q=zebra").await.is_empty());
    assert_eq!(titles(&api, &admin, "trash=true").await, ["old"]);
    assert_eq!(
        titles(&api, &admin, "trash=true&kind=document").await,
        ["old"]
    );
    assert!(
        titles(&api, &admin, "trash=true&kind=image")
            .await
            .is_empty()
    );
    assert_eq!(titles(&api, &admin, "trash=false").await.len(), 2);

    // A search page's cursor and a list's cursor are not interchangeable.
    let r = admin.get(&api, "/api/v1/media?limit=1").await;
    let list_cursor = r.json()["next_cursor"].as_str().unwrap().to_owned();
    let r = admin.get(&api, "/api/v1/media?q=quokka&limit=1").await;
    let search_cursor = r.json()["next_cursor"].as_str().unwrap().to_owned();
    let p = admin
        .get(
            &api,
            &format!("/api/v1/media?q=quokka&cursor={list_cursor}"),
        )
        .await
        .assert_problem(400, "validation");
    assert_eq!(field(&p), "cursor");
    let p = admin
        .get(&api, &format!("/api/v1/media?cursor={search_cursor}"))
        .await
        .assert_problem(400, "validation");
    assert_eq!(field(&p), "cursor");
    let r = admin
        .get(
            &api,
            &format!("/api/v1/media?q=quokka&limit=1&cursor={search_cursor}"),
        )
        .await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(r.json()["items"].as_array().unwrap().len(), 1);

    // Bad values.
    for q in ["kind=nope", "kind=Image", "trash=maybe"] {
        admin
            .get(&api, &format!("/api/v1/media?{q}"))
            .await
            .assert_problem(400, "malformed-query");
    }
    // `q` with `author` or `trash`: 422 naming `q` (F19).
    for q in [
        format!("q=quokka&author={}", admin_user.id),
        "q=quokka&trash=true".to_owned(),
    ] {
        let p = admin
            .get(&api, &format!("/api/v1/media?{q}"))
            .await
            .assert_problem(422, "validation");
        assert_eq!(field(&p), "q", "{q}: {p}");
    }
    // Blank text is no search.
    assert_eq!(titles(&api, &admin, "q=%20&trash=true").await, ["old"]);
    api
}

async fn media_restore_needs_if_match(api: Api) -> Api {
    let admin = admin(&api).await;
    let author = api.user("writer", Role::Author, false).await;
    let writer = api.bearer(&author, &["media:write", "media:read"]).await;
    let contributor = api.user("helper", Role::Contributor, false).await;
    let helper = api
        .bearer(&contributor, &["media:write", "media:read"])
        .await;
    let body = created(
        &writer
            .upload(&api, &[Part::file("back.png", "image/png", &png("back"))])
            .await,
    );
    let id = body["id"].as_str().unwrap().to_owned();
    let url = body["url"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/media/{id}");
    let restore = format!("{path}/restore");
    let v1 = body["version"].as_i64().unwrap();
    assert_eq!(api.get(&url).await.status.as_u16(), 200);

    // Not trashed yet: 422, as the content route answers.
    writer
        .send(&api, Method::POST, &restore, None, Some(v1))
        .await
        .assert_problem(422, "validation");
    let r = writer
        .send(&api, Method::DELETE, &path, None, Some(v1))
        .await;
    assert_eq!(r.status.as_u16(), 204, "{r:?}");
    assert_eq!(api.get(&url).await.status.as_u16(), 404);
    let v2 = writer.get(&api, &path).await.json()["version"]
        .as_i64()
        .unwrap();

    writer
        .send(&api, Method::POST, &restore, None, None)
        .await
        .assert_problem(428, "precondition-required");
    let p = writer
        .send(&api, Method::POST, &restore, None, Some(v1))
        .await
        .assert_problem(412, "precondition-failed");
    assert_eq!(p["current"]["status"], "trash", "{p}");
    // R8: contributors cannot restore.
    helper
        .send(&api, Method::POST, &restore, None, Some(v2))
        .await
        .assert_problem(403, "forbidden");
    // A token without `media:write` cannot either.
    let reader = api.bearer(&author, &["media:read"]).await;
    reader
        .send(&api, Method::POST, &restore, None, Some(v2))
        .await
        .assert_problem(403, "forbidden");

    let r = writer
        .send(&api, Method::POST, &restore, None, Some(v2))
        .await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    let restored = r.json();
    let v3 = restored["version"].as_i64().unwrap();
    assert!(v3 > v2, "{restored}");
    assert_eq!(r.header("etag"), Some(format!("\"{v3}\"").as_str()));
    assert_eq!(restored["status"], "published");
    assert_eq!(restored["id"], body["id"]);
    assert_eq!(restored["url"], body["url"]);
    // Served and listed again.
    assert_eq!(api.get(&url).await.status.as_u16(), 200);
    assert_eq!(titles(&api, &admin, "").await, ["back"]);
    assert!(titles(&api, &admin, "trash=true").await.is_empty());
    // An unknown id is 404.
    admin
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/media/{}/restore", uuid::Uuid::now_v7()),
            None,
            Some(1),
        )
        .await
        .assert_problem(404, "not-found");
    api
}
