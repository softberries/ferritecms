//! Host-side mirror of the `ferrite:plugin@0.1.0` WIT types.
//!
//! These types do not depend on generated bindings; conversions live in `bindings.rs`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermRef {
    pub taxonomy: String,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub kind: String,
    pub slug: String,
    pub title: String,
    pub excerpt: String,
    pub status: String,
    /// RFC 3339 timestamp.
    pub published: Option<String>,
    pub url: String,
    pub author: String,
    pub terms: Vec<TermRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaTag {
    pub name: Option<String>,
    pub property: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTag {
    pub rel: String,
    pub href: String,
    pub hreflang: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptTag {
    pub src: Option<String>,
    pub json_ld: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    pub title: Option<String>,
    pub metas: Vec<MetaTag>,
    pub links: Vec<LinkTag>,
    pub scripts: Vec<ScriptTag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterValue {
    Head(Head),
    Html(String),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestInfo {
    pub path: String,
    pub locale: String,
    pub is_logged_in: bool,
    pub post: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionEvent {
    pub subject: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRequest {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub client_ip_hash: String,
    pub user: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::v0_1::ferrite::plugin::types as wit;

    #[test]
    fn filter_value_roundtrips_through_wit_types() {
        let original = FilterValue::Head(Head {
            title: Some("T".into()),
            metas: vec![MetaTag {
                name: Some("description".into()),
                property: None,
                content: "d".into(),
            }],
            ..Head::default()
        });
        let generated: wit::FilterValue = original.clone().into();
        assert_eq!(FilterValue::from(generated), original);
    }
}
