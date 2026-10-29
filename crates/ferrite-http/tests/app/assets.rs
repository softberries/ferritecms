//! `/_media` (Review Focus 5, the media rule) and `/_theme` assets.

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use bytes::Bytes;
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, Media, NewContent, Timestamp,
};
use serde_json::{Map, json};

use crate::app_tests;
use crate::common::HOUR;
use crate::harness::{App, request};

/// A PNG signature followed by filler: sniffed as `image/png`, 1000 bytes.
fn png() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend((0..992u32).map(|i| (i % 251) as u8));
    v
}

pub struct Uploaded {
    pub item: ContentId,
    pub hash: ArtifactHash,
    pub variants: Vec<ArtifactHash>,
}

async fn artifact(app: &App, data: &[u8], kind: ArtifactKind) -> (ArtifactHash, String) {
    let r = app
        .site
        .media
        .put_artifact(Bytes::copy_from_slice(data))
        .await
        .unwrap();
    app.site
        .store
        .read()
        .artifacts()
        .insert_if_absent(&Artifact {
            hash: r.hash.clone(),
            kind,
            bytes: i64::try_from(r.bytes).unwrap(),
            mime: r.mime.clone(),
            created_at: Timestamp::now(),
        })
        .await
        .unwrap();
    (r.hash, r.mime)
}

/// A media item for `data` named `name` in `status` (published an hour ago, or in an
/// hour when `future`), with one variant per entry of `variants`.
pub async fn upload(
    app: &App,
    data: &[u8],
    name: &str,
    status: ContentStatus,
    future: bool,
    variants: &[(&str, &[u8])],
) -> Uploaded {
    upload_row(app, data, name, status, future, variants, None).await
}

/// A published media item whose row records `mime` and the sanitiser marker as given
/// (written through the store, whatever the bytes sniff as).
async fn upload_as(app: &App, data: &[u8], name: &str, mime: &str, sanitized: bool) -> Uploaded {
    upload_row(
        app,
        data,
        name,
        ContentStatus::Published,
        false,
        &[],
        Some((mime, sanitized)),
    )
    .await
}

