//! Media tools (design 12 §5.2): `media_list`, `media_get`, `media_upload`,
//! `media_update`, `media_trash` and `media_restore`.
//!
//! The tools are adapters over the REST layer's pieces ([`crate::api::media`]: the DTOs,
//! the list query and its cursor check) and [`MediaService`](ferrite_core::MediaService);
//! core checks every permission.
//!
//! * `media_upload` takes exactly one source. `data_base64` is decoded here (the encoded
//!   length is checked before decoding) and staged by [`Uploader::stage_bytes`](crate::media::Uploader::stage_bytes); `url` is
//!   fetched by [`Uploader::stage_url`](crate::media::Uploader::stage_url) with the importer's SSRF-checked client (`https`
//!   only, public addresses, 30 s) and cut at the same size cap while it streams. Both go
//!   through the node's upload slots, type sniffing and SVG sanitising, and
//!   `media.max_upload` still applies. The caller is authorized before anything is decoded
//!   or fetched.
//! * A media item is returned with its absolute `url` and the `ferrite:media/<id>`
//!   reference to use in Markdown. Dimensions and resized copies appear once the
//!   `media.variants` job has run (raster images only).
//! * A write takes the `version` read last; a stale one is an error result whose
//!   `current` holds the item as stored now (the REST 412 rule).

use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use ferrite_core::{
    Actor, CoreError, MediaItem, MediaPatch, MediaQuery, NewUpload, check_file_name,
};
use ferrite_domain::{ContentId, MediaKind, UserId};
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::per_page;
use crate::{
    api::{
        dto::media::MediaDto,
        media::{check_cursor, dto, dtos},
        problem::Problem,
    },
    mcp::{Caller, FerriteMcp, results},
    media::UploadError,
};

/// The largest file `media_upload` takes, decoded or fetched: 6 MiB (design 12 §5.2).
pub const MAX_UPLOAD_BYTES: u64 = 6 << 20;

// ---- inputs ------------------------------------------------------------------------------

/// The kind of file to list.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Pictures, SVG included.
    Image,
    Video,
    Audio,
    /// Everything else: PDFs, office documents, archives.
    Document,
}

impl From<Kind> for MediaKind {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Image => Self::Image,
            Kind::Video => Self::Video,
            Kind::Audio => Self::Audio,
            Kind::Document => Self::Document,
        }
    }
}

/// The input of `media_list`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// Words to look for in titles (not file names). Best matches first; cannot be
    /// combined with `trash` or `author`.
    #[serde(default)]
    pub q: Option<String>,
    /// Only this kind of file.
    #[serde(default)]
    pub kind: Option<Kind>,
    /// `true`: only trashed items (to find something to media_restore). Default `false`:
    /// every item but the trashed ones.
    #[serde(default)]
    pub trash: bool,
    /// Only files uploaded by this user id.
    #[serde(default)]
    pub author: Option<Uuid>,
    /// Items per page, 1 to 100 (default 20).
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub per_page: Option<u32>,
    /// `next_cursor` of the previous page of the same query, to get the next page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// A media item named by id (`media_get`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdInput {
    /// The media item's id.
    pub id: Uuid,
}

/// The input of `media_upload`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadInput {
    /// The file's bytes as standard base64 (RFC 4648, with `=` padding, no line breaks
    /// and no `data:` prefix), at most 6 MiB decoded. Send exactly one of `data_base64`
    /// and `url`. Needs `filename`.
    #[serde(default)]
    pub data_base64: Option<String>,
    /// An `https` URL to download the file from, at most 6 MiB. Only public internet
    /// addresses work (no private networks, no `http`, no credentials in the URL); the
    /// download may take 30 s. Send exactly one of `url` and `data_base64`.
    #[serde(default)]
    pub url: Option<String>,
    /// The file name with its extension, for example `team-photo.jpg`. Required with
    /// `data_base64`; with `url` it defaults to the last part of the URL's path. The
    /// real file type is decided from the content, not from the name.
    #[serde(default)]
    pub filename: Option<String>,
    /// The title (at most 200 characters). Default: the file name without its extension.
    #[serde(default)]
    pub title: Option<String>,
    /// Alternative text for images (at most 1 000 characters): describe what the picture
    /// shows, for readers who cannot see it.
    #[serde(default)]
    pub alt: Option<String>,
}

