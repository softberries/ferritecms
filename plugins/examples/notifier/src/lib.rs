//! Example `mail = "admins"` plugin (M2a-2 P18): one mail to the site's admins when a
//! post is published, naming the post. The host adds the site-title prefix and a footer.
//! The host refuses a subject over 200 characters, so a long title is cut first.
use ferrite_sdk::bindings::ferrite::plugin::{content, mail};
use ferrite_sdk::{ActionEvent, Ctx, PluginError, Result};

/// "Published: " (11) + 180 + "…" stays within the host's 200-character subject limit.
const MAX_TITLE_CHARS: usize = 180;

fn published(ev: ActionEvent, _ctx: &Ctx) -> Result<()> {
    let id = ev.subject.unwrap_or_default();
    let title = content::get_post(&id).map_or_else(|| id.clone(), |p| p.title);
    let mut short: String = title.chars().take(MAX_TITLE_CHARS).collect();
    if short.len() < title.len() {
        short.push('…');
    }
    mail::notify_admins(
        &format!("Published: {short}"),
        &format!("The post \"{title}\" was published.\nPost id: {id}"),
    )
    .map_err(PluginError::Other)
}

ferrite_sdk::export_plugin! {
    filters: {},
    actions: { "post.published" => published },
}
