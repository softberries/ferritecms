//! `/media` (M2a-2b Task 4): streamed uploads whose type is decided from their content
//! (P4, R1/F13), SVG sanitising (P7), the size limit and the node's upload slots (P9),
//! and the item endpoints (`If-Match`, trash, delete).

use std::time::Duration;

use axum::http::{Method, header};
use ferrite_domain::Role;
use ferrite_http::media::{MEDIA_UPLOAD_SLOTS, Uploader};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{
        Api, Bearer, MAX_UPLOAD, Part, Resp, body_end, broken_body, chunked_body, gated_body,
        part_head, send, upload_body, upload_content_type,
    },
};

api_tests!(
    upload_png_flow,
    upload_svg_is_sanitised,
    upload_over_limit_is_413_and_records_nothing,
    type_confusion_is_refused,
    document_classes_are_download_only,
    contributor_upload_is_403_before_reading,
    upload_slots_bound_a_node,
    media_list_pages,
    upload_parts_are_checked,
    aborted_uploads_leave_nothing_staged,
    idle_upload_is_aborted_and_frees_its_slot,
);

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const ZIP_MAGIC: &[u8] = b"PK\x03\x04";

/// `len` bytes that sniff as PNG, with `tag` after the magic (distinct artifacts).
fn png(len: usize, tag: &str) -> Vec<u8> {
    let mut v = PNG_MAGIC.to_vec();
    v.extend_from_slice(b"\0\0\0\rIHDR");
    v.extend_from_slice(tag.as_bytes());
    v.resize(len.max(v.len()), 0);
    v
}

/// `len` bytes that sniff as PDF.
fn pdf(len: usize, tag: &str) -> Vec<u8> {
    let mut v = b"%PDF-1.7\n%".to_vec();
    v.extend_from_slice(tag.as_bytes());
    v.resize(len.max(v.len()), b' ');
    v
}

/// A ZIP local-file header followed by `rest`.
fn zip(rest: &[u8]) -> Vec<u8> {
    let mut v = ZIP_MAGIC.to_vec();
    v.extend_from_slice(b"\x14\x00\x00\x00\x08\x00");
    v.extend_from_slice(rest);
    v
}

async fn admin(api: &Api) -> Bearer {
    let u = api.user("admin", Role::Admin, false).await;
    api.bearer(&u, &["*"]).await
}

async fn job_count(api: &Api) -> u64 {
    api.store
        .read()
        .jobs()
        .counts()
        .await
        .unwrap()
        .values()
        .sum()
}

/// Media items listed (every status but trash).
async fn media_count(api: &Api, who: &Bearer) -> usize {
    let r = who.get(api, "/api/v1/media").await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    r.json()["items"].as_array().unwrap().len()
}

/// A `201` upload's body.
fn created(r: &Resp) -> Value {
    assert_eq!(r.status.as_u16(), 201, "{r:?}");
    r.json()
}

