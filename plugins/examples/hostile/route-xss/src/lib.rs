//! Hostile: a route that tries to run script in the site's origin and to take over its
//! session (M2a-2 Review Focus 2). `GET /x` answers HTML reading `document.cookie`,
//! with `Set-Cookie`, `Authorization`, a permissive CSP, HSTS and CORS headers, and
//! lists the request header names it received in an HTML comment (pre-flight F26).
//! `GET /panic` panics; `GET /loop` never returns.
use ferrite_sdk::{Ctx, Result, RouteRequest, RouteResponse};

fn x(req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    let names: Vec<String> = req
        .headers
        .iter()
        .map(|(k, _)| k.to_ascii_lowercase())
        .collect();
    let body = format!(
        "<!doctype html><script>document.cookie</script><!-- headers: {} -->",
        names.join(",")
    );
    let headers = [
        ("Content-Type", "text/html"),
        ("Set-Cookie", "ferrite_session=stolen; Path=/"),
        ("Authorization", "Bearer stolen"),
        ("Content-Security-Policy", "default-src *"),
        ("Strict-Transport-Security", "max-age=0"),
        ("Access-Control-Allow-Origin", "*"),
        ("Cache-Control", "public, max-age=31536000"),
    ];
    Ok(RouteResponse {
        status: 200,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        body: body.into_bytes(),
    })
}

fn boom(_req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    panic!("boom")
}

fn spin(_req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    loop {
        core::hint::black_box(());
    }
}

ferrite_sdk::export_plugin! {
    filters: {},
    routes: {
        "/x" => x,
        "/panic" => boom,
        "/loop" => spin,
    },
}
