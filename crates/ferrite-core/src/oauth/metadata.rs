//! Client ID Metadata Documents (design 12 §3.2): a client whose `client_id` is an
//! `https` URL is described by the JSON document at that URL. Core validates the URL and
//! the document; the fetch itself (SSRF-checked egress, public addresses only, 5 s, at
//! most [`MAX_METADATA_DOCUMENT`] bytes, at most 3 `https` redirects) is the
//! [`ClientMetadataFetcher`] the HTTP crate implements.
//!
//! The client fields both kinds of client share (name, redirect URIs, `client_uri`,
//! `logo_uri`) are checked here too ([`client_fields`]).

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use super::redirect::{self, MAX_REDIRECT_URI};

/// Largest metadata document accepted, in bytes.
pub const MAX_METADATA_DOCUMENT: usize = 16 * 1024;
/// Longest client name kept, in characters (longer names are cut).
pub(crate) const MAX_CLIENT_NAME: usize = 100;
/// Most redirect URIs one client may register.
pub(crate) const MAX_REDIRECT_URIS: usize = 20;

/// Fetches a client's metadata document.
#[async_trait]
pub trait ClientMetadataFetcher: Send + Sync {
    /// The body at `url` (already checked to be an `https` URL with a path), at most
    /// [`MAX_METADATA_DOCUMENT`] bytes.
    async fn fetch(&self, url: &Url) -> Result<Vec<u8>, FetchError>;
}

/// Why a metadata document could not be fetched.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The egress rules refused the URL or a redirect (private address, not `https`, too
    /// many redirects).
    #[error("refused: {0}")]
    Refused(String),
    /// The body is larger than [`MAX_METADATA_DOCUMENT`].
    #[error("the document is larger than {MAX_METADATA_DOCUMENT} bytes")]
    TooLarge,
    /// Network error, timeout or a non-success status.
    #[error("failed: {0}")]
    Failed(String),
}

/// The checked fields of a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientFields {
    pub(crate) name: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) client_uri: Option<String>,
    pub(crate) logo_uri: Option<String>,
}

/// What is wrong with client fields: RFC 7591 tells a bad redirect URI apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldError {
    RedirectUri(String),
    Metadata(String),
}

impl FieldError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::RedirectUri(m) | Self::Metadata(m) => m,
        }
    }
}

/// The URL of a metadata-document client id: `https`, a host, a path other than `/`, no
/// fragment, no user information, at most 2000 bytes and already in normal form (so no
/// `.`/`..` segments or other spellings the parser would rewrite). `None` otherwise.
pub fn metadata_url(client_id: &str) -> Option<Url> {
    if client_id.len() > MAX_REDIRECT_URI {
        return None;
    }
    let url = Url::parse(client_id).ok()?;
    let ok = url.scheme() == "https"
        && url.host().is_some()
        && url.path() != "/"
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.as_str() == client_id;
    ok.then_some(url)
}

/// Whether `c` is a Unicode format character (general category `Cf`, Unicode 16): bidi
/// overrides and isolates, zero-width characters, soft hyphen, tags and the like, which
/// could make a client name read as something else on the consent screen (also used to
/// clean the site title the MCP server's instructions quote).
pub fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{AD}'
            | '\u{600}'..='\u{605}'
            | '\u{61C}'
            | '\u{6DD}'
            | '\u{70F}'
            | '\u{890}'..='\u{891}'
            | '\u{8E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// Checks the shared client fields: a name (trimmed, control and format characters
/// removed, cut to
/// [`MAX_CLIENT_NAME`]), 1 to [`MAX_REDIRECT_URIS`] redirect URIs
/// ([`redirect::check_registered`]), and optional `http(s)` `client_uri` and `logo_uri`
/// (a logo is kept only when `https`, design 12 §7).
pub(crate) fn client_fields(
    name: Option<&str>,
    redirect_uris: &[String],
    client_uri: Option<&str>,
    logo_uri: Option<&str>,
) -> Result<ClientFields, FieldError> {
    let name: String = name
        .unwrap_or_default()
        .chars()
        .filter(|&c| !c.is_control() && !is_format(c))
        .collect();
    let name: String = name.trim().chars().take(MAX_CLIENT_NAME).collect();
    if name.trim().is_empty() {
        return Err(FieldError::Metadata("client_name is required".into()));
    }
    if redirect_uris.is_empty() || redirect_uris.len() > MAX_REDIRECT_URIS {
        return Err(FieldError::RedirectUri(format!(
            "1 to {MAX_REDIRECT_URIS} redirect_uris are required"
        )));
    }
    for uri in redirect_uris {
        redirect::check_registered(uri).map_err(FieldError::RedirectUri)?;
    }
    let web = |field: &str, value: Option<&str>| -> Result<Option<Url>, FieldError> {
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            return Ok(None);
        };
        let url = Url::parse(value)
            .ok()
            .filter(|u| {
                value.len() <= MAX_REDIRECT_URI
                    && matches!(u.scheme(), "http" | "https")
                    && u.host().is_some()
            })
            .ok_or_else(|| FieldError::Metadata(format!("{field} must be an http(s) URL")))?;
        Ok(Some(url))
    };
    let client_uri = web("client_uri", client_uri)?.map(String::from);
    let logo_uri = web("logo_uri", logo_uri)?
        .filter(|u| u.scheme() == "https")
        .map(String::from);
    Ok(ClientFields {
        name,
        redirect_uris: redirect_uris.to_vec(),
        client_uri,
        logo_uri,
    })
}

