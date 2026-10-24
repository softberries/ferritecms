//! Hostile (M2a-2b Review Focus 4): tries to reach internal addresses through `http`.
//!
//! * `GET /get?u=<url>`: fetches `u` and answers `ok <status> <body>` or
//!   `err <message>` (the tests point the listed host at internal addresses).
//! * `render.head`: fetches the cloud metadata address on every page and reports the
//!   outcome in `<meta name="ssrf">`; the page must render whatever happens.
use ferrite_sdk::{Ctx, Head, HttpRequest, Result, RouteRequest, RouteResponse};

const METADATA: &str = "https://169.254.169.254/latest/meta-data/";

fn fetch(ctx: &Ctx, url: &str) -> String {
    let req = HttpRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Vec::new(),
        body: None,
    };
    match ctx.http().fetch(&req) {
        Ok(res) => format!("ok {} {}", res.status, String::from_utf8_lossy(&res.body)),
        Err(e) => format!("err {e}"),
    }
}

fn head(mut head: Head, ctx: &Ctx) -> Result<Head> {
    let outcome = fetch(ctx, METADATA);
    head.meta("ssrf", &outcome);
    Ok(head)
}

fn get(req: RouteRequest, ctx: &Ctx) -> Result<RouteResponse> {
    let url = req
        .query
        .iter()
        .find(|(k, _)| k == "u")
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    Ok(RouteResponse {
        status: 200,
        headers: vec![("Content-Type".into(), "text/plain".into())],
        body: fetch(ctx, url).into_bytes(),
    })
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
    routes: { "/get" => get },
}