/// The input of `media_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The media item's id.
    pub id: Uuid,
    /// The `version` you last read. If someone changed the item since, nothing is
    /// written and the current item is returned so you can re-apply your change.
    #[schemars(range(min = 1))]
    pub version: i64,
    /// A new title (at most 200 characters). Leave out to keep it.
    #[serde(default)]
    pub title: Option<String>,
    /// New alternative text (at most 1 000 characters); `""` clears it. Leave out to
    /// keep it.
    #[serde(default)]
    pub alt: Option<String>,
}

/// A media item at the version the caller read (`media_trash`, `media_restore`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VersionedInput {
    /// The media item's id.
    pub id: Uuid,
    /// The `version` you last read.
    #[schemars(range(min = 1))]
    pub version: i64,
}

// ---- outputs -----------------------------------------------------------------------------

/// A media item as the tools return it.
#[derive(Debug, Serialize)]
pub struct MediaOut {
    #[serde(flatten)]
    pub media: MediaDto,
    /// What to put in Markdown: `![alt](ferrite:media/<id>)` (images only).
    pub reference: String,
    /// The Markdown image for this item, `![alt](ferrite:media/<id>)`; empty for files
    /// that are not images (link to `url` instead).
    pub markdown: String,
}

/// One page of `media_list`.
#[derive(Debug, Serialize)]
pub struct ListOut {
    pub items: Vec<MediaOut>,
    /// Pass as `cursor` for the next page; `null` on the last one.
    pub next_cursor: Option<String>,
}

// ---- tools -------------------------------------------------------------------------------

