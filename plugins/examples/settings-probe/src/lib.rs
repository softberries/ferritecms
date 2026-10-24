//! Test fixture for plugin settings and secrets (M2a-2 P13, Review Focus 3): reports
//! what `settings.get` and `options.get` answer as one meta `settings-probe`:
//! `title=<…>;limit=<…>;key=<…>;undeclared=<…>;opt-a=<…>;opt-b=<…>` (`-` for none).
//! `opt-a` / `opt-b` are `options.get` of the `api_key` settings of the plugins
//! installed from this component as `probe-a` and `probe-b`.
use ferrite_sdk::bindings::ferrite::plugin::{options, settings};
use ferrite_sdk::{Ctx, Head, Result};

fn show(v: Option<String>) -> String {
    v.map_or_else(|| "-".to_owned(), |s| s.trim_matches('"').to_owned())
}

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    let report = format!(
        "title={};limit={};key={};undeclared={};opt-a={};opt-b={}",
        show(settings::get("title")),
        show(settings::get("limit")),
        show(settings::get("api_key")),
        show(settings::get("undeclared")),
        show(options::get("plugin.probe-a.settings.api_key")),
        show(options::get("plugin.probe-b.settings.api_key")),
    );
    head.meta("settings-probe", &report);
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
