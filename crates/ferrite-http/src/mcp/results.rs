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

use crate::api::{extract::MAX_JSON_BODY, problem::Problem};
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
    if let Some(findings) = doc.findings.as_ref().and_then(Value::as_array) {
        text.push_str("\nFix these (content_check lists them with how to fix each):");
        for f in findings {
            let rule = f["rule"].as_str().unwrap_or("");
            let context = f["context"].as_str();
            text.push_str(&format!(
                "\n- {rule}: {}",
                crate::mcp::tools::quality::message(rule, context)
            ));
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
/// with `field` when known): [`bad_input`] as a result.
pub fn invalid(field: Option<&str>, detail: impl Into<String>) -> CallToolResult {
    from_problem(bad_input(field, detail))
}

/// The problem of input a tool refuses before calling core: `422 validation` naming
/// `field` (when known).
pub fn bad_input(field: Option<&str>, detail: impl Into<String>) -> Problem {
    let detail = detail.into();
    Problem::validation(detail.clone()).with_error(field, detail)
}

/// `Err` ([`bad_input`] naming `field`) when `value`, serialized as JSON, is larger than
/// the REST API's body cap ([`MAX_JSON_BODY`], 4 MiB). `/mcp` accepts bigger requests
/// (for base64 uploads), but a structured value a tool writes stays within what
/// `/api/v1` would take. Counts the bytes without keeping them.
pub fn within_json_cap(field: &str, value: &impl Serialize) -> Result<(), Problem> {
    /// Counts bytes and stops the serializer once past the cap.
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            if self.0 > MAX_JSON_BODY {
                return Err(std::io::Error::other("over the cap"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    match serde_json::to_writer(&mut count, value) {
        Ok(()) => Ok(()),
        Err(_) if count.0 > MAX_JSON_BODY => Err(bad_input(
            Some(field),
            format!(
                "{field} is more than {MAX_JSON_BODY} bytes as JSON (4 MiB, the REST API's \
                 limit). Send less at once"
            ),
        )),
        Err(e) => {
            tracing::error!(error = %e, field, "MCP input not serializable");
            Err(Problem::internal())
        }
    }
}

/// The summary of one page of a listing, without a final period: `"3 jobs"`, `"1 job"`,
/// with `"; more with next_cursor"` when `more`.
pub fn page_summary(n: usize, noun: &str, more: bool) -> String {
    format!(
        "{n} {noun}{}{}",
        if n == 1 { "" } else { "s" },
        if more { "; more with next_cursor" } else { "" }
    )
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
    fn page_summaries() {
        assert_eq!(page_summary(0, "job", false), "0 jobs");
        assert_eq!(page_summary(1, "job", false), "1 job");
        assert_eq!(
            page_summary(2, "audit row", true),
            "2 audit rows; more with next_cursor"
        );
    }

    #[test]
    fn json_cap_matches_the_rest_api() {
        assert!(within_json_cap("fields", &json!({"a": "b"})).is_ok());
        // Exactly at the cap is fine; one byte more is not.
        let at_cap = "x".repeat(MAX_JSON_BODY - 2);
        assert!(within_json_cap("fields", &json!(at_cap)).is_ok());
        let over = json!({"text": "x".repeat(MAX_JSON_BODY)});
        let p = within_json_cap("fields", &over).unwrap_err();
        let doc = p.document();
        assert_eq!(doc.status, 422);
        assert_eq!(doc.errors[0].field.as_deref(), Some("fields"));
        assert!(doc.detail.contains("4 MiB"), "{}", doc.detail);
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