/// Checks a fetched document for the client id `url`: at most [`MAX_METADATA_DOCUMENT`]
/// bytes of JSON, an object whose `client_id` is exactly `url`, with `client_name` and
/// `redirect_uris` ([`client_fields`]). Other members are ignored.
pub(crate) fn parse_document(url: &Url, bytes: &[u8]) -> Result<ClientFields, String> {
    if bytes.len() > MAX_METADATA_DOCUMENT {
        return Err(format!(
            "the document is larger than {MAX_METADATA_DOCUMENT} bytes"
        ));
    }
    let doc: Value =
        serde_json::from_slice(bytes).map_err(|_| "the document is not JSON".to_owned())?;
    let Value::Object(doc) = doc else {
        return Err("the document is not a JSON object".into());
    };
    if doc.get("client_id").and_then(Value::as_str) != Some(url.as_str()) {
        return Err("the document's client_id is not its own URL".into());
    }
    let text = |key: &str| -> Result<Option<&str>, String> {
        match doc.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(format!("{key} must be a string")),
        }
    };
    let name = text("client_name")?.ok_or("the document has no client_name")?;
    let uris = match doc.get("redirect_uris") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()
            .ok_or("redirect_uris must be strings")?,
        _ => return Err("the document has no redirect_uris".into()),
    };
    client_fields(Some(name), &uris, text("client_uri")?, text("logo_uri")?)
        .map_err(|e| e.message().to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const URL: &str = "https://app.example/oauth/client.json";

    fn parse(doc: serde_json::Value) -> Result<ClientFields, String> {
        parse_document(
            &Url::parse(URL).unwrap(),
            &serde_json::to_vec(&doc).unwrap(),
        )
    }

    #[test]
    fn metadata_document_must_name_itself() {
        let ok = parse(json!({
            "client_id": URL,
            "client_name": "  App\u{7}  ",
            "redirect_uris": ["http://127.0.0.1/cb"],
            "logo_uri": "http://app.example/logo.png",
            "client_uri": "https://app.example/",
            "extra": {"ignored": true},
        }));
        assert_eq!(
            ok,
            Ok(ClientFields {
                name: "App".into(),
                redirect_uris: vec!["http://127.0.0.1/cb".into()],
                client_uri: Some("https://app.example/".into()),
                // Only an https logo is kept.
                logo_uri: None,
            })
        );
        for bad in [
            json!({"client_id": "https://app.example/oauth/other.json", "client_name": "A", "redirect_uris": ["https://a.example/cb"]}),
            json!({"client_id": format!("{URL}/"), "client_name": "A", "redirect_uris": ["https://a.example/cb"]}),
            json!({"client_name": "A", "redirect_uris": ["https://a.example/cb"]}),
            json!({"client_id": URL, "redirect_uris": ["https://a.example/cb"]}),
            json!({"client_id": URL, "client_name": "A"}),
            json!({"client_id": URL, "client_name": "A", "redirect_uris": []}),
            json!({"client_id": URL, "client_name": "A", "redirect_uris": [1]}),
            json!({"client_id": URL, "client_name": "A", "redirect_uris": ["javascript:alert(1)"]}),
            json!({"client_id": URL, "client_name": 7, "redirect_uris": ["https://a.example/cb"]}),
            json!([URL]),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
        let url = Url::parse(URL).unwrap();
        assert!(parse_document(&url, b"not json").is_err());
        assert!(parse_document(&url, &vec![b' '; MAX_METADATA_DOCUMENT + 1]).is_err());
    }

    #[test]
    fn metadata_urls() {
        assert!(metadata_url(URL).is_some());
        assert!(metadata_url("https://app.example/client?v=1").is_some());
        for bad in [
            "http://app.example/client.json",
            "https://app.example",
            "https://app.example/",
            "https://app.example/a/../client.json",
            "https://app.example/./client.json",
            "https://APP.example/client.json",
            "https://user@app.example/client.json",
            "https://app.example/client.json#x",
            "ferc_abc",
            "",
        ] {
            assert!(metadata_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn names_are_cleaned_and_cut() {
        let uris = vec!["https://a.example/cb".to_owned()];
        let long = "é".repeat(300);
        let fields = client_fields(Some(&long), &uris, None, None).unwrap();
        assert_eq!(fields.name.chars().count(), MAX_CLIENT_NAME);
        assert!(client_fields(Some(" \t "), &uris, None, None).is_err());
        // Bidi overrides, isolates and zero-width characters go too.
        let tricky = "Good\u{202E}gnp.exe\u{2066}\u{200B} App\u{FEFF}\u{E0041}";
        let fields = client_fields(Some(tricky), &uris, None, None).unwrap();
        assert_eq!(fields.name, "Goodgnp.exe App");
        assert!(client_fields(Some("\u{200B}\u{202A}"), &uris, None, None).is_err());
        assert!(client_fields(None, &uris, None, None).is_err());
        assert!(matches!(
            client_fields(Some("A"), &[], None, None),
            Err(FieldError::RedirectUri(_))
        ));
        assert!(matches!(
            client_fields(Some("A"), &uris, Some("ftp://a.example/"), None),
            Err(FieldError::Metadata(_))
        ));
    }
}