/// `errors[0].field` of a problem.
fn field(p: &Value) -> &str {
    p["errors"][0]["field"].as_str().unwrap_or_default()
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn upload_png_flow(api: Api) -> Api {
    let admin = admin(&api).await;
    let bytes = png(1000, "flow");
    let r = admin
        .upload(
            &api,
            &[
                Part::text("title", "Sunset"),
                Part::file("sunset.png", "image/png", &bytes),
                Part::text("alt", "A red sky"),
            ],
        )
        .await;
    let body = created(&r);
    let id = body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/media/{id}");
    assert_eq!(r.header("location"), Some(path.as_str()));
    let version = body["version"].as_i64().unwrap();
    assert_eq!(r.header("etag"), Some(format!("\"{version}\"").as_str()));
    assert_eq!(body["mime"], "image/png");
    assert_eq!(body["sanitized"], false);
    assert_eq!(body["bytes"], 1000);
    assert_eq!(body["title"], "Sunset");
    assert_eq!(body["alt"], "A red sky");
    assert_eq!(body["original_name"], "sunset.png");
    assert_eq!(body["status"], "published");
    assert_eq!(body["variants"], json!([]));
    assert!(
        body["width"].is_null() && body["height"].is_null(),
        "{body}"
    );
    let url = body["url"].as_str().unwrap().to_owned();
    assert!(
        url.starts_with("/_media/") && url.ends_with("/sunset.png"),
        "{url}"
    );
    // A raster upload gets its variants job.
    assert_eq!(job_count(&api).await, 1);

    let file = api.get(&url).await;
    assert_eq!(file.status.as_u16(), 200, "{file:?}");
    assert_eq!(file.header("content-type"), Some("image/png"));
    assert_eq!(file.header("x-content-type-options"), Some("nosniff"));

    let r = admin.get(&api, &path).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(r.header("etag"), Some(format!("\"{version}\"").as_str()));
    assert_eq!(r.json(), body);

    let patch = json!({ "alt": "Dusk" });
    let r = admin
        .send(&api, Method::PATCH, &path, Some(&patch), None)
        .await;
    r.assert_problem(428, "precondition-required");
    let r = admin
        .send(&api, Method::PATCH, &path, Some(&patch), Some(version))
        .await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    let patched = r.json();
    assert_eq!(patched["alt"], "Dusk");
    assert_eq!(patched["title"], "Sunset");
    let v2 = patched["version"].as_i64().unwrap();
    assert!(v2 > version, "{patched}");
    assert_eq!(r.header("etag"), Some(format!("\"{v2}\"").as_str()));
    // A stale version is refused with the item as stored now.
    let r = admin
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({ "alt": "x" })),
            Some(version),
        )
        .await;
    let p = r.assert_problem(412, "precondition-failed");
    assert_eq!(p["current"]["alt"], "Dusk", "{p}");
    // An over-long alt text names the field.
    let r = admin
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({ "alt": "a".repeat(1001) })),
            Some(v2),
        )
        .await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "alt");

    let r = admin.send(&api, Method::DELETE, &path, None, None).await;
    r.assert_problem(428, "precondition-required");
    let r = admin
        .send(&api, Method::DELETE, &path, None, Some(v2))
        .await;
    assert_eq!(r.status.as_u16(), 204, "{r:?}");
    let r = admin.get(&api, &path).await;
    assert_eq!(r.json()["status"], "trash", "{r:?}");
    let v3 = r.json()["version"].as_i64().unwrap();
    // A trashed item is no longer served or listed.
    assert_eq!(api.get(&url).await.status.as_u16(), 404);
    assert_eq!(media_count(&api, &admin).await, 0);
    let force = format!("{path}?force=true");
    let r = admin
        .send(&api, Method::DELETE, &force, None, Some(v3))
        .await;
    assert_eq!(r.status.as_u16(), 204, "{r:?}");
    admin
        .get(&api, &path)
        .await
        .assert_problem(404, "not-found");
    admin
        .get(&api, "/api/v1/media/not-a-uuid")
        .await
        .assert_problem(404, "not-found");
    api
}

async fn upload_svg_is_sanitised(api: Api) -> Api {
    let admin = admin(&api).await;
    // The claimed type and name do not matter: the content is an SVG (Task 3 F13 table).
    for (name, claimed, size) in [
        ("logo.svg", "image/svg+xml", 10),
        ("logo.png", "image/png", 20),
    ] {
        let svg = format!(
            r#"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}">
  <script>alert(1)</script>
  <rect width="10" height="10" fill="red" onclick="alert(2)"/>
</svg>"#
        );
        let body = created(
            &admin
                .upload(&api, &[Part::file(name, claimed, svg.as_bytes())])
                .await,
        );
        assert_eq!(body["mime"], "image/svg+xml", "{name}");
        assert_eq!(body["sanitized"], true, "{name}");
        let stored = api.get(body["url"].as_str().unwrap()).await;
        assert_eq!(stored.status.as_u16(), 200, "{stored:?}");
        assert!(!stored.body.contains("script"), "{}", stored.body);
        assert!(!stored.body.contains("onclick"), "{}", stored.body);
        assert!(stored.body.contains("<rect"), "{}", stored.body);
        assert_eq!(body["bytes"], stored.body.len(), "{name}");
        // Uploaded and marked: inline under the SVG policy (Task 6, P7).
        assert_eq!(stored.header("content-type"), Some("image/svg+xml"));
        assert_eq!(stored.header("content-disposition"), Some("inline"));
        assert_eq!(
            stored.header("content-security-policy"),
            Some("sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'")
        );
        assert_eq!(stored.header("x-content-type-options"), Some("nosniff"));
    }
    // SVGs get no variants job.
    assert_eq!(job_count(&api).await, 0);

    // Refusals are fixed text: an unknown entity's name is never echoed (Task 2 carry).
    let bad = br#"<svg xmlns="http://www.w3.org/2000/svg"><text>&leakyentityname;</text></svg>"#;
    let r = admin
        .upload(&api, &[Part::file("bad.svg", "image/svg+xml", bad)])
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(field(&p), "file");
    assert!(
        p["detail"]
            .as_str()
            .unwrap()
            .starts_with("the SVG could not be made safe: "),
        "{p}"
    );
    assert!(!r.body.contains("leakyentityname"), "{}", r.body);
    let broken = b"<svg xmlns=\"http://www.w3.org/2000/svg\"><g secret-attr-name=</svg>";
    let r = admin
        .upload(&api, &[Part::file("broken.svg", "image/svg+xml", broken)])
        .await;
    let p = r.assert_problem(422, "validation");
    assert!(!r.body.contains("secret-attr-name"), "{p}");
    // An XML document that is not an SVG.
    let html = b"<?xml version=\"1.0\"?><html><body><svg/></body></html>";
    let r = admin
        .upload(&api, &[Part::file("page.svg", "image/svg+xml", html)])
        .await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "file");
    assert_eq!(media_count(&api, &admin).await, 2);

    // An SVG over 2 MiB is refused while it is read, even when the upload limit is larger.
    let (router, _) = api.router_with_max_upload(4 << 20);
    let mut big = b"<svg xmlns=\"http://www.w3.org/2000/svg\">".to_vec();
    big.resize((2 << 20) + 1, b' ');
    big.extend_from_slice(b"</svg>");
    let r = send(
        &router,
        admin.upload_request(&[Part::file("big.svg", "image/svg+xml", &big)]),
    )
    .await;
    r.assert_problem(413, "too-large");
    // Over `max_upload` (1 MiB here) it is the upload limit that refuses it.
    let r = admin
        .upload(&api, &[Part::file("big.svg", "image/svg+xml", &big)])
        .await;
    r.assert_problem(413, "too-large");
    assert_eq!(media_count(&api, &admin).await, 2);
    api
}

