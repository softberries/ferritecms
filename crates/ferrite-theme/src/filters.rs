//! Custom filters (06 §4): `date`, `excerpt`, `slugify`, `json`.

use std::fmt::Write as _;

use chrono::{DateTime, format::StrftimeItems};
use minijinja::{Error, ErrorKind, Value};

/// Default `date` format.
const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%d";
/// Longest `date` format string accepted.
const MAX_FORMAT_LEN: usize = 200;
/// Default word count of `excerpt` (WordPress's).
const DEFAULT_EXCERPT_WORDS: usize = 55;

/// `{{ post.date | date("%e %B %Y") }}`: an RFC 3339 timestamp formatted with chrono
/// strftime, in UTC. `none`/undefined give `""`. An invalid format or timestamp is a
/// template error, never a panic: formatting goes through the fallible `write!` path,
/// and an unknown `%` sequence makes it fail.
pub(crate) fn date(value: &Value, fmt: Option<&str>) -> Result<String, Error> {
    if value.is_none() || value.is_undefined() {
        return Ok(String::new());
    }
    let fmt = fmt.unwrap_or(DEFAULT_DATE_FORMAT);
    if fmt.len() > MAX_FORMAT_LEN {
        return Err(invalid("date: format string too long"));
    }
    let raw = value
        .as_str()
        .ok_or_else(|| invalid("date: expected an RFC 3339 timestamp string"))?;
    let dt = DateTime::parse_from_rfc3339(raw)
        .map_err(|_| invalid("date: expected an RFC 3339 timestamp string"))?
        .to_utc();
    let mut out = String::new();
    write!(out, "{}", dt.format_with_items(StrftimeItems::new(fmt)))
        .map_err(|_| invalid("date: invalid format string"))?;
    Ok(out)
}

/// `{{ post.body | excerpt(30) }}`: tags stripped, entities decoded, whitespace
/// collapsed, at most `n` words (default 55) with `…` appended when cut. Returns
/// plain text, so auto-escaping applies.
pub(crate) fn excerpt(value: &Value, n: Option<usize>) -> String {
    let n = n.unwrap_or(DEFAULT_EXCERPT_WORDS);
    let text = strip_tags(&value.to_string());
    let text = html_escape::decode_html_entities(&text);
    let mut words = text.split_whitespace();
    let mut out = words.by_ref().take(n).collect::<Vec<_>>().join(" ");
    if words.next().is_some() {
        out.push('…');
    }
    out
}

/// `{{ title | slugify }}`: core's slug function.
pub(crate) fn slugify(value: &Value) -> String {
    ferrite_core::slug::slugify(&value.to_string())
}

/// `{{ data | json }}`: compact JSON with `<`, `>`, `&`, `'`, U+2028 and U+2029
/// escaped as `\uXXXX`, so it is safe inside `<script>` and single-quoted attributes.
/// Marked safe. (The builtin `tojson` escapes the first four but not U+2028/U+2029.)
pub(crate) fn json(value: &Value) -> Result<Value, Error> {
    let s = serde_json::to_string(value)
        .map_err(|e| invalid("json: cannot serialize").with_source(e))?;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\'' => out.push_str("\\u0027"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    Ok(Value::from_safe_string(out))
}

fn invalid(msg: &'static str) -> Error {
    Error::new(ErrorKind::InvalidOperation, msg)
}

/// Drops everything between `<` and `>`; a tag boundary counts as whitespace.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                out.push(' ');
            }
            (true, '>') => in_tag = false,
            (false, c) => out.push(c),
            (true, _) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_formats_and_rejects() {
        let v = Value::from("2024-03-05T10:20:30+00:00");
        assert_eq!(date(&v, None).unwrap(), "2024-03-05");
        assert_eq!(
            date(&v, Some("%e %B %Y, %H:%M")).unwrap(),
            " 5 March 2024, 10:20"
        );
        // Offsets are converted to UTC.
        let v2 = Value::from("2024-03-05T01:00:00+02:00");
        assert_eq!(date(&v2, Some("%Y-%m-%d %H")).unwrap(), "2024-03-04 23");
        for bad in ["%Q", "%", "%Y %!", "%.3"] {
            assert!(date(&v, Some(bad)).is_err(), "{bad}");
        }
        assert!(date(&Value::from("yesterday"), None).is_err());
        assert!(date(&Value::from(5), None).is_err());
        assert_eq!(date(&Value::from(()), None).unwrap(), "");
        assert_eq!(date(&Value::UNDEFINED, None).unwrap(), "");
    }

    #[test]
    fn excerpt_strips_and_truncates() {
        let v = Value::from("<p>Hello <b>wide</b>&nbsp;world &amp; more</p><p>zażółć</p>");
        assert_eq!(excerpt(&v, Some(3)), "Hello wide world…");
        assert_eq!(excerpt(&v, Some(10)), "Hello wide world & more zażółć");
        assert_eq!(excerpt(&Value::from("a<br>b"), None), "a b");
        assert_eq!(excerpt(&Value::from("one two"), Some(2)), "one two");
    }

    #[test]
    fn slugify_uses_core() {
        assert_eq!(slugify(&Value::from("Hello, Wörld!")), "hello-world");
    }

    #[test]
    fn json_is_script_safe() {
        let v = Value::from("</script><b>&'\u{2028}");
        let out = json(&v).unwrap();
        assert!(out.is_safe());
        assert_eq!(
            out.as_str().unwrap(),
            "\"\\u003c/script\\u003e\\u003cb\\u003e\\u0026\\u0027\\u2028\""
        );
    }
}
