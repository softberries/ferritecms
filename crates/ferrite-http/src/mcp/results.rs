//! Tool results (design 12 §5.1): structured content plus text, and errors as `isError`
//! results (never protocol errors), so the model can read what went wrong and correct
//! itself.
//!
//! * [`ok`]: `structuredContent` is the value; `content` is the short summary, then the
//!   value as JSON text (MCP: a tool returning structured content should also return it
//!   serialized, for clients that only pass `content` to the model).
//! * [`from_problem`] / [`from_core`]: `structuredContent` is the REST API's problem
//!   document (`type`, `title`, `status`, `detail`, `errors`, and `current` for a stale
//!   version); `content` is one text: `<title>: <detail>`, then one line per field
//!   error and a hint about `current`, followed (for a stale version) by a second block
//!   with the current item as JSON. A missing capability or scope is
//!   named (`Forbidden: missing content:read`).

use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;
use serde_json::Value;

use crate::api::problem::Problem;
use ferrite_core::CoreError;

/// A successful result: `value` as structured content, `summary` for people.
pub fn ok(value: impl Serialize, summary: impl Into<String>) -> CallToolResult {
    let value = match serde_json::to_value(value) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "MCP tool result not serializable");
            return from_problem(Problem::internal());
        }
    };
    let mut result = CallToolResult::structured(value);
    let json = result.content.pop();
    result.content = std::iter::once(ContentBlock::text(summary))
        .chain(json)
        .collect();
    result
}

/// An error result for a core error (see the module docs).
pub fn from_core(e: CoreError) -> CallToolResult {
    from_problem(Problem::from(e))
}

/// An error result for a problem (see the module docs).
pub fn from_problem(p: Problem) -> CallToolResult {
    let doc = p.document();
    let mut text = format!("{}: {}", doc.title, doc.detail);
    for e in &doc.errors {
        match &e.field {
            Some(field) => text.push_str(&format!("\n- {field}: {}", e.detail)),
            None => text.push_str(&format!("\n- {}", e.detail)),
        }
    }
    if let Some(current) = &doc.current {
        text.push_str("\nThe current version is in `current`: re-apply the change to it.");
        if let Some(v) = current.get("version") {
            text.push_str(&format!(" (version {v})"));
        }
    }
    if let Some(secs) = p.retry_after {
        text.push_str(&format!("\nRetry after {secs} s."));
    }
    let value = serde_json::to_value(&doc).unwrap_or(Value::Null);
    let mut result = CallToolResult::structured_error(value);
    result.content = vec![ContentBlock::text(text)];
    // The current item as JSON text too, for clients that pass only `content` to the
    // model: it needs it to re-apply its change.
    if let Some(current) = &doc.current {
        result.content.push(ContentBlock::text(current.to_string()));
    }
    result
}

/// An error result for input the tool refuses before calling core (`422 validation`,
/// with `field` when known).
pub fn invalid(field: Option<&str>, detail: impl Into<String>) -> CallToolResult {
    let detail = detail.into();
    from_problem(Problem::validation(detail.clone()).with_error(field, detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(r: &CallToolResult) -> String {
        r.content[0].as_text().unwrap().text.clone()
    }

    #[test]
    fn ok_has_summary_then_json() {
        let r = ok(json!({"a": 1}), "one item");
        assert_eq!(r.is_error, Some(false));
        assert_eq!(r.structured_content, Some(json!({"a": 1})));
        assert_eq!(text(&r), "one item");
        assert_eq!(r.content[1].as_text().unwrap().text, r#"{"a":1}"#);
    }

    #[test]
    fn errors_name_what_is_missing() {
        let r = from_core(CoreError::Forbidden("content:read"));
        assert_eq!(r.is_error, Some(true));
        assert_eq!(text(&r), "Forbidden: missing content:read");
        let doc = r.structured_content.unwrap();
        assert_eq!(doc["status"], 403);
        assert_eq!(doc["type"], "/api/v1/problems/forbidden");

        let r = from_core(CoreError::invalid_field("title", "title must not be blank"));
        assert_eq!(
            text(&r),
            "Invalid input: title must not be blank\n- title: title must not be blank"
        );

        let r = from_core(CoreError::RateLimited {
            retry_after_secs: 12,
        });
        assert!(text(&r).ends_with("Retry after 12 s."), "{}", text(&r));
        assert_eq!(r.structured_content.unwrap()["status"], 429);

        let r = from_problem(
            Problem::from(CoreError::Conflict { current_version: 4 })
                .with_current(json!({"version": 4})),
        );
        assert!(text(&r).contains("(version 4)"), "{}", text(&r));
        assert_eq!(r.content.len(), 2);
        assert_eq!(r.content[1].as_text().unwrap().text, r#"{"version":4}"#);
        assert_eq!(r.structured_content.unwrap()["current"]["version"], 4);
    }
}