async fn upload_row(
    app: &App,
    data: &[u8],
    name: &str,
    status: ContentStatus,
    future: bool,
    variants: &[(&str, &[u8])],
    typed: Option<(&str, bool)>,
) -> Uploaded {
    let (hash, sniffed) = artifact(app, data, ArtifactKind::Media).await;
    let (mime, sanitized) = typed.map_or((sniffed, false), |(m, s)| (m.to_owned(), s));
    let mut vjson = Map::new();
    let mut vhashes = Vec::new();
    for (vname, vdata) in variants {
        let (h, m) = artifact(app, vdata, ArtifactKind::Variant).await;
        vjson.insert(
            (*vname).to_owned(),
            json!({"hash": h.as_str(), "w": 10, "h": 10, "mime": m}),
        );
        vhashes.push(h);
    }
    let item = ContentId::new();
    let now = Timestamp::now().as_micros();
    let offset = if future { HOUR } else { -HOUR };
    let tx = app.site.store.begin().await.unwrap();
    tx.content()
        .insert(&NewContent {
            id: item,
            kind: "media".into(),
            status,
            slug: format!("m-{item}"),
            parent_id: None,
            author_id: app.ada_user.id,
            title: name.into(),
            excerpt: String::new(),
            body: json!({"type": "doc", "content": []}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: Some(Timestamp::from_micros(now + offset).unwrap()),
            scheduled_at: None,
            created_at: None,
            updated_at: None,
        })
        .await
        .unwrap();
    tx.media()
        .insert(&Media {
            item_id: item,
            artifact_hash: hash.clone(),
            mime,
            bytes: i64::try_from(data.len()).unwrap(),
            width: None,
            height: None,
            alt: String::new(),
            original_name: name.into(),
            variants: vjson.into(),
            sanitized,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Uploaded {
        item,
        hash,
        variants: vhashes,
    }
}

fn get_with(path: &str, headers: &[(header::HeaderName, &str)]) -> Request<Body> {
    let mut req = request(Method::GET, path);
    for (name, value) in headers {
        req.headers_mut()
            .insert(name.clone(), value.parse().unwrap());
    }
    req
}

/// Review Focus 5 and the media headers.
async fn media_ranges_and_headers(app: App) -> App {
    let data = png();
    let up = upload(&app, &data, "cat.png", ContentStatus::Published, false, &[]).await;
    let url = format!("/_media/{}/cat.png", up.hash);
    let etag = format!("\"{}\"", up.hash);

    let r = app
        .send(get_with(&url, &[(header::ACCEPT_ENCODING, "gzip, br")]))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.raw.as_slice().len(), 1000);
    assert_eq!(r.header("content-length"), Some("1000"));
    assert_eq!(r.header("content-type"), Some("image/png"));
    assert_eq!(r.header("content-disposition"), Some("inline"));
    assert_eq!(r.header("accept-ranges"), Some("bytes"));
    assert_eq!(r.header("etag"), Some(etag.as_str()));
    assert_eq!(
        r.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        r.header("content-encoding"),
        None,
        "media is never compressed"
    );
    assert_eq!(r.header("content-security-policy"), None);

    let part = |range: &str| get_with(&url, &[(header::RANGE, range)]);
    for (range, want, content_range) in [
        ("bytes=0-0", 0..1, "bytes 0-0/1000"),
        ("bytes=10-19", 10..20, "bytes 10-19/1000"),
        ("bytes=990-", 990..1000, "bytes 990-999/1000"),
        ("bytes=-100", 900..1000, "bytes 900-999/1000"),
        ("bytes=-5000", 0..1000, "bytes 0-999/1000"),
        ("bytes=995-99999", 995..1000, "bytes 995-999/1000"),
    ] {
        let r = app.send(part(range)).await;
        assert_eq!(r.status, StatusCode::PARTIAL_CONTENT, "{range}");
        assert_eq!(r.header("content-range"), Some(content_range), "{range}");
        let len = (want.end - want.start).to_string();
        assert_eq!(r.header("content-length"), Some(len.as_str()), "{range}");
        assert_eq!(r.raw.as_slice(), &data[want.clone()], "{range}: bytes");
    }
    // Many ranges: one full 200, no multipart.
    let many: Vec<String> = (0..200).map(|i| format!("{i}-{i}")).collect();
    let r = app.send(part(&format!("bytes={}", many.join(",")))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.raw.as_slice(), data.as_slice());
    assert_eq!(r.header("content-range"), None);
    for range in ["bytes=-0", "bytes=99999999-", "bytes=1000-"] {
        let r = app.send(part(range)).await;
        assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE, "{range}");
        assert_eq!(r.header("content-range"), Some("bytes */1000"), "{range}");
        assert!(r.body.is_empty());
    }
    // Ignored ranges: the full body.
    for range in ["bytes=5-1", "items=0-1", "bytes=x-"] {
        let r = app.send(part(range)).await;
        assert_eq!(r.status, StatusCode::OK, "{range}");
        assert_eq!(r.raw.as_slice().len(), 1000);
    }
    // If-Range: strong match applies the range, anything else gets the full body.
    let r = app
        .send(get_with(
            &url,
            &[(header::RANGE, "bytes=0-9"), (header::IF_RANGE, &etag)],
        ))
        .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    for if_range in ["\"other\"", "W/\"x\"", "Tue, 14 Nov 2023 22:13:20 GMT"] {
        let r = app
            .send(get_with(
                &url,
                &[(header::RANGE, "bytes=0-9"), (header::IF_RANGE, if_range)],
            ))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{if_range}");
        assert_eq!(r.raw.as_slice().len(), 1000);
    }
    // Conditional GET.
    let r = app
        .send(get_with(&url, &[(header::IF_NONE_MATCH, &etag)]))
        .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    assert!(r.body.is_empty());
    assert_eq!(r.header("etag"), Some(etag.as_str()));
    // HEAD: the GET headers, no body. HEAD ignores Range (RFC 9110 §14.2): still the
    // full-size 200 headers.
    let r = app.send(request(Method::HEAD, &url)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-length"), Some("1000"));
    assert!(r.body.is_empty());
    let mut head = request(Method::HEAD, &url);
    head.headers_mut()
        .insert(header::RANGE, "bytes=-10".parse().unwrap());
    let r = app.send(head).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-length"), Some("1000"));
    assert_eq!(r.header("content-range"), None);
    assert!(r.body.is_empty());
    // Range units are case-insensitive; a huge start is unsatisfiable, a huge end the
    // whole file.
    let r = app.send(part("Bytes=0-0")).await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    let r = app.send(part("bytes=99999999999999999999999-")).await;
    assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE);
    let r = app.send(part("bytes=0-99999999999999999999999")).await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.header("content-range"), Some("bytes 0-999/1000"));
    // Another name: 301 to the canonical one.
    let r = app.get(&format!("/_media/{}/dog.png", up.hash)).await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(r.header("location"), Some(url.as_str()));
    // Never in the page cache.
    assert_eq!(app.cached().await, 0);
    app
}

