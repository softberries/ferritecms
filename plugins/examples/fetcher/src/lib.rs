//! Test plugin for the real `http` import (M2a-2b Task 11): `GET /get?u=<url>` fetches
//! `u` and answers `ok <status> <body>` (the body as lossy UTF-8) or `err <message>`.
//!
//! Optional query parameters: `m=<method>` (default `GET`), `h=<name>:<value>`
//! (repeatable request headers), `b=<n>` (a request body of `n` bytes `x`) and `n=<k>`
//! (`k` fetches in one call, at most 10, one answer line each).
use ferrite_sdk::{Ctx, HttpRequest, Result, RouteRequest, RouteResponse};

fn param<'a>(req: &'a RouteRequest, name: &str) -> Option<&'a str> {
    req.query
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn get(req: RouteRequest, ctx: &Ctx) -> Result<RouteResponse> {
    let headers = req
        .query
        .iter()
        .filter(|(k, _)| k == "h")
        .filter_map(|(_, v)| v.split_once(':'))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    let fetch = HttpRequest {
        method: param(&req, "m").unwrap_or("GET").to_owned(),
        url: param(&req, "u").unwrap_or_default().to_owned(),
        headers,
        body: param(&req, "b")
            .and_then(|n| n.parse::<usize>().ok())
            .map(|n| vec![b'x'; n]),
    };
    let times = param(&req, "n")
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 10);
    let lines: Vec<String> = (0..times)
        .map(|_| match ctx.http().fetch(&fetch) {
            Ok(res) => format!("ok {} {}", res.status, String::from_utf8_lossy(&res.body)),
            Err(e) => format!("err {e}"),
        })
        .collect();
    Ok(RouteResponse {
        status: 200,
        headers: vec![("Content-Type".into(), "text/plain".into())],
        body: lines.join("\n").into_bytes(),
    })
}

ferrite_sdk::export_plugin! {
    filters: {},
    routes: { "/get" => get },
}
