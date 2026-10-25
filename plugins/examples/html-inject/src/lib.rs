//! Test fixture for output sanitising: appends a script, an inline handler, a
//! `javascript:` link and a plain link to every body, a marker to every excerpt, and
//! a footer note with a script to every page footer (`render.footer`). Installed with
//! `render.html = "sanitized"` the host strips the active parts; tests also install it
//! with a `raw` manifest, where everything passes through.
use ferrite_sdk::{Ctx, Result};

const INJECTED: &str = "<script>alert(1)</script>\
<img src=\"/x.png\" onerror=\"alert(2)\">\
<a href=\"javascript:alert(3)\">js</a>\
<a href=\"https://example.org/\">ok</a>";

/// Appended to the page footer: the paragraph survives sanitising, the script, the
/// handler and the class do not.
const FOOTER: &str = "<p class=\"injected-footer\" onclick=\"alert(4)\">Injected footer</p>\
<script>alert(5)</script>";

fn body(html: String, _ctx: &Ctx) -> Result<String> {
    Ok(format!("{html}{INJECTED}"))
}

fn excerpt(text: String, _ctx: &Ctx) -> Result<String> {
    Ok(format!("{text} [filtered]"))
}

fn footer(html: String, _ctx: &Ctx) -> Result<String> {
    Ok(format!("{html}{FOOTER}"))
}

ferrite_sdk::export_plugin! {
    filters: {
        "content.body" => body,
        "content.excerpt" => excerpt,
        "render.footer" => footer,
    },
}