/// Only files of public media items are served: originals and variants.
async fn media_rule_published_only(app: App) -> App {
    let thumb: Vec<u8> = png().into_iter().rev().collect();
    let public = upload(
        &app,
        &png(),
        "pub.png",
        ContentStatus::Published,
        false,
        &[("thumb", thumb.as_slice())],
    )
    .await;
    let mut other = png();
    other[100] ^= 0xFF;
    let draft = upload(&app, &other, "draft.png", ContentStatus::Draft, false, &[]).await;
    let mut later = png();
    later[200] ^= 0xFF;
    let future = upload(
        &app,
        &later,
        "later.png",
        ContentStatus::Published,
        true,
        &[],
    )
    .await;
    let mut trashed_bytes = png();
    trashed_bytes[300] ^= 0xFF;
    let trashed = upload(
        &app,
        &trashed_bytes,
        "t.png",
        ContentStatus::Trash,
        false,
        &[],
    )
    .await;

    let r = app
        .get(&format!("/_media/{}/pub.png", public.variants[0]))
        .await;
    assert_eq!(r.status, StatusCode::OK, "variant served");
    assert_eq!(r.raw.as_slice(), thumb.as_slice());
    for (hash, name) in [
        (&draft.hash, "draft.png"),
        (&future.hash, "later.png"),
        (&trashed.hash, "t.png"),
    ] {
        let r = app.get(&format!("/_media/{hash}/{name}")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{name}");
        assert_eq!(r.body, "not found\n");
        assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    }
    // A stored artifact that is not media (a plugin), and an unknown hash.
    app.site.install("hello").await;
    let plugin = app
        .site
        .store
        .read()
        .plugins()
        .list()
        .await
        .unwrap()
        .pop()
        .unwrap();
    for hash in [plugin.artifact_hash.to_string(), "e".repeat(64)] {
        let r = app.get(&format!("/_media/{hash}/x.wasm")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{hash}");
    }
    // Publishing the draft makes its file public (no cache involved).
    let item = app.svc.get(draft.item).await.unwrap();
    app.svc
        .publish(&app.admin, item.id, item.version)
        .await
        .unwrap();
    let r = app.get(&format!("/_media/{}/draft.png", draft.hash)).await;
    assert_eq!(r.status, StatusCode::OK);
    app
}

/// Content types: inline-safe types inline; PDF sandboxed; the rest attachments.
async fn media_dispositions(app: App) -> App {
    let html = b"<!doctype html><html><body><script>alert(1)</script></body></html>";
    let up = upload(
        &app,
        html,
        "Ünï page.html",
        ContentStatus::Published,
        false,
        &[],
    )
    .await;
    let r = app
        .get(&format!("/_media/{}/%C3%9Cn%C3%AF%20page.html", up.hash))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/octet-stream"));
    assert_eq!(
        r.header("content-disposition"),
        Some(
            "attachment; filename=\"_n_ page.html\"; filename*=UTF-8''%C3%9Cn%C3%AF%20page%2Ehtml"
        )
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    // Not compressed, even for a compressible-looking type.
    let r = app
        .send(get_with(
            &format!("/_media/{}/%C3%9Cn%C3%AF%20page.html", up.hash),
            &[(header::ACCEPT_ENCODING, "gzip")],
        ))
        .await;
    assert_eq!(r.header("content-encoding"), None);
    assert_eq!(r.raw.as_slice(), html.as_slice());

    let svg = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
    let up = upload(&app, svg, "a.svg", ContentStatus::Published, false, &[]).await;
    let r = app.get(&format!("/_media/{}/a.svg", up.hash)).await;
    assert_eq!(
        r.header("content-type"),
        Some("application/octet-stream"),
        "unsanitised SVG"
    );
    assert!(
        r.header("content-disposition")
            .unwrap()
            .starts_with("attachment")
    );

    let pdf = b"%PDF-1.4\n1 0 obj << >> endobj\ntrailer << >>\n%%EOF\n";
    let up = upload(&app, pdf, "doc.pdf", ContentStatus::Published, false, &[]).await;
    let r = app.get(&format!("/_media/{}/doc.pdf", up.hash)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("application/pdf"));
    assert_eq!(r.header("content-disposition"), Some("inline"));
    assert_eq!(r.header("content-security-policy"), Some("sandbox"));
    app
}

/// The exact policy of an inline SVG (Review Focus 1, P7).
const SVG_CSP: &str = "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'";

/// Review Focus 1: a row marked sanitised is served inline as `image/svg+xml` under
/// the exact `sandbox` policy, never sniffed. HEAD and a range carry the same headers.
async fn sanitised_svg_is_inline_with_sandbox(app: App) -> App {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>"#;
    let up = upload_as(&app, svg, "logo.svg", "image/svg+xml", true).await;
    let url = format!("/_media/{}/logo.svg", up.hash);
    let r = app.get(&url).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("image/svg+xml"));
    assert_eq!(r.header("content-disposition"), Some("inline"));
    assert_eq!(r.header("content-security-policy"), Some(SVG_CSP));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.raw.as_slice(), svg.as_slice());
    let head = app.send(request(Method::HEAD, &url)).await;
    assert_eq!(head.header("content-security-policy"), Some(SVG_CSP));
    assert_eq!(head.header("content-type"), Some("image/svg+xml"));
    let part = app
        .send(get_with(&url, &[(header::RANGE, "bytes=0-3")]))
        .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.header("content-security-policy"), Some(SVG_CSP));
    assert_eq!(part.header("content-disposition"), Some("inline"));
    app
}

