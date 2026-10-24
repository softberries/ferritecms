//! Bodies of the `/media` endpoints.
//!
//! Ids are UUID strings and times RFC 3339 UTC (`…Z`). URLs are what the public site
//! serves the file at (`MediaStore::public_url`: `/_media/<hash>/<name>`, or the CDN
//! URL in direct mode).

use chrono::{DateTime, Utc};
use ferrite_core::MediaItem;
use ferrite_domain::ArtifactHash;
use ferrite_media::MediaStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

/// A media item (`GET`, and every write's result).
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaDto {
    pub id: Uuid,
    pub title: String,
    /// Alternative text.
    pub alt: String,
    /// Decided from the content at upload, never from the client's claim.
    pub mime: String,
    pub bytes: i64,
    /// Set once the variants are made (raster images only).
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// The uploaded file's name (last path segment only).
    pub original_name: String,
    /// Where the original is served.
    pub url: String,
    /// The resized copies, by name (empty until they are made).
    pub variants: Vec<MediaVariantDto>,
    /// The stored bytes are the SVG sanitiser's output.
    pub sanitized: bool,
    /// `published` from the upload on; `trash` once trashed.
    pub status: String,
    pub author_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// What `ETag` carries and `If-Match` must send back.
    pub version: i64,
}

/// One resized copy of a media item.
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaVariantDto {
    /// The size's name (`thumbnail`, `medium`, `large`, or a theme's size).
    pub name: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub mime: String,
}

impl MediaDto {
    /// `m` with its URLs from `store`.
    pub fn new(m: MediaItem, store: &MediaStore) -> Self {
        let MediaItem { item, media } = m;
        let name = media.original_name;
        let mut variants: Vec<MediaVariantDto> = media
            .variants
            .as_object()
            .map(|map| {
                map.iter()
                    .filter_map(|(variant, v)| variant_dto(store, &name, variant, v))
                    .collect()
            })
            .unwrap_or_default();
        variants.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            id: item.id.as_uuid(),
            title: item.title,
            alt: media.alt,
            mime: media.mime,
            bytes: media.bytes,
            width: media.width,
            height: media.height,
            url: store.public_url(&media.artifact_hash, &name),
            original_name: name,
            variants,
            sanitized: media.sanitized,
            status: item.status.as_str().to_owned(),
            author_id: item.author_id.as_uuid(),
            created_at: item.created_at.as_datetime(),
            updated_at: item.updated_at.as_datetime(),
            version: item.version,
        }
    }
}

/// One entry of the row's `variants` (`name -> {hash, w, h, mime}`); a malformed one is
/// left out.
fn variant_dto(store: &MediaStore, file: &str, name: &str, v: &Value) -> Option<MediaVariantDto> {
    let hash = ArtifactHash::parse(v.get("hash")?.as_str()?).ok()?;
    let dimension = |key: &str| v.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok());
    Some(MediaVariantDto {
        name: name.to_owned(),
        url: store.public_url(&hash, file),
        width: dimension("w")?,
        height: dimension("h")?,
        mime: v.get("mime")?.as_str()?.to_owned(),
    })
}

/// `GET /media`.
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaList {
    pub items: Vec<MediaDto>,
    /// Pass as `cursor` for the next page; `null` on the last page.
    pub next_cursor: Option<String>,
}

/// `PATCH /media/{id}`: the members sent are changed, the others kept. Unknown members
/// are ignored.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MediaPatchInput {
    /// At most 200 characters; blank keeps an empty title.
    #[serde(default)]
    pub title: Option<String>,
    /// At most 1 000 characters.
    #[serde(default)]
    pub alt: Option<String>,
}

/// `POST /media` (`multipart/form-data`; the schema only). Parts in any order; an
/// unknown or repeated one is refused.
#[derive(ToSchema)]
pub struct MediaUpload {
    /// The file, streamed (at most `media.max_upload`; its part's file name is kept as
    /// `original_name`). Its type is decided from its content.
    #[schema(value_type = String, format = Binary)]
    pub file: Vec<u8>,
    /// At most 200 characters; absent or blank: the file name without its extension.
    pub title: Option<String>,
    /// At most 1 000 characters.
    pub alt: Option<String>,
}
