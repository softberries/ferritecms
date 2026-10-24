//! Example `mail = "admins"` plugin (M2a-2 P18): one mail to the site's admins when a
//! post is published, naming the post. The host adds the site-title prefix and a footer.
use ferrite_sdk::bindings::ferrite::plugin::{content, mail};
use ferrite_sdk::{ActionEvent, Ctx, PluginError, Result};

fn published(ev: ActionEvent, _ctx: &Ctx) -> Result<()> {
    let id = ev.subject.unwrap_or_default();
    let title = content::get_post(&id).map_or_else(|| id.clone(), |p| p.title);
    mail::notify_admins(
        &format!("Published: {title}"),
        &format!("The post \"{title}\" was published.\nPost id: {id}"),
    )
    .map_err(PluginError::Other)
}

ferrite_sdk::export_plugin! {
    filters: {},
    actions: { "post.published" => published },
}