async fn upload_over_limit_is_413_and_records_nothing(api: Api) -> Api {
    let admin = admin(&api).await;
    let over = png(MAX_UPLOAD as usize + 1, "over");
    let r = admin
        .upload(&api, &[Part::file("big.png", "image/png", &over)])
        .await;
    let p = r.assert_problem(413, "too-large");
    assert!(
        p["detail"]
            .as_str()
            .unwrap()
            .contains(&MAX_UPLOAD.to_string()),
        "{p}"
    );
    // Streamed without a declared length: cut the same way.
    let (content_type, body) = upload_body(&[Part::file("big.png", "image/png", &over)]);
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, &content_type)
        .body(chunked_body(body))
        .unwrap();
    api.send(req).await.assert_problem(413, "too-large");
    assert_eq!(media_count(&api, &admin).await, 0);
    assert_eq!(job_count(&api).await, 0);
    assert!(api.stored_files().is_empty(), "{:?}", api.stored_files());
    let started = api.api.uploader.started();

    // A declared length over the route limit (`max_upload` + 1 MiB) is refused before
    // any of the body is read: the gate is never opened.
    let (body, _gate) = gated_body(
        part_head(&Part::file("big.png", "image/png", &[])),
        over.clone(),
    );
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, &content_type)
        .header(
            header::CONTENT_LENGTH,
            (MAX_UPLOAD + (1 << 20) + 1).to_string(),
        )
        .body(body)
        .unwrap();
    let r = tokio::time::timeout(Duration::from_secs(10), api.send(req))
        .await
        .expect("refused before reading the body");
    r.assert_problem(413, "too-large");
    assert_eq!(api.api.uploader.started(), started);

    // Exactly `max_upload` is accepted (a PDF: no job).
    let exact = pdf(MAX_UPLOAD as usize, "exact");
    let body = created(
        &admin
            .upload(&api, &[Part::file("exact.pdf", "application/pdf", &exact)])
            .await,
    );
    assert_eq!(body["bytes"], MAX_UPLOAD);
    assert_eq!(job_count(&api).await, 0);
    api
}

