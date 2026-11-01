//! M5 Task 8: the MCP tool reference page (`site/content/05-reference/02-mcp-tools.md`)
//! stays in step with the server: every tool `tools/list` returns has a row in the
//! page's tables and every row names a tool (design 12 §9), the page says what a tool
//! needs and does correctly for the annotation it carries, and the human-only actions of
//! design 12 §5.4 have no tool.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
    mcp::{mcp_call, tool},
};
use ferrite_domain::Role;

api_tests!(
    reference_page_lists_every_tool,
    reference_rows_match_annotations_and_scopes,
    no_tool_installs_or_deletes_extensions_or_users,
    prompts_name_only_existing_tools,
);

const PAGE: &str = "site/content/05-reference/02-mcp-tools.md";

async fn token(api: &Api) -> Bearer {
    let u = api.user("ada", Role::Admin, false).await;
    api.bearer(&u, &["*"]).await
}

/// Every tool the server lists.
async fn tools(api: &Api, t: &Bearer) -> Vec<Value> {
    let (_, body) = mcp_call(api, &t.0, "tools/list", json!({})).await;
    body["result"]["tools"].as_array().unwrap().clone()
}

fn names(tools: &[Value]) -> BTreeSet<String> {
    tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

/// One row of a tool table: the tool, its scope and whether it reads or writes.
#[derive(Debug)]
struct Row {
    name: String,
    scope: String,
    access: String,
}

/// The rows of every table of the page: lines whose first cell is a backticked tool
/// name. Header and separator rows have no backticks in the first cell.
fn rows() -> Vec<Row> {
    let path = crate::harness::repo_root().join(PAGE);
    let page = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    page.lines()
        .filter(|l| l.trim_start().starts_with('|'))
        .filter_map(|l| {
            let cells: Vec<&str> = l
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            let name = cells.first()?.strip_prefix('`')?.strip_suffix('`')?;
            assert!(
                cells.len() >= 4,
                "a row needs name, what, scope, access: {l}"
            );
            Some(Row {
                name: name.to_owned(),
                scope: cells[2].to_owned(),
                access: cells[3].to_owned(),
            })
        })
        .collect()
}

async fn reference_page_lists_every_tool(api: Api) -> Api {
    let t = token(&api).await;
    let listed = names(&tools(&api, &t).await);
    let rows = rows();
    let mut seen = BTreeSet::new();
    for row in &rows {
        assert!(
            seen.insert(row.name.clone()),
            "{} is listed twice",
            row.name
        );
    }
    let documented: BTreeSet<String> = rows.into_iter().map(|r| r.name).collect();
    let missing: Vec<_> = listed.difference(&documented).collect();
    let unknown: Vec<_> = documented.difference(&listed).collect();
    assert!(missing.is_empty(), "tools missing from {PAGE}: {missing:?}");
    assert!(
        unknown.is_empty(),
        "rows of {PAGE} without a tool: {unknown:?}"
    );
    assert!(listed.len() >= 56, "{listed:?}");
    api
}

async fn reference_rows_match_annotations_and_scopes(api: Api) -> Api {
    let t = token(&api).await;
    let tools = tools(&api, &t).await;
    for row in rows() {
        let tool = tools
            .iter()
            .find(|t| t["name"] == row.name.as_str())
            .unwrap();
        let read_only = tool["annotations"]["readOnlyHint"] == true;
        let want = if read_only { "read" } else { "write" };
        assert_eq!(row.access, want, "{}: access", row.name);
        // The scope is `<area>:read` or `<area>:write`, matching the access; the site
        // overview needs none (a section the caller cannot read is left out).
        if row.scope.starts_with("none") {
            assert_eq!(
                row.name, "site_overview",
                "only the overview needs no scope"
            );
            continue;
        }
        let (area, level) = row
            .scope
            .trim_matches('`')
            .split_once(':')
            .unwrap_or_else(|| panic!("{}: scope {:?}", row.name, row.scope));
        assert_eq!(level, want, "{}: scope level", row.name);
        assert!(
            [
                "content",
                "media",
                "terms",
                "menus",
                "comments",
                "users",
                "settings",
                "redirects",
                "extensions",
                "system",
            ]
            .contains(&area),
            "{}: area {area}",
            row.name
        );
    }
    api
}

async fn no_tool_installs_or_deletes_extensions_or_users(api: Api) -> Api {
    let t = token(&api).await;
    let listed = names(&tools(&api, &t).await);
    // Design 12 §5.4: the assistant cannot install, uninstall or purge plugins or
    // themes, create, delete or change users, roles or passwords, manage tokens,
    // sessions, TOTP or OAuth grants, run imports, backups or restores, or retry jobs.
    for name in &listed {
        let words: Vec<&str> = name.split('_').collect();
        let area = words[0];
        let verbs = &words[1..];
        if ["plugin", "plugins", "theme", "themes", "user", "users"].contains(&area) {
            for verb in [
                "install",
                "uninstall",
                "purge",
                "delete",
                "remove",
                "create",
                "upload",
            ] {
                assert!(!verbs.contains(&verb), "{name}");
            }
        }
        for banned in [
            "install",
            "uninstall",
            "purge",
            "password",
            "role",
            "token",
            "tokens",
            "session",
            "sessions",
            "totp",
            "oauth",
            "grant",
            "grants",
            "import",
            "backup",
            "restore_site",
            "retry",
            "migrate",
            "update_check",
            "self_update",
            "rollback",
        ] {
            assert!(!words.contains(&banned), "{name} contains {banned}");
        }
    }
    // The user tools are the two reads.
    let users: BTreeSet<&str> = listed
        .iter()
        .map(String::as_str)
        .filter(|n| n.starts_with("user"))
        .collect();
    assert_eq!(users, BTreeSet::from(["user_get", "users_list"]));
    // The plugin and theme tools are exactly the reviewed set.
    let ext: BTreeSet<&str> = listed
        .iter()
        .map(String::as_str)
        .filter(|n| n.starts_with("plugin") || n.starts_with("theme"))
        .collect();
    assert_eq!(
        ext,
        BTreeSet::from([
            "plugin_disable",
            "plugin_enable",
            "plugin_settings_get",
            "plugin_settings_update",
            "plugins_list",
            "theme_activate",
            "theme_settings_get",
            "theme_settings_update",
            "themes_list",
        ])
    );
    // The system tools only read.
    let system: Vec<Value> = tools(&api, &t)
        .await
        .into_iter()
        .filter(|t| {
            ["system_status", "jobs_list", "audit_list"].contains(&t["name"].as_str().unwrap())
        })
        .collect();
    assert_eq!(system.len(), 3);
    assert!(
        system
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true)
    );
    // `mcp.enabled` is not reachable by any other tool either: nothing is named for it.
    assert!(!listed.iter().any(|n| n.contains("mcp")), "{listed:?}");
    api
}

