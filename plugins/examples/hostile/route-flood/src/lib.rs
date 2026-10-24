//! Hostile target: a cross-origin form endpoint that visitors (or a bot) flood with
//! POSTs. The host's per-route limit (`5/min`, counted in the database) must cut the
//! flood on every node (M2a-2 Review Focus 2).
use ferrite_sdk::{Ctx, Result, RouteRequest, RouteResponse};

fn submit(_req: RouteRequest, _ctx: &Ctx) -> Result<RouteResponse> {
    Ok(RouteResponse {
        status: 200,
        headers: vec![("Content-Type".to_owned(), "text/plain".to_owned())],
        body: b"ok".to_vec(),
    })
}

ferrite_sdk::export_plugin! {
    filters: {},
    routes: { "/submit" => submit },
}