async fn type_confusion_is_refused(api: Api) -> Api {
    let admin = admin(&api).await;
    let html: &[u8] = b"<!doctype html><html><body><script>alert(1)</script></body></html>";
    let mut html_zip = html.to_vec();
    html_zip.extend_from_slice(&zip(b"trailer"));
    let mut utf16 = vec![0xFF, 0xFE];
    for c in "plain words".encode_utf16() {
        utf16.extend_from_slice(&c.to_le_bytes());
    }
    let mut elf = b"\x7fELF\x02\x01\x01\x00".to_vec();
    elf.resize(256, 0);
    let mut exe = b"MZ\x90\x00\x03\x00\x00\x00".to_vec();
    exe.resize(256, 0);
    exe[0x3c] = 0x80;
    exe.resize(0x80, 0);
    exe.extend_from_slice(b"PE\0\0");
    exe.resize(512, 0);
    // ISO-BMFF images infer names but that are not served inline (review m4).
    let isobmff = |brand: &[u8; 4]| {
        let mut v = vec![0, 0, 0, 0x1c];
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(brand);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(brand);
        v.extend_from_slice(b"mif1miaf");
        v.resize(256, 0);
        v
    };
    let avif = isobmff(b"avif");
    let heic = isobmff(b"heic");
    // A gzip-compressed SVG: never decompressed, so not an SVG.
    let mut svgz = b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03".to_vec();
    svgz.resize(64, 0x55);
    let refused: [(&str, &str, &[u8]); 13] = [
        ("x.avif", "image/avif", &avif),
        ("x.heic", "image/heic", &heic),
        ("x.svgz", "image/svg+xml", &svgz),
        // HTML claiming to be a PNG, by type and by name.
        ("x.png", "image/png", html),
        ("x.html", "text/html", html),
        // Executables.
        ("x", "application/octet-stream", &elf),
        ("setup.exe", "application/octet-stream", &exe),
        // Text classes: markup first, UTF-16 (NULs).
        (
            "x.txt",
            "text/plain",
            b"<html><script>alert(1)</script></html>",
        ),
        ("x.txt", "text/plain", b"  \n <script>alert(1)</script>"),
        ("x.txt", "text/plain", &utf16),
        // Unknown magic.
        (
            "x.bin",
            "application/octet-stream",
            b"\x13\x37\xab\xcd random",
        ),
        // HTML first with a ZIP trailer; a .docx that is not a ZIP.
        ("x.zip", "application/zip", &html_zip),
        ("x.docx", "application/octet-stream", b"just some words"),
    ];
    for (name, claimed, bytes) in refused {
        let r = admin
            .upload(&api, &[Part::file(name, claimed, bytes)])
            .await;
        let p = r.assert_problem(415, "unsupported-media-type");
        assert_eq!(field(&p), "file", "{name}: {p}");
        assert!(!r.body.contains("alert"), "{name}: {p}");
    }
    // A bidi override in the file name is refused before the file is read (review m2).
    let r = admin
        .upload(
            &api,
            &[Part::file(
                "report\u{202E}fdp.zip",
                "application/zip",
                &zip(b"x"),
            )],
        )
        .await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "file");
    let long_name = format!("{}.png", "n".repeat(252));
    let r = admin
        .upload(
            &api,
            &[Part::file(&long_name, "image/png", &png(64, "long"))],
        )
        .await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "file");
    // Decided before any byte is stored (P4).
    assert!(api.stored_files().is_empty(), "{:?}", api.stored_files());
    assert_eq!(media_count(&api, &admin).await, 0);

    // A ZIP named .txt is not text (the name picks only within a class).
    let r = admin
        .upload(&api, &[Part::file("x.txt", "text/plain", &zip(b"words"))])
        .await;
    r.assert_problem(415, "unsupported-media-type");

    // A PNG named .svg is a PNG.
    let body = created(
        &admin
            .upload(
                &api,
                &[Part::file("x.svg", "image/svg+xml", &png(200, "named-svg"))],
            )
            .await,
    );
    assert_eq!(body["mime"], "image/png");
    assert_eq!(body["sanitized"], false);

    // GIFAR-style polyglots: image magic wins and the file is served as that image,
    // with nosniff.
    let mut gifar = png(300, "gifar");
    gifar.extend_from_slice(html);
    let mut gif_jar = b"GIF89a\x01\x00\x01\x00\x00\x00\x00".to_vec();
    gif_jar.extend_from_slice(&zip(b"META-INF/MANIFEST.MF"));
    for (name, bytes, mime) in [
        ("gifar.png", &gifar, "image/png"),
        ("gifar.gif", &gif_jar, "image/gif"),
    ] {
        let body = created(
            &admin
                .upload(&api, &[Part::file(name, "image/png", bytes)])
                .await,
        );
        assert_eq!(body["mime"], mime, "{name}");
        let file = api.get(body["url"].as_str().unwrap()).await;
        assert_eq!(file.header("content-type"), Some(mime), "{file:?}");
        assert_eq!(file.header("x-content-type-options"), Some("nosniff"));
    }

    // Media and images followed by HTML: the magic decides, served as that type with
    // nosniff, so the browser never renders the HTML (review m4).
    let mut mp4 = vec![0, 0, 0, 0x18];
    mp4.extend_from_slice(b"ftypisom\x00\x00\x02\x00isomiso2");
    mp4.extend_from_slice(html);
    let mut jpeg = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00".to_vec();
    jpeg.extend_from_slice(b"\xFF\xD9");
    jpeg.extend_from_slice(html);
    let mut png_late_html = png(9000, "late");
    png_late_html.extend_from_slice(html);
    for (name, bytes, mime) in [
        ("clip.mp4", &mp4, "video/mp4"),
        ("photo.jpg", &jpeg, "image/jpeg"),
        ("late.png", &png_late_html, "image/png"),
    ] {
        let body = created(
            &admin
                .upload(&api, &[Part::file(name, "text/html", bytes)])
                .await,
        );
        assert_eq!(body["mime"], mime, "{name}");
        let file = api.get(body["url"].as_str().unwrap()).await;
        assert_eq!(file.header("content-type"), Some(mime), "{name}: {file:?}");
        assert_eq!(file.header("x-content-type-options"), Some("nosniff"));
    }

    // An SVG with a UTF-8 BOM is still an SVG, and sanitised.
    let bom_svg = b"\xEF\xBB\xBF<svg xmlns=\"http://www.w3.org/2000/svg\"><script>x()</script><rect width=\"1\" height=\"1\"/></svg>";
    let body = created(
        &admin
            .upload(&api, &[Part::file("bom.svg", "image/svg+xml", bom_svg)])
            .await,
    );
    assert_eq!(body["mime"], "image/svg+xml");
    assert_eq!(body["sanitized"], true);
    let stored = api.get(body["url"].as_str().unwrap()).await;
    assert!(!stored.body.contains("script"), "{}", stored.body);

    // A PDF with JavaScript is a PDF, served inline only under `sandbox` (F14).
    let pdf_js =
        b"%PDF-1.7\n1 0 obj << /OpenAction << /S /JavaScript /JS (app.alert(1)) >> >> endobj\n";
    let body = created(
        &admin
            .upload(&api, &[Part::file("js.pdf", "application/pdf", pdf_js)])
            .await,
    );
    assert_eq!(body["mime"], "application/pdf");
    let file = api.get(body["url"].as_str().unwrap()).await;
    assert_eq!(file.header("content-type"), Some("application/pdf"));
    assert_eq!(file.header("content-security-policy"), Some("sandbox"));

    // A ZIP that also holds HTML is a ZIP, download-only.
    let mut zip_html = zip(b"x");
    zip_html.extend_from_slice(html);
    let body = created(
        &admin
            .upload(&api, &[Part::file("site.zip", "text/html", &zip_html)])
            .await,
    );
    assert_eq!(body["mime"], "application/zip");
    let file = api.get(body["url"].as_str().unwrap()).await;
    assert_eq!(
        file.header("content-type"),
        Some("application/octet-stream")
    );
    assert!(
        file.header("content-disposition")
            .unwrap()
            .starts_with("attachment"),
        "{file:?}"
    );
    api
}

