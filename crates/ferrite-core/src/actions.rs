//! Action hooks enqueued by core (M2a-2 P20, design 05 §4).
//!
//! A write that fires an action hook calls [`enqueue`] inside its own `Tx`: one
//! [`plugin.action`](crate::jobs::PLUGIN_ACTION_KIND) job per enabled plugin whose
//! manifest subscribes to the hook. The jobs commit or roll back with the write, and the
//! actions run later in the job runner, bounded like every plugin call, so a failing
//! action never affects the write. Payloads carry ids only.
//!
//! | Hook | Fired by | Subject | Payload |
//! |---|---|---|---|
//! | `post.saved` | content create, update, revision restore | item id | `{"post": id}` |
//! | `post.published` | every transition to `published` (publish, scheduled publish) | item id | `{"post": id}` |
//! | `user.created` | user create (API and operator) | user id | `{"user": id}` |

use ferrite_store::Repos;
use serde_json::Value;

use crate::{
    CoreError,
    jobs::{PLUGIN_ACTION_VERSION, PluginAction, plugin_action_job},
};

/// A content item was created or its content changed.
pub const POST_SAVED: &str = "post.saved";
/// A content item became published.
pub const POST_PUBLISHED: &str = "post.published";
/// A user account was created.
pub const USER_CREATED: &str = "user.created";

/// Enqueues `hook` for every enabled plugin row whose `manifest.hooks` names it, in
/// name order, with `repos` (the write's `Tx`); returns how many jobs were enqueued.
/// Rows are read in the same `Tx`, so a plugin enabled or disabled concurrently is
/// either seen or not; the handler re-checks the row when the job runs (P29).
pub async fn enqueue(
    repos: &dyn Repos,
    hook: &str,
    subject: Option<String>,
    payload: Value,
) -> Result<u32, CoreError> {
    let mut enqueued = 0;
    for plugin in repos.plugins().list().await? {
        if !plugin.enabled || !subscribes(&plugin.manifest, hook) {
            continue;
        }
        let action = PluginAction {
            v: PLUGIN_ACTION_VERSION,
            plugin: plugin.name,
            hook: hook.to_owned(),
            subject: subject.clone(),
            payload: payload.clone(),
        };
        repos.jobs().enqueue(&plugin_action_job(&action)).await?;
        enqueued += 1;
    }
    Ok(enqueued)
}

/// Whether the stored manifest JSON (the TOML table as JSON) has `hook` in `[hooks]`.
fn subscribes(manifest: &Value, hook: &str) -> bool {
    manifest
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| hooks.contains_key(hook))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subscribes_reads_the_hooks_table() {
        let m = json!({"plugin": {"name": "x"}, "hooks": {"post.saved": 10}});
        assert!(subscribes(&m, POST_SAVED));
        assert!(!subscribes(&m, POST_PUBLISHED));
        assert!(!subscribes(&json!({"name": "x"}), POST_SAVED));
        assert!(!subscribes(&json!({"hooks": ["post.saved"]}), POST_SAVED));
    }
}
