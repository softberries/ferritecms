//! Hostile mail plugin (M2a-2 Review Focus 3, P18). Every outcome is written to kv
//! (`ok` or the host's error text) and the hooks return `Ok`, so refusals never count
//! as plugin failures:
//!
//! * `post.saved`: 50 sends in one call; kv `first` = the first result, `second` = the
//!   second, `accepted` = how many of the 50 the host accepted.
//! * `post.published`: one send whose subject is the event subject (header-injection
//!   attempts); kv `inject` = the result.
//! * `render.head`: one send from a render filter; the result as meta `mail-flood`.
use ferrite_sdk::bindings::ferrite::plugin::mail;
use ferrite_sdk::{ActionEvent, Ctx, Head, Result};

fn outcome(r: &core::result::Result<(), String>) -> &str {
    r.as_ref().map_or_else(|e| e.as_str(), |()| "ok")
}

fn record(ctx: &Ctx, key: &str, value: &str) {
    let _ = ctx.kv().set(key, value.as_bytes());
}

fn flood(_ev: ActionEvent, ctx: &Ctx) -> Result<()> {
    let results: Vec<_> = (0..50)
        .map(|i| mail::notify_admins(&format!("Spam {i}"), "Buy now"))
        .collect();
    record(ctx, "first", outcome(&results[0]));
    record(ctx, "second", outcome(&results[1]));
    let accepted = results.iter().filter(|r| r.is_ok()).count();
    record(ctx, "accepted", &accepted.to_string());
    Ok(())
}

fn inject(ev: ActionEvent, ctx: &Ctx) -> Result<()> {
    let r = mail::notify_admins(&ev.subject.unwrap_or_default(), "Injected?");
    record(ctx, "inject", outcome(&r));
    Ok(())
}

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    let r = mail::notify_admins("From a page view", "Every visitor sends a mail?");
    head.meta("mail-flood", outcome(&r));
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
    actions: { "post.saved" => flood, "post.published" => inject },
}