async fn document_classes_are_download_only(api: Api) -> Api {
    let admin = admin(&api).await;
    let zips = [
        (
            "report.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        (
            "SHEET.XLSX",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        (
            "deck.pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
        ("letter.odt", "application/vnd.oasis.opendocument.text"),
        (
            "table.ods",
            "application/vnd.oasis.opendocument.spreadsheet",
        ),
        (
            "slides.odp",
            "application/vnd.oasis.opendocument.presentation",
        ),
        ("archive.zip", "application/zip"),
    ];
    let mut cases: Vec<(&str, &str, Vec<u8>)> = zips
        .iter()
        .map(|(name, mime)| (*name, *mime, zip(name.as_bytes())))
        .collect();
    cases.push(("notes.txt", "text/plain", "Hello,\nworld ✓\n".into()));
    cases.push(("bom.txt", "text/plain", "\u{feff}Byte order mark".into()));
    cases.push(("data.csv", "text/csv", b"a,b\n1,2\n".to_vec()));
    cases.push(("empty.txt", "text/plain", Vec::new()));
    // infer's 3-byte MP3 match does not win over a text name with a text head (review m5).
    cases.push(("id3.txt", "text/plain", b"ID3 tags, explained\n".to_vec()));
    // A character split by the 8 KiB head is still text.
    let long = format!("{}é{}", "a".repeat(8191), "b".repeat(800));
    cases.push(("long.txt", "text/plain", long.into_bytes()));
    for (name, mime, bytes) in &cases {
        let body = created(
            &admin
                .upload(&api, &[Part::file(name, "application/octet-stream", bytes)])
                .await,
        );
        assert_eq!(body["mime"], *mime, "{name}");
        assert_eq!(body["sanitized"], false, "{name}");
        assert_eq!(body["bytes"], bytes.len(), "{name}");
        // R1: never inline, never sniffed.
        let file = api.get(body["url"].as_str().unwrap()).await;
        assert_eq!(file.status.as_u16(), 200, "{name}: {file:?}");
        assert_eq!(
            file.header("content-type"),
            Some("application/octet-stream"),
            "{name}"
        );
        assert!(
            file.header("content-disposition")
                .unwrap()
                .starts_with("attachment;"),
            "{name}: {file:?}"
        );
        assert_eq!(file.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(file.body.as_bytes(), bytes.as_slice(), "{name}");
    }
    // PDF stays inline under `sandbox` (F14).
    let body = created(
        &admin
            .upload(
                &api,
                &[Part::file("paper.pdf", "application/pdf", &pdf(64, "p"))],
            )
            .await,
    );
    let file = api.get(body["url"].as_str().unwrap()).await;
    assert_eq!(file.header("content-type"), Some("application/pdf"));
    assert_eq!(file.header("content-disposition"), Some("inline"));
    assert_eq!(file.header("content-security-policy"), Some("sandbox"));
    // Documents get no variants job.
    assert_eq!(job_count(&api).await, 0);
    api
}

async fn contributor_upload_is_403_before_reading(api: Api) -> Api {
    api.user("contrib", Role::Contributor, false).await;
    let mut contributor = api.logged_in("contrib").await;
    let author = api.user("author", Role::Author, false).await;
    let content_only = api.bearer(&author, &["content:write", "media:read"]).await;
    for who in ["contributor", "token without media:write"] {
        // The body never finishes: a handler that read it would hang.
        let (body, _gate) = gated_body(
            part_head(&Part::file("x.png", "image/png", &[])),
            png(100, "never"),
        );
        let b = if who == "contributor" {
            contributor.builder(Method::POST, "/api/v1/media")
        } else {
            content_only.builder(Method::POST, "/api/v1/media")
        };
        let req = b
            .header(header::CONTENT_TYPE, upload_content_type())
            .body(body)
            .unwrap();
        let r = tokio::time::timeout(Duration::from_secs(10), async {
            if who == "contributor" {
                contributor.send_on(&api.router, req).await
            } else {
                api.send(req).await
            }
        })
        .await
        .expect("answered before the body");
        let p = r.assert_problem(403, "forbidden");
        assert!(p["detail"].is_string(), "{who}: {p}");
        // No upload slot was taken.
        assert_eq!(api.api.uploader.started(), 0, "{who}");
        assert_eq!(api.api.uploader.in_flight(), 0, "{who}");
    }
    api
}

async fn upload_slots_bound_a_node(api: Api) -> Api {
    let admin = admin(&api).await;
    let mut gates = Vec::new();
    let mut tasks = Vec::new();
    for i in 0..MEDIA_UPLOAD_SLOTS {
        let name = format!("held-{i}.png");
        let bytes = png(20_000, &name);
        let mut first = part_head(&Part::file(&name, "image/png", &[]));
        first.extend_from_slice(&bytes[..100]);
        let mut rest = bytes[100..].to_vec();
        rest.extend_from_slice(b"\r\n");
        rest.extend_from_slice(&body_end());
        let (body, gate) = gated_body(first, rest);
        let req = admin
            .builder(Method::POST, "/api/v1/media")
            .header(header::CONTENT_TYPE, upload_content_type())
            .body(body)
            .unwrap();
        let router = api.router.clone();
        tasks.push(tokio::spawn(async move { send(&router, req).await }));
        gates.push(gate);
    }
    let uploader = api.api.uploader.clone();
    wait_until("every slot held", || {
        uploader.in_flight() == MEDIA_UPLOAD_SLOTS
    })
    .await;
    // The ninth is refused at once.
    let r = admin
        .upload(&api, &[Part::file("ninth.png", "image/png", &png(64, "9"))])
        .await;
    r.assert_problem(503, "busy");
    assert_eq!(r.header("retry-after"), Some("5"));
    for gate in gates {
        gate.send(()).unwrap();
    }
    for task in tasks {
        let r = task.await.unwrap();
        assert_eq!(r.status.as_u16(), 201, "{r:?}");
    }
    assert_eq!(api.api.uploader.in_flight(), 0);
    created(
        &admin
            .upload(&api, &[Part::file("ninth.png", "image/png", &png(64, "9"))])
            .await,
    );
    assert_eq!(media_count(&api, &admin).await, MEDIA_UPLOAD_SLOTS + 1);
    api
}

async fn media_list_pages(api: Api) -> Api {
    let admin = admin(&api).await;
    let author = api.user("writer", Role::Author, false).await;
    let writer = api.bearer(&author, &["media:write", "media:read"]).await;
    for name in ["a", "b", "c"] {
        created(
            &admin
                .upload(
                    &api,
                    &[Part::file(
                        &format!("{name}.pdf"),
                        "application/pdf",
                        &pdf(32, name),
                    )],
                )
                .await,
        );
    }
    let mine = created(
        &writer
            .upload(
                &api,
                &[Part::file("mine.pdf", "application/pdf", &pdf(32, "m"))],
            )
            .await,
    );
    let r = admin.get(&api, "/api/v1/media?limit=3").await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    let page = r.json();
    let titles: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    // Newest change first; the title falls back to the file name.
    assert_eq!(titles, ["mine", "c", "b"]);
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();
    let r = admin
        .get(&api, &format!("/api/v1/media?limit=3&cursor={cursor}"))
        .await;
    let page = r.json();
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "{page}");
    assert_eq!(page["items"][0]["title"], "a");
    assert!(page["next_cursor"].is_null(), "{page}");

    let r = admin
        .get(&api, &format!("/api/v1/media?author={}", author.id))
        .await;
    let page = r.json();
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "{page}");
    assert_eq!(page["items"][0]["id"], mine["id"]);
    assert_eq!(page["items"][0]["author_id"], author.id.to_string());

    let p = admin
        .get(&api, "/api/v1/media?cursor=garbage")
        .await
        .assert_problem(400, "validation");
    assert_eq!(field(&p), "cursor");
    let p = admin
        .get(&api, "/api/v1/media?limit=0")
        .await
        .assert_problem(400, "validation");
    assert_eq!(field(&p), "limit");
    // Reads need `media:read`.
    let reader = api.bearer(&author, &["content:read"]).await;
    reader
        .get(&api, "/api/v1/media")
        .await
        .assert_problem(403, "forbidden");
    api
}

async fn upload_parts_are_checked(api: Api) -> Api {
    let admin = admin(&api).await;
    let file = png(100, "parts");
    let f = Part::file("photo.png", "image/png", &file);
    let r = admin.upload(&api, &[f, Part::text("extra", "x")]).await;
    r.assert_problem(400, "malformed-body");
    let r = admin.upload(&api, &[f, f]).await;
    r.assert_problem(400, "malformed-body");
    let r = admin
        .upload(
            &api,
            &[Part::text("title", "a"), f, Part::text("title", "b")],
        )
        .await;
    r.assert_problem(400, "malformed-body");
    let r = admin.upload(&api, &[Part::text("title", "only")]).await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "file");
    let nameless = Part {
        filename: None,
        ..f
    };
    let r = admin.upload(&api, &[nameless]).await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "file");
    let r = admin
        .upload(&api, &[f, Part::text("title", &"t".repeat(201))])
        .await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "title");
    let r = admin
        .upload(&api, &[f, Part::text("alt", &"a".repeat(10_000))])
        .await;
    r.assert_problem(413, "too-large");
    let r = admin
        .upload(
            &api,
            &[
                Part {
                    bytes: b"\xff\xfe",
                    ..Part::text("title", "")
                },
                f,
            ],
        )
        .await;
    assert_eq!(field(&r.assert_problem(422, "validation")), "title");
    // Not multipart.
    let r = admin
        .send(&api, Method::POST, "/api/v1/media", Some(&json!({})), None)
        .await;
    r.assert_problem(415, "unsupported-media-type");

    // Only the last path segment of the file name is kept; the title comes from it.
    let body = created(
        &admin
            .upload(
                &api,
                &[Part::file("..\\..\\dir/holiday.png", "image/png", &file)],
            )
            .await,
    );
    assert_eq!(body["original_name"], "holiday.png");
    assert_eq!(body["title"], "holiday");
    assert_eq!(body["alt"], "");
    api
}