/// A prompt names tools in its text; each must exist.
async fn prompts_name_only_existing_tools(api: Api) -> Api {
    let t = token(&api).await;
    let listed = names(&tools(&api, &t).await);
    let areas: BTreeSet<&str> = listed.iter().filter_map(|n| n.split('_').next()).collect();
    let (_, body) = mcp_call(&api, &t.0, "prompts/list", json!({})).await;
    let prompts = body["result"]["prompts"].as_array().unwrap();
    assert!(!prompts.is_empty(), "{body}");
    for prompt in prompts {
        let name = prompt["name"].as_str().unwrap();
        let args: serde_json::Map<String, Value> = prompt["arguments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| (a["name"].as_str().unwrap().to_owned(), json!("x")))
            .collect();
        let (_, got) = mcp_call(
            &api,
            &t.0,
            "prompts/get",
            json!({"name": name, "arguments": args}),
        )
        .await;
        let text = got["result"]["messages"].to_string();
        // Words like `content_create` or `media_list` (an area prefix, an underscore).
        for word in text.split(|c: char| !(c.is_ascii_lowercase() || c == '_')) {
            let Some((area, rest)) = word.split_once('_') else {
                continue;
            };
            if areas.contains(area) && !rest.is_empty() {
                assert!(
                    listed.contains(word),
                    "prompt {name} names {word}, which is no tool"
                );
            }
        }
    }
    // A tool that was already called works through the same server.
    let r = tool(&api, &t.0, "site_overview", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    api
}
