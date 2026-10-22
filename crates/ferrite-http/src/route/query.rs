//! The normalised query string: the only parameters that reach routing or cache keys.

use ferrite_core::permalink::encode_segment;

use super::MAX_PAGE;
use super::path::{decode_form, number};

/// The allowlisted query parameters of a request; everything else is ignored.
///
/// - `s`: the search terms, form-decoded (`+` is a space) and trimmed. Absent when
///   empty, malformed (bad escape, invalid UTF-8) or containing a control character.
/// - `page`: a number in `2..=1000` without a leading zero; `1` is the same page as no
///   parameter, so it is dropped. Routing paginates by path (`/page/N/`) and does not
///   read it; it is kept only so cache keys follow the allowlist contract.
///
/// The first occurrence of a parameter wins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalQuery {
    pub s: Option<String>,
    pub page: Option<u32>,
}

impl NormalQuery {
    pub fn parse(raw: Option<&str>) -> Self {
        let mut q = Self::default();
        let (mut seen_s, mut seen_page) = (false, false);
        for pair in raw.unwrap_or("").split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "s" if !seen_s => {
                    seen_s = true;
                    q.s = decode_form(value)
                        .map(|v| v.trim().to_owned())
                        .filter(|v| !v.is_empty());
                }
                "page" if !seen_page => {
                    seen_page = true;
                    q.page = number(value, MAX_PAGE).filter(|n| *n > 1);
                }
                _ => {}
            }
        }
        q
    }

    /// The parameters in canonical form for a cache key: sorted by name, values
    /// percent-encoded like path segments (`page=2&s=a%20b`); empty when there are none.
    pub fn cache_query(&self) -> String {
        let mut parts = Vec::new();
        if let Some(page) = self.page {
            parts.push(format!("page={page}"));
        }
        if let Some(s) = &self.s {
            parts.push(format!("s={}", encode_segment(s)));
        }
        parts.join("&")
    }
}