async fn aborted_uploads_leave_nothing_staged(api: Api) -> Api {
    let admin = admin(&api).await;
    // Larger than the store keeps in memory (8 MiB), so these stream through staging.
    //
    // On the cluster profile `stored_files` walks only `<bucket>/<prefix>`: s3s-fs may keep
    // in-progress multipart parts elsewhere, so there this proves only that no object
    // landed under the prefix, not that the multipart upload was aborted. The abort
    // itself is `put_stream`'s staging guard, covered by ferrite-media's store tests
    // (`cancelled_upload_cleans_staging`); on local media the check is complete.
    let (router, uploader) = api.router_with_max_upload(16 << 20);

    // The client goes away after 10 MiB.
    let mut first = part_head(&Part::file("big.png", "image/png", &[]));
    first.extend_from_slice(&png(10 << 20, "gone"));
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, upload_content_type())
        .body(broken_body(first))
        .unwrap();
    let r = send(&router, req).await;
    assert!(r.status.is_client_error(), "{r:?}");
    wait_until("staging cleaned up", || api.stored_files().is_empty()).await;

    // Over the limit while streaming.
    let over = png((16 << 20) + 1, "over");
    let (_, body) = upload_body(&[Part::file("over.png", "image/png", &over)]);
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, upload_content_type())
        .body(chunked_body(body))
        .unwrap();
    send(&router, req).await.assert_problem(413, "too-large");
    wait_until("staging cleaned up", || api.stored_files().is_empty()).await;
    assert_eq!(media_count(&api, &admin).await, 0);
    assert_eq!(job_count(&api).await, 0);
    assert_eq!(uploader.in_flight(), 0);

    // Within the limit it is stored once, hashed while streaming.
    let ok = png(9 << 20, "ok");
    let (_, body) = upload_body(&[Part::file("ok.png", "image/png", &ok)]);
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, upload_content_type())
        .body(chunked_body(body))
        .unwrap();
    let body = created(&send(&router, req).await);
    assert_eq!(body["bytes"], 9 << 20);
    let file = api.get(body["url"].as_str().unwrap()).await;
    assert_eq!(file.status.as_u16(), 200, "{file:?}");
    let len = (9u64 << 20).to_string();
    assert_eq!(file.header("content-length"), Some(len.as_str()));
    assert_eq!(uploader.in_flight(), 0);
    api
}