#[tool_router(router = media_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists media files (images, video, audio, documents), newest change first, by
    /// default without trashed ones. Filter by `kind`, by title words (`q`, best matches
    /// first), by uploader, or `trash: true` to see what media_restore can bring back;
    /// page with `cursor`. Each item has its id, title, alt text, mime type, size, URL
    /// and the Markdown reference to put in a post body.
    #[tool(annotations(title = "List media", read_only_hint = true, open_world_hint = false))]
    pub async fn media_list(
        &self,
        caller: Caller,
        Parameters(input): Parameters<ListInput>,
    ) -> CallToolResult {
        let limit = match per_page(input.per_page) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let run = async {
            let actor = caller.actor();
            let query = MediaQuery {
                limit,
                cursor: input.cursor,
                author: input.author.map(UserId::from_uuid),
                q: input.q,
                kind: input.kind.map(MediaKind::from),
                trash: input.trash,
            };
            check_cursor(&query)?;
            let page = self.api.media_service.list(actor, query).await?;
            let items = dtos(&self.api, actor, page.items)
                .await?
                .into_iter()
                .map(|m| self.media_out(m))
                .collect();
            Ok::<_, Problem>(ListOut {
                items,
                next_cursor: page.next_cursor,
            })
        };
        match run.await {
            Ok(out) => {
                let n = out.items.len();
                let more = if out.next_cursor.is_some() {
                    "; more with next_cursor"
                } else {
                    ""
                };
                let summary = format!("{n} media item{}{more}.", if n == 1 { "" } else { "s" });
                results::ok(out, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Returns one media item: title, alt text, mime type, size, width and height (once
    /// known), URL, resized copies (`variants`), the uploader, `version` (needed for
    /// media_update, media_trash and media_restore) and the Markdown reference to use in
    /// a post body.
    #[tool(annotations(title = "Get media", read_only_hint = true, open_world_hint = false))]
    pub async fn media_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<IdInput>,
    ) -> CallToolResult {
        let actor = caller.actor();
        let id = ContentId::from_uuid(input.id);
        match self.media_item(actor, id).await {
            Ok(out) => {
                let summary = media_summary(&out);
                results::ok(out, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Adds a file to the media library from base64 `data_base64` (with `filename`) or
    /// from a public `https` `url`, at most 6 MiB. It is published at once (media have
    /// no draft). Returns the item with its `id`, `url` and `markdown`: put
    /// `![describe the picture](ferrite:media/<id>)` in a post body or use the id as a
    /// featured image. Always give `alt` for pictures. The type is decided from the file
    /// content: SVGs are sanitised, types the site does not accept are refused. Sizes
    /// and resized copies appear a moment later. The URL is fetched from the site's
    /// server, never from private networks.
    #[tool(annotations(
        title = "Upload media",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = true
    ))]
    pub async fn media_upload(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UploadInput>,
    ) -> CallToolResult {
        match self.upload_media(&caller, input).await {
            Ok((out, source)) => {
                let summary = format!(
                    "Uploaded {} from {source}. Reference it in Markdown as \
                     ferrite:media/{}.",
                    out.label, out.id
                );
                results::ok(out.value, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Changes a media item's title or alternative text (the file itself cannot change).
    /// Send the `version` from your last read: if someone saved since, nothing is
    /// written and the error carries the current item (`current`). Alt text is what
    /// screen readers and search engines use, so describe what the picture shows.
    /// Returns the updated item.
    #[tool(annotations(
        title = "Update media",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn media_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UpdateInput>,
    ) -> CallToolResult {
        if input.title.is_none() && input.alt.is_none() {
            return results::invalid(None, "send a title, an alt text, or both");
        }
        let actor = caller.actor();
        let id = ContentId::from_uuid(input.id);
        let patch = MediaPatch {
            title: input.title,
            alt: input.alt,
        };
        match self
            .api
            .media_service
            .update(actor, id, input.version, patch)
            .await
        {
            Ok(m) => {
                self.audit(
                    &caller,
                    "media_update",
                    "media",
                    Some(id.to_string()),
                    json!({"version": m.item.version}),
                )
                .await;
                let out = self.media_written(actor, m).await;
                results::ok(out.value, format!("Updated {}.", out.label))
            }
            Err(e) => results::from_problem(self.media_problem(actor, id, e).await),
        }
    }

    /// Moves a media item to the trash: its file stops being served, so pages that show
    /// it lose the picture. It can be brought back with media_restore, but ask the user
    /// first. Send the `version` you last read.
    #[tool(annotations(
        title = "Trash media",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn media_trash(
        &self,
        caller: Caller,
        Parameters(input): Parameters<VersionedInput>,
    ) -> CallToolResult {
        let actor = caller.actor();
        let id = ContentId::from_uuid(input.id);
        match self.api.media_service.trash(actor, id, input.version).await {
            Ok(item) => {
                self.audit(
                    &caller,
                    "media_trash",
                    "media",
                    Some(id.to_string()),
                    json!({"version": item.version}),
                )
                .await;
                let value = json!({
                    "id": item.id.as_uuid(),
                    "title": item.title,
                    "status": item.status.as_str(),
                    "version": item.version,
                    "note": "In the trash; media_restore brings it back.",
                });
                results::ok(value, format!("Trashed {:?}.", item.title))
            }
            Err(e) => results::from_problem(self.media_problem(actor, id, e).await),
        }
    }

    /// Brings a trashed media item back: it is served again (published) unless it was a
    /// draft when trashed. Find trashed items with media_list and `trash: true`. Send the
    /// `version` you last read. Returns the item.
    #[tool(annotations(
        title = "Restore media from the trash",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn media_restore(
        &self,
        caller: Caller,
        Parameters(input): Parameters<VersionedInput>,
    ) -> CallToolResult {
        let actor = caller.actor();
        let id = ContentId::from_uuid(input.id);
        match self
            .api
            .media_service
            .restore(actor, id, input.version)
            .await
        {
            Ok(m) => {
                self.audit(
                    &caller,
                    "media_restore",
                    "media",
                    Some(id.to_string()),
                    json!({"version": m.item.version}),
                )
                .await;
                let out = self.media_written(actor, m).await;
                results::ok(out.value, format!("Restored {}.", out.label))
            }
            Err(e) => results::from_problem(self.media_problem(actor, id, e).await),
        }
    }
}

/// A committed write's answer.
struct Written {
    value: serde_json::Value,
    /// `"title" (status, version n)` for the summary.
    label: String,
    id: Uuid,
}

impl FerriteMcp {
    /// `url` as an absolute address: the media store serves originals under the site's
    /// own path (`/_media/...`) unless a CDN URL is configured.
    fn absolute(&self, url: String) -> String {
        if url.starts_with('/') {
            format!("{}{url}", self.api.site_url)
        } else {
            url
        }
    }

    /// `m` with absolute URLs and the Markdown reference.
    fn media_out(&self, mut m: MediaDto) -> MediaOut {
        m.url = self.absolute(std::mem::take(&mut m.url));
        for v in &mut m.variants {
            v.url = self.absolute(std::mem::take(&mut v.url));
        }
        let reference = format!("ferrite:media/{}", m.id);
        let markdown = if m.mime.starts_with("image/") {
            format!("![{}]({reference})", m.alt.replace(['[', ']', '\n'], " "))
        } else {
            String::new()
        };
        MediaOut {
            media: m,
            reference,
            markdown,
        }
    }

    async fn media_item(&self, actor: &Actor, id: ContentId) -> Result<MediaOut, Problem> {
        let m = self.api.media_service.get(actor, id).await?;
        Ok(self.media_out(dto(&self.api, actor, m).await?))
    }

    /// The answer of a write that is committed: the item as the tools return it, or, if
    /// that cannot be built, a minimal record with a pointer to media_get. Never an error.
    async fn media_written(&self, actor: &Actor, m: MediaItem) -> Written {
        let (id, title, status, version) = (
            m.item.id.as_uuid(),
            m.item.title.clone(),
            m.item.status.as_str().to_owned(),
            m.item.version,
        );
        let label = format!("{title:?} ({status}, version {version})");
        let built = async {
            let m = dto(&self.api, actor, m)
                .await
                .map_err(|p| p.document().detail)?;
            serde_json::to_value(self.media_out(m)).map_err(|e| e.to_string())
        }
        .await;
        let value = built.unwrap_or_else(|reason| {
            tracing::warn!(%reason, item = %id, "media item not built after a done write");
            json!({
                "id": id, "title": title, "status": status, "version": version,
                "reference": format!("ferrite:media/{id}"),
                "note": "The change was saved, but the full item could not be built; \
                         call media_get for it.",
            })
        });
        Written { value, label, id }
    }

    /// The problem of a failed write to item `id`: a stale version carries the item as
    /// stored now in `current`.
    async fn media_problem(&self, actor: &Actor, id: ContentId, e: CoreError) -> Problem {
        let conflict = matches!(e, CoreError::Conflict { .. });
        let problem = Problem::from(e);
        if !conflict {
            return problem;
        }
        let current = async {
            let out = self.media_item(actor, id).await?;
            serde_json::to_value(out).map_err(|_| Problem::internal())
        }
        .await;
        match current {
            Ok(value) => problem.with_current(value),
            // Gone or unreadable since: the error stays without `current`.
            Err(_) => problem,
        }
    }

    /// Uploads by `data_base64` or `url`; the item and a phrase for the summary.
    async fn upload_media(
        &self,
        caller: &Caller,
        input: UploadInput,
    ) -> Result<(Written, &'static str), Problem> {
        let api = &self.api;
        let actor = caller.actor();
        // Before anything is decoded or fetched.
        api.media_service.authorize_upload(actor)?;
        let UploadInput {
            data_base64,
            url,
            filename,
            title,
            alt,
        } = input;
        let cap = MAX_UPLOAD_BYTES.min(api.max_upload);
        let (staged, source, host) = match (data_base64, url) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(bad(None, "send exactly one of data_base64 and url"));
            }
            (Some(data), None) => {
                let name = filename.ok_or_else(|| {
                    bad(Some("filename"), "filename is required with data_base64")
                })?;
                check_file_name(&name)?;
                let bytes = decode_base64(&data, cap)?;
                let staged = api.uploader.stage_bytes(bytes, &name, cap).await?;
                (staged, "base64 data", None)
            }
            (None, Some(raw)) => {
                let parsed = api
                    .uploader
                    .check_fetch_url(&raw)
                    .map_err(|why| bad(Some("url"), why))?;
                let name = match filename {
                    Some(n) => n,
                    None => name_from_url(&parsed),
                };
                check_file_name(&name).map_err(|_| {
                    bad(
                        Some("filename"),
                        "the URL gives no usable file name; send filename",
                    )
                })?;
                let host = parsed.host_str().map(str::to_owned);
                let staged = api.uploader.stage_url(parsed.as_str(), &name, cap).await?;
                (staged, "the URL", host)
            }
        };
        let item = api
            .media_service
            .record_upload(
                actor,
                NewUpload {
                    artifact: staged.artifact.clone(),
                    mime: staged.mime.clone(),
                    bytes: staged.bytes,
                    original_name: staged.original_name.clone(),
                    title,
                    alt: alt.unwrap_or_default(),
                    sanitized: staged.sanitized,
                    raster: staged.raster,
                },
            )
            .await?;
        // The slot is released once the upload is recorded.
        drop(staged);
        let mut details = json!({"mime": item.media.mime, "bytes": item.media.bytes});
        if let Some(host) = host {
            details["source_host"] = json!(host);
        }
        self.audit(
            caller,
            "media_upload",
            "media",
            Some(item.item.id.to_string()),
            details,
        )
        .await;
        Ok((self.media_written(actor, item).await, source))
    }
}

/// `422 validation` naming `field` (when known).
fn bad(field: Option<&str>, detail: impl Into<String>) -> Problem {
    let detail = detail.into();
    Problem::validation(detail.clone()).with_error(field, detail)
}

/// The summary line of a media item.
fn media_summary(out: &MediaOut) -> String {
    let m = &out.media;
    let md = if out.markdown.is_empty() {
        format!("link: {}", m.url)
    } else {
        format!("Markdown: {}", out.markdown)
    };
    format!(
        "{:?} ({}, {} bytes, id {}, version {}). {md}",
        m.title, m.mime, m.bytes, m.id, m.version
    )
}

/// The decoded `data`, at most `cap` bytes. The encoded length is checked first, so an
/// over-long text is never decoded.
fn decode_base64(data: &str, cap: u64) -> Result<Bytes, Problem> {
    let too_large = || Problem::from(UploadError::TooLarge { limit: cap });
    let longest = usize::try_from(cap.div_ceil(3).saturating_mul(4)).unwrap_or(usize::MAX);
    if data.len() > longest {
        return Err(too_large());
    }
    let bytes = STANDARD.decode(data.trim()).map_err(|_| {
        bad(
            Some("data_base64"),
            "not valid standard base64 (RFC 4648 with = padding, no line breaks, no data: prefix)",
        )
    })?;
    if bytes.len() as u64 > cap {
        return Err(too_large());
    }
    Ok(Bytes::from(bytes))
}

/// The last non-empty path segment of `url`, percent-decoded; `download` when it has
/// none.
fn name_from_url(url: &url::Url) -> String {
    url.path_segments()
        .and_then(|mut s| s.rfind(|seg| !seg.is_empty()))
        .map(|seg| {
            percent_encoding::percent_decode_str(seg)
                .decode_utf8_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "download".to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn base64_is_checked_before_and_after_decoding() {
        let ok = STANDARD.encode(b"hello");
        assert_eq!(&decode_base64(&ok, 10).unwrap()[..], b"hello");
        // Over the cap once decoded.
        assert!(decode_base64(&ok, 4).is_err());
        // Over the cap by its length alone: not decoded (the text is not even valid).
        let long = "!".repeat(8_388_609);
        let err = decode_base64(&long, MAX_UPLOAD_BYTES).unwrap_err();
        assert_eq!(err.document().status, 413);
        // Not base64 within the cap.
        let err = decode_base64("not base64!", 100).unwrap_err();
        assert_eq!(err.document().status, 422);
    }

    #[test]
    fn names_come_from_the_last_path_segment() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert_eq!(
            name_from_url(&u("https://a.test/x/team%20photo.jpg?v=2")),
            "team photo.jpg"
        );
        assert_eq!(name_from_url(&u("https://a.test/x/")), "x");
        assert_eq!(name_from_url(&u("https://a.test/")), "download");
    }
}
