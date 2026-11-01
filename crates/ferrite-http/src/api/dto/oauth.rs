//! The consent API's bodies (`/api/v1/oauth`, design 12 §6, §8).

use chrono::{DateTime, Utc};
use ferrite_core::oauth::{ConsentView, GrantView};
use ferrite_domain::{ClientKind, OAuthClient};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// How a client is known.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClientKindDto {
    /// By its Client ID Metadata Document (its `id` is the document's `https` URL):
    /// "verified".
    Metadata,
    /// Self-registered with `POST /oauth/register` (`ferc_…`).
    Registered,
}

impl From<ClientKind> for ClientKindDto {
    fn from(k: ClientKind) -> Self {
        match k {
            ClientKind::Metadata => Self::Metadata,
            ClientKind::Registered => Self::Registered,
        }
    }
}

/// An OAuth client as the consent screen and the connected apps show it (no redirect
/// URIs).
#[derive(Debug, Serialize, ToSchema)]
pub struct OAuthClientDto {
    /// The `client_id`: `ferc_…`, or the metadata document's URL.
    pub id: String,
    pub kind: ClientKindDto,
    /// Control and format characters removed, at most 100 characters; written by the
    /// client, so show it as text.
    pub name: String,
    /// The client's home page (`http(s)`), as it declared.
    pub client_uri: Option<String>,
    /// The client's logo; only ever an `https` URL.
    pub logo_uri: Option<String>,
}

impl From<OAuthClient> for OAuthClientDto {
    fn from(c: OAuthClient) -> Self {
        Self {
            id: c.id.to_string(),
            kind: c.kind.into(),
            name: c.name,
            client_uri: c.client_uri,
            logo_uri: c.logo_uri,
        }
    }
}

/// `GET /oauth/requests/{id}`: a pending authorization request (10 minutes) as the
/// consent screen shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ConsentDto {
    /// The request's id (the `request` query parameter of `/admin/connect`).
    pub id: Uuid,
    pub client: OAuthClientDto,
    /// `true` when the client is known by a metadata document, `false` when it
    /// registered itself.
    pub verified: bool,
    /// Where the browser goes back to: the host of an `http(s)` redirect URI,
    /// `scheme://host` for another scheme, the whole URI when it has no host.
    pub redirect_host: String,
    /// The requested scopes (all 22 when the client named none).
    pub requested: Vec<String>,
    /// The requested scopes this user can grant (preselect these); `<area>:write`
    /// implies `<area>:read`.
    pub grantable: Vec<String>,
}

impl ConsentDto {
    pub fn new(id: Uuid, v: ConsentView) -> Self {
        Self {
            id,
            client: v.client.into(),
            verified: v.verified,
            redirect_host: v.redirect_host,
            requested: v.requested,
            grantable: v.grantable,
        }
    }
}

/// `POST /oauth/requests/{id}/approve`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApproveInput {
    /// At least one, each in the request's `grantable`.
    pub scopes: Vec<String>,
}

/// Where the browser goes next (approve and deny): the client's redirect URI with
/// `code` (or `error=access_denied`), `state` and `iss`. Navigate there with
/// `window.location`; never fetch it.
#[derive(Debug, Serialize, ToSchema)]
pub struct RedirectDto {
    pub redirect_to: String,
}

/// A connected app: one grant of a user to a client.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrantDto {
    pub id: Uuid,
    pub user_id: Uuid,
    pub client: OAuthClientDto,
    /// The granted area scopes.
    pub scopes: Vec<String>,
    /// When the user connected the app.
    pub created_at: DateTime<Utc>,
    /// When the app last refreshed its access (about hourly while in use).
    pub last_used_at: DateTime<Utc>,
    /// When the connection ends unless used again (30 days after its last use).
    pub expires_at: DateTime<Utc>,
}

impl From<GrantView> for GrantDto {
    fn from(v: GrantView) -> Self {
        let g = v.grant;
        Self {
            id: g.id.as_uuid(),
            user_id: g.user_id.as_uuid(),
            client: v.client.into(),
            scopes: g.scopes,
            created_at: g.created_at.as_datetime(),
            last_used_at: g.last_used_at.as_datetime(),
            expires_at: g.expires_at.as_datetime(),
        }
    }
}

/// `GET /oauth/grants`: a user's connected apps, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct GrantList {
    pub items: Vec<GrantDto>,
}

/// `GET /oauth/grants` query.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct GrantParams {
    /// Another user's id (needs `manage_users`); default: the request's own user.
    pub user: Option<Uuid>,
}