async fn idle_upload_is_aborted_and_frees_its_slot(api: Api) -> Api {
    let admin = admin(&api).await;
    // `media.upload_idle_timeout` shortened for the test; 16 MiB so the second case
    // stalls after the store has switched to a staging object.
    let (router, uploader) = api.router_with_uploader(
        Uploader::new(api.api.media.clone(), 16 << 20)
            .with_idle_timeout(Duration::from_millis(300)),
    );
    // A stream that pauses inside the head, and one that pauses after 9 MiB. The gates
    // are never opened while the requests run.
    for (case, sent) in [("head", 100usize), ("staging", 9 << 20)] {
        let bytes = png(sent + 1000, case);
        let mut first = part_head(&Part::file("slow.png", "image/png", &[]));
        first.extend_from_slice(&bytes[..sent]);
        let mut rest = bytes[sent..].to_vec();
        rest.extend_from_slice(b"\r\n");
        rest.extend_from_slice(&body_end());
        let (body, gate) = gated_body(first, rest);
        let req = admin
            .builder(Method::POST, "/api/v1/media")
            .header(header::CONTENT_TYPE, upload_content_type())
            .body(body)
            .unwrap();
        let r = tokio::time::timeout(Duration::from_secs(10), send(&router, req))
            .await
            .unwrap_or_else(|_| panic!("{case}: the paused upload was not aborted"));
        let p = r.assert_problem(400, "malformed-body");
        assert_eq!(field(&p), "file", "{case}: {p}");
        assert_eq!(uploader.in_flight(), 0, "{case}");
        wait_until("staging cleaned up", || api.stored_files().is_empty()).await;
        drop(gate);
    }
    assert_eq!(uploader.started(), 2);
    assert_eq!(media_count(&api, &admin).await, 0);
    assert_eq!(job_count(&api).await, 0);
    // The freed slots take uploads again.
    let (_, body) = upload_body(&[Part::file("ok.png", "image/png", &png(64, "ok"))]);
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, upload_content_type())
        .body(axum::body::Body::from(body))
        .unwrap();
    created(&send(&router, req).await);
    api
}