/// Review Focus 1: an SVG row without the marker (written through the store, as an
/// older binary or a hand edit would) is a download, whatever its recorded type.
async fn unmarked_svg_stays_an_attachment(app: App) -> App {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
    let up = upload_as(&app, svg, "raw.svg", "image/svg+xml", false).await;
    let r = app.get(&format!("/_media/{}/raw.svg", up.hash)).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/octet-stream"));
    assert_eq!(
        r.header("content-disposition"),
        Some("attachment; filename=\"raw.svg\"; filename*=UTF-8''raw%2Esvg")
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("content-security-policy"), None);
    // The marker on another type does not make it inline either.
    let html = b"<!doctype html><script>alert(1)</script>";
    let up = upload_as(&app, html, "page.html", "text/html", true).await;
    let r = app.get(&format!("/_media/{}/page.html", up.hash)).await;
    assert_eq!(r.header("content-type"), Some("application/octet-stream"));
    assert!(
        r.header("content-disposition")
            .unwrap()
            .starts_with("attachment;")
    );
    app
}

/// Every media class's headers (R1, F14, P7): inline-safe images, audio and video
/// inline with their type and `nosniff`, no policy; PDF inline under `sandbox`;
/// sanitised SVG under its policy; documents, unmarked SVG and anything else as
/// `application/octet-stream` attachments. Every response carries `nosniff`.
async fn every_media_class_has_its_headers(app: App) -> App {
    let inline = [
        "image/jpeg",
        "image/png",
        "image/gif",
        "image/webp",
        "audio/mpeg",
        "audio/ogg",
        "audio/wav",
        "audio/x-wav",
        "video/mp4",
        "video/webm",
    ];
    let attachments = [
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "application/vnd.oasis.opendocument.text",
        "application/vnd.oasis.opendocument.spreadsheet",
        "application/vnd.oasis.opendocument.presentation",
        "application/zip",
        "text/plain",
        "text/csv",
        "text/html",
        "application/octet-stream",
    ];
    let mut cases: Vec<(&str, bool, Option<&str>, &str)> = Vec::new();
    for mime in inline {
        cases.push((mime, false, None, mime));
    }
    cases.push(("application/pdf", false, Some("sandbox"), "application/pdf"));
    cases.push(("image/svg+xml", true, Some(SVG_CSP), "image/svg+xml"));
    for mime in attachments {
        cases.push((mime, false, None, "application/octet-stream"));
    }
    cases.push(("image/svg+xml", false, None, "application/octet-stream"));
    for (i, (mime, sanitized, csp, served)) in cases.into_iter().enumerate() {
        // Distinct bytes per case: one artifact per row.
        let data = format!("case {i} {mime}");
        let up = upload_as(&app, data.as_bytes(), "f.bin", mime, sanitized).await;
        let r = app.get(&format!("/_media/{}/f.bin", up.hash)).await;
        assert_eq!(r.status, StatusCode::OK, "{mime}: {r:?}");
        assert_eq!(r.header("content-type"), Some(served), "{mime}");
        assert_eq!(
            r.header("x-content-type-options"),
            Some("nosniff"),
            "{mime}"
        );
        assert_eq!(r.header("content-security-policy"), csp, "{mime}");
        let disposition = r.header("content-disposition").unwrap();
        if served == "application/octet-stream" {
            assert!(
                disposition.starts_with("attachment;"),
                "{mime}: {disposition}"
            );
        } else {
            assert_eq!(disposition, "inline", "{mime}");
        }
    }
    app
}

