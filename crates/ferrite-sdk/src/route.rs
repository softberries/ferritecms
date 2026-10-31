//! Response builders for plugin route handlers.

use crate::RouteResponse;

fn with_type(status: u16, content_type: &str, body: &str) -> RouteResponse {
    RouteResponse {
        status,
        headers: vec![("Content-Type".to_owned(), content_type.to_owned())],
        body: body.as_bytes().to_vec(),
    }
}

/// An HTML response (`Content-Type: text/html; charset=utf-8`).
pub fn html(status: u16, body: &str) -> RouteResponse {
    with_type(status, "text/html; charset=utf-8", body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_sets_type_status_and_body() {
        let r = html(422, "<p>no</p>");
        assert_eq!(r.status, 422);
        assert_eq!(
            r.headers,
            vec![(
                "Content-Type".to_owned(),
                "text/html; charset=utf-8".to_owned()
            )]
        );
        assert_eq!(r.body, b"<p>no</p>");
    }
}
