//! Example plugin routes under `/_p/router/` (M2a-2 Task 8): a JSON answer with the
//! visitor's user id and per-plugin IP hash, an echo, a same-site redirect, and two
//! responses the host must refuse (a cross-site redirect and a 2 MiB body).
use ferrite_sdk::{Ctx, Result, RouteRequest, RouteResponse};

fn respond(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> RouteResponse {
    RouteResponse {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        body,
    }
}

/// A JSON string literal (the inputs are a UUID and a hex hash, but escape anyway).
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn hello(req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    let user = req.user.as_deref().map_or("null".to_owned(), json_string);
    let body = format!(
        "{{\"user\":{user},\"ip\":{}}}",
        json_string(&req.client_ip_hash)
    );
    Ok(respond(
        200,
        &[("Content-Type", "application/json")],
        body.into_bytes(),
    ))
}

fn echo(req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    Ok(respond(200, &[("Content-Type", "text/plain")], req.body))
}

fn go(_req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    Ok(respond(303, &[("Location", "/thanks/")], Vec::new()))
}

fn away(_req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    Ok(respond(303, &[("Location", "https://evil.example/")], Vec::new()))
}

fn big(_req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    Ok(respond(
        200,
        &[("Content-Type", "text/plain")],
        vec![b'x'; 2 * 1024 * 1024],
    ))
}

ferrite_sdk::export_plugin! {
    filters: {},
    routes: {
        "/hello" => hello,
        "/echo" => echo,
        "/go" => go,
        "/away" => away,
        "/big" => big,
    },
}