/// Variant URLs (review M3): a raster variant is served inline with its recorded type
/// and `nosniff`, no policy. A variant entry without a `mime` falls back to the
/// original's type; on a sanitised SVG item that is `image/svg+xml`, which is still an
/// attachment (variants never carry the sanitiser's marker).
async fn variant_headers_and_svg_fallback(app: App) -> App {
    let thumb = {
        let mut v = png();
        v.push(1);
        v
    };
    let up = upload(
        &app,
        &png(),
        "pic.png",
        ContentStatus::Published,
        false,
        &[("thumbnail", &thumb)],
    )
    .await;
    let r = app
        .get(&format!("/_media/{}/pic.png", up.variants[0]))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("image/png"));
    assert_eq!(r.header("content-disposition"), Some("inline"));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("content-security-policy"), None);

    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"/>"#;
    let up = upload_as(&app, svg, "icon.svg", "image/svg+xml", true).await;
    let other = br#"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="3"/>"#;
    let (vhash, _) = artifact(&app, other, ArtifactKind::Variant).await;
    let tx = app.site.store.begin().await.unwrap();
    let mut row = tx.media().get_for_update(up.item).await.unwrap().unwrap();
    row.variants = json!({"thumbnail": {"hash": vhash.as_str(), "w": 3, "h": 3}});
    tx.media().update(&row).await.unwrap();
    tx.commit().await.unwrap();
    let r = app.get(&format!("/_media/{vhash}/icon.svg")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/octet-stream"));
    assert!(
        r.header("content-disposition")
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("content-security-policy"), None);
    // The original itself stays inline under the SVG policy.
    let r = app.get(&format!("/_media/{}/icon.svg", up.hash)).await;
    assert_eq!(r.header("content-security-policy"), Some(SVG_CSP));
    app
}

async fn theme_assets(app: App) -> App {
    let hash = app.state.theme.load().hash().to_owned();
    let url = format!("/_theme/{hash}/css/site.css");
    let r = app.get(&url).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("text/css; charset=utf-8"));
    assert_eq!(
        r.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert!(r.body.contains("color: #222"));
    let etag = r.header("etag").unwrap().to_owned();
    // Weak for compressible text, strong for binaries.
    assert!(etag.starts_with(&format!("W/\"{hash}-")), "{etag}");
    let r = app
        .send(get_with(&url, &[(header::IF_NONE_MATCH, &etag)]))
        .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    // Text compresses, fonts do not.
    let gz = [(header::ACCEPT_ENCODING, "gzip")];
    let r = app
        .send(get_with(&format!("/_theme/{hash}/css/big.css"), &gz))
        .await;
    assert_eq!(r.header("content-encoding"), Some("gzip"));
    let r = app
        .send(get_with(&format!("/_theme/{hash}/fonts/x.woff2"), &gz))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("font/woff2"));
    assert_eq!(r.header("content-encoding"), None);
    let font_etag = r.header("etag").unwrap();
    assert!(font_etag.starts_with(&format!("\"{hash}-")), "{font_etag}");
    let r = app
        .send(get_with(&format!("/_theme/{hash}/css/big.css"), &gz))
        .await;
    let big_etag = r.header("etag").unwrap().to_owned();
    assert!(big_etag.starts_with("W/"), "compressed: {big_etag}");
    let mut req = get_with(&format!("/_theme/{hash}/css/big.css"), &gz);
    req.headers_mut()
        .insert(header::IF_NONE_MATCH, big_etag.parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::NOT_MODIFIED);

    // Wrong hash, templates, the manifest, traversal: 404.
    for path in [
        format!("/_theme/{}/css/site.css", "0".repeat(16)),
        format!("/_theme/{hash}/index.html"),
        format!("/_theme/{hash}/templates/index.html"),
        format!("/_theme/{hash}/../theme.toml"),
        format!("/_theme/{hash}/theme.toml"),
        format!("/_theme/{hash}/css/%2e%2e/theme.toml"),
        format!("/_theme/{hash}/css/missing.css"),
    ] {
        let r = app.get(&path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
    // Asset URLs never enter the page cache; the one entry is the shared 404 body of
    // the paths the router itself rejects (`..`).
    assert_eq!(app.cached().await, 1);
    let key = ferrite_cache::CacheKey::anon("example.com", url.as_str(), "");
    assert!(app.state.cache.get(&key).await.is_none());
    app
}

/// Stored names are routed through the shared sanitiser (`/`, `\`, controls, empty
/// and dot names), the same one `MediaStore::public_url` uses.
async fn media_names_are_sanitised(app: App) -> App {
    let mut a = png();
    a[50] ^= 1;
    let up = upload(
        &app,
        &a,
        "dir/sub\\x\u{1}.png",
        ContentStatus::Published,
        false,
        &[],
    )
    .await;
    let canonical = app.site.media.public_url(&up.hash, "dir/sub\\x\u{1}.png");
    assert_eq!(canonical, format!("/_media/{}/dir_sub_x_.png", up.hash));
    assert_eq!(app.get(&canonical).await.status, StatusCode::OK);
    let mut b = png();
    b[60] ^= 1;
    let up = upload(&app, &b, "..", ContentStatus::Published, false, &[]).await;
    let canonical = app.site.media.public_url(&up.hash, "..");
    assert_eq!(canonical, format!("/_media/{}/file", up.hash));
    assert_eq!(app.get(&canonical).await.status, StatusCode::OK);
    let r = app.get(&format!("/_media/{}/other", up.hash)).await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(r.header("location"), Some(canonical.as_str()));
    app
}

/// After a hot swap the replaced theme's assets stay served (pages rendered just before
/// the swap link to them); after the next swap they are gone (P18).
async fn previous_theme_assets_survive_one_swap(app: App) -> App {
    let old = app.state.theme.load().hash().to_owned();
    let url = format!("/_theme/{old}/css/site.css");
    assert_eq!(app.get(&url).await.status, StatusCode::OK);
    let blog = std::sync::Arc::new(ferrite_theme::Theme::load_embedded("blog").unwrap());
    assert_ne!(blog.hash(), old);
    app.state.swap_theme(blog.clone());
    assert_eq!(app.state.theme.load().hash(), blog.hash());
    assert_eq!(app.get(&url).await.status, StatusCode::OK, "previous theme");
    app.state.swap_theme(blog);
    assert_eq!(
        app.get(&url).await.status,
        StatusCode::NOT_FOUND,
        "two swaps ago"
    );
    app
}

app_tests!(
    previous_theme_assets_survive_one_swap,
    media_names_are_sanitised,
    media_ranges_and_headers,
    media_rule_published_only,
    media_dispositions,
    sanitised_svg_is_inline_with_sandbox,
    unmarked_svg_stays_an_attachment,
    every_media_class_has_its_headers,
    variant_headers_and_svg_fallback,
    theme_assets,
);
