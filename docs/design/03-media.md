# 03 — Media Storage

## 1. Requirements

| # | Requirement | Source |
|---|---|---|
| M1 | Local folder backend (default) — part of the single data folder | Principle 1 |
| M2 | S3-compatible backend from the first release (AWS S3, Cloudflare R2, MinIO, Hetzner, Scaleway, Backblaze B2 S3 API…) | User (D4) |
| M3 | All nodes in a cluster see the same media | D5 |
| M4 | Media can be served by the binary or directly from a CDN/bucket URL | Design |
| M5 | Plugins/themes are distributed through the same store (artifacts) | D11 |

## 2. Library: `object_store` (D10)

The Apache Arrow `object_store` crate (0.14.x) provides one async `ObjectStore` trait
with `LocalFileSystem`, `AmazonS3` (custom endpoint via `with_endpoint`, verified), GCS,
Azure and in-memory implementations. That gives us local + S3-compatible with one code
path, and GCS/Azure nearly free later. Its conditional create (`PutMode::Create`) and `copy_if_not_exists`
are used for idempotent writes (see §3).

Rejected: `aws-sdk-s3` (heavy, AWS-specific), `rust-s3` (smaller community, S3 only).

## 3. `MediaStore` abstraction (implemented in M1b, `crates/ferrite-media`)

```rust
pub struct MediaStore { /* Arc<dyn ObjectStore>, prefix, PublicUrlMode */ }

impl MediaStore {
    pub fn open(cfg: MediaConfig) -> Result<Self, MediaError>;
    pub async fn check(&self) -> Result<(), MediaError>;           // write + read back + delete a probe
    pub async fn put_artifact(&self, data: Bytes) -> Result<ArtifactRef, MediaError>;
    pub async fn put_stream<S>(&self, s: S, max_bytes: u64) -> Result<ArtifactRef, MediaError>
        where S: Stream<Item = Result<Bytes, io::Error>> + Send + Unpin;
    pub async fn get(&self, hash: &ArtifactHash) -> Result<Bytes, MediaError>;
    pub async fn get_range(&self, hash: &ArtifactHash, range: Range<u64>) -> Result<Bytes, MediaError>;
    pub async fn open_read(&self, hash: &ArtifactHash, range: Option<Range<u64>>) -> Result<ReadStream, MediaError>;
    pub async fn head(&self, hash: &ArtifactHash) -> Result<Option<u64>, MediaError>;   // size
    pub async fn delete(&self, hash: &ArtifactHash) -> Result<(), MediaError>;           // idempotent
    pub fn public_url(&self, hash: &ArtifactHash, filename: &str) -> String;
}

pub struct ArtifactRef { pub hash: ArtifactHash, pub bytes: u64, pub mime: String, pub created: bool }
pub struct ReadStream { pub size: u64 /* total object size */, pub range: Range<u64> /* served */,
                        pub stream: BoxStream<'static, Result<Bytes, MediaError>> }
```

- **Reads.** `get` and `get_range` return buffered `Bytes` and are for small reads only
  (metadata, manifests, variants being processed). **Serving uses `open_read`**, which streams the
  body without buffering it (see Range semantics below). The `/_media` handler (M1c-2, §5) serves
  through `open_read`.
- `head` returns the object size (`Option<u64>`), `None` when missing.
- `delete` succeeds when the artifact is already missing.
- `ArtifactRef.mime` is sniffed from the first 8 KiB of content (see Sniffing below), never
  from a file name.
- `ArtifactRef.created` is **advisory under concurrent writers**: it is `false` if the artifact
  already existed when this call looked. With local storage and S3 conditional put it is exact,
  but the S3 streaming path (head + copy fallback, below) can report `true` to two racing writers
  of identical bytes. The `artifacts` DB row (`insert_if_absent`) is the source of truth for
  "first writer", not `created` (decision recorded in M1b).
- `open` validates the prefix: empty, or `/`-separated segments each matching `[A-Za-z0-9._-]+`
  (not `.` or `..`, no empty segment such as `//`), ending with `/` and not starting with `/`.
  With `ServeViaApp`, `path_prefix` must start with `/` and not end with `/`. Violations, and
  errors from `LocalFileSystem::new_with_prefix` / `AmazonS3Builder::build`, are
  `MediaError::Config`. A local root directory is created if missing (failure there is `Io`). S3 requires `access_key_id` and `secret_access_key` together;
  with neither set, credentials come from `AmazonS3Builder::from_env()`. S3 requests retry up to
  2 times with a 20 s retry timeout (`store.rs::build_s3`).
- `check()` writes a probe to `<prefix>tmp/probe-<uuid>`, reads it back, and attempts to delete
  it after a successful put, even when the read-back fails (a failed delete only logs a warning).

### Key layout (content-addressed)

```
<prefix>artifacts/ab/abcdef…(sha256 hex)        # originals, variants, plugins, themes
<prefix>tmp/<uuid v7>                           # multipart staging (put_stream only)
<prefix>tmp/probe-<uuid v7>                     # check() probe
```

(`crates/ferrite-media/src/keys.rs`.) The `ab/` directory is the first two hex characters of the hash.

- Content addressing makes writes idempotent, gives free dedup (WP sites upload the same
  image many times), and makes cache headers trivial (`immutable`).
- Human-friendly URLs are a routing concern, not a storage concern (see public URLs below).

### Write paths and the 8 MiB threshold

- **Small path** (< 8 MiB, `SMALL_LIMIT`): hash in memory, `head` the final key; if absent,
  `put_opts(PutMode::Create)` straight to `artifacts/ab/<hash>`. `AlreadyExists` maps to
  `created = false`. Only if the backend answers `NotImplemented` does it fall back to a plain `put`.
- **`put_artifact`** with a payload >= 8 MiB delegates to `put_stream`.
- **`put_stream`** buffers up to the first 8 MiB. A stream that ends before that takes the small
  path. Otherwise it starts a multipart upload to `tmp/<uuid>` (8 MiB parts, at most 2 parts in
  flight, which bounds memory to roughly that many parts; the initial 8 MiB buffer is released
  once handed to the writer), hashing while streaming, then places the object at its final key and deletes the
  staging object.
- **Placement** uses `copy_if_not_exists` (`AlreadyExists` -> `created = false`). Backends that
  return `NotImplemented`/`NotSupported` get the fallback: `head` the final key (exists ->
  `created = false`), else `copy`. This head-then-copy window is the race behind the advisory
  `created`. The final bytes are identical either way because the key is the content hash.
  **Production S3 currently always uses the fallback**: no `copy_if_not_exists` mode is
  configured on the S3 builder (M4 follow-up). The s3s-fs test server also takes the
  fallback, because the test `S3Config` sets no `copy_if_not_exists`; only the local backend
  exercises the conditional (hard-link) paths in tests.

### Upload size cap and the 5 GB copy ceiling

`put_stream(stream, max_bytes)` fails with `MediaError::TooLarge { limit }` as soon as the running
total exceeds `max_bytes` (exactly `max_bytes` is accepted); the staging upload is aborted like on
any other error. `put_artifact` has no cap parameter (the payload is already in memory; it
delegates with `u64::MAX`). The HTTP layer must always pass a limit.

Placement of a streamed upload uses a single server-side `CopyObject`. AWS S3 caps one
`CopyObject` at 5 GB; object_store's `copy` does not switch to multipart copy on its own. Upload
limits must therefore stay **below 5 GB** (recommended: far below, for example 1 GB or less)
until a multipart-copy path exists. Other S3-compatible providers have their own limits.

### Staging cleanup

- On any error in the streaming path (source stream error, backend error, `finish()` failure,
  placement failure) the multipart upload is aborted and `tmp/<uuid>` is deleted, best effort
  (failures are logged with `tracing::warn!`).
- On **cancellation** (the `put_stream` future is dropped) a drop guard (`StagingGuard`) spawns
  the same abort + delete on the current tokio runtime.
- Known limits: if no tokio runtime exists at drop time, or the abort/delete request itself
  fails, staging objects or orphan S3 parts remain. Cancellation during the final
  `writer.finish()` call can also orphan S3 parts, since the writer has already been taken from
  the guard. The backstop is an S3 lifecycle rule (abort incomplete multipart uploads, expire
  `tmp/`), which is an M4 ops recommendation.

### Range semantics

`get_range(hash, start..end)`, end exclusive. A missing artifact is `NotFound`. An inverted range
(`start > end`) or `end` past the object size is `MediaError::Range`; an empty range
(`start == end <= size`) returns empty `Bytes` without a backend read. Mapping to HTTP 416 or
suffix/open-ended ranges is the caller's job.

`open_read(hash, range)` returns a `ReadStream { size, range, stream }`: `size` is the total object
size (from the response metadata of one `get_opts` request, not a separate `head`), `range` is the
byte range actually served (`0..size` when no range was requested). Validation is identical to
`get_range` (same `NotFound` / `Range` errors and messages). Inverted and empty ranges are answered
with a metadata-only request. Dropping the stream aborts the transfer.

S3 client options (`build_s3`, object_store 0.14.2 `ClientOptions`; set before `with_allow_http`, which would otherwise be overwritten, and they replace any client options read by `from_env`): `with_timeout_disabled()`
(no total-request timeout; the 30 s default would cut off large streams), `with_connect_timeout(5 s)`
(same as the default, set explicitly), `with_read_timeout(30 s)` (per-read idle timeout; the
default is none). Retries are unchanged (2 retries, 20 s retry timeout). These options were checked
against `src/client/mod.rs`; they are not exercised by a test (the in-process test server cannot
stall).

### Public URLs

`public_url(hash, filename)`:

- `ServeViaApp { path_prefix }` -> `<path_prefix without trailing />/<hash>/<filename>`. The
  filename first goes through `sanitize_filename` (M1c-2, shared with the `/_media` handler):
  `/`, `\` and control characters become `_`, and the empty string, `.` and `..` become
  `file`, because clients normalise those segments away and would drop the hash. It is then
  percent-encoded as one path segment (everything but RFC 3986 unreserved characters is
  escaped).
- `Direct { base_url }` -> `<base_url without trailing />/<prefix>artifacts/ab/<hash>`. The
  filename is not part of the URL. The `Content-Disposition` filename idea from the first draft
  is **not implemented**.

### Sniffing

`sniff_mime(head)` uses the `infer` crate on content only and falls back to
`application/octet-stream`. `is_raster_image` accepts exactly `image/jpeg`, `image/png`,
`image/gif`, `image/webp`. SVG is not a raster type and is not processed.

**Serving safety.** `sniff_mime` can return `text/html` and `text/xml` (the bytes of an uploaded
HTML or XML file), and SVG is not detected at all: it comes back as `text/xml` or
`application/octet-stream`. The sniffed type must therefore never be echoed as `Content-Type`
blindly. `inline_safe(mime)` is an exact-match allowlist: `image/jpeg`, `image/png`, `image/gif`,
`image/webp`, `audio/mpeg`, `audio/ogg`, `audio/wav` (and `audio/x-wav`, which is what `infer`
reports for WAV), `video/mp4`, `video/webm`, `application/pdf`. The serving layer (M1c-2, §5) sends
those inline with their type, and everything else as `application/octet-stream` with
`Content-Disposition: attachment` and `X-Content-Type-Options: nosniff`.

**SVG detection** (`crates/ferrite-media/src/svg.rs`, M2a-2b P4/P7). `sniff_mime` never reports
SVG, so uploads check `looks_like_svg(head)` first: after an optional UTF-8 BOM and XML
whitespace the head starts with `<?xml`, `<!--` or `<svg`, and `<svg` occurs within the first
8 KiB. Such an upload is never stored as sent: it goes through `sanitize_svg` (§5 "SVG") and the
sanitised bytes are stored as `image/svg+xml` (`SVG_MIME`) with `media.sanitized = true`; a
refused SVG is not stored. Bytes that are not detected as SVG follow `sniff_mime`, so a PNG
named `.svg` is a PNG.

### Local backend

- Root: `data/media/` (configurable) via `LocalFileSystem::new_with_prefix`, same key layout.
- object_store's local `put` writes a staged temp file and publishes it with an atomic rename;
  `PutMode::Create` publishes with a hard link (fails if the target exists), and
  `copy_if_not_exists` is hard-link based (object_store 0.14.2 `local.rs`). Settled; no longer
  to be verified.

### S3 backend

Config (`S3Config`): `bucket`, `region`, `endpoint`, `access_key_id`/`secret_access_key`
(or env credentials), `allow_http` (MinIO dev), `virtual_hosted_style`; `prefix` and the
public URL mode are on `MediaConfig`. Config-file wiring (including `*_file` secrets) belongs
to the core config layer ([01](01-architecture.md#4-configuration)).

Provider notes to document (each verified when writing the ops docs, not assumed):
R2 path-style URLs, B2 region naming, Hetzner/Scaleway endpoints.

Recommended bucket lifecycle rules (to be written up in M4 ops docs): abort incomplete multipart
uploads after 1 day, expire `tmp/` after 1 day.

## 4. Images and variants

### What is implemented (`process_image`, `crates/ferrite-media/src/image.rs`)

`process_image(data: &[u8], specs: &[VariantSpec]) -> Result<Processed, MediaError>` is a pure,
CPU-bound function (callers use `spawn_blocking`). It does not touch storage.

- **Input**: sniffed JPEG, PNG, GIF or WebP. Anything else is `UnsupportedImage`.
- **Limits**: decoded pixels <= 100 MP (`MAX_PIXELS`), width and height each <= 50 000,
  decoded buffer (w × h × bytes per pixel of the decoder's colour type) <= 512 MiB, checked from
  the header before anything is allocated (M2a-2b R12: image 0.25 allocates that buffer outside
  `Limits::max_alloc`, so 100 MP of 16-bit RGBA would otherwise take 800 MB), and
  decoder-internal allocations <= 512 MiB (`max_alloc`). `ImageTooLarge { pixels }` is returned when the header
  reports more than 100 MP, and when the decoder rejects a dimension/allocation limit
  (in that case `pixels` is `w*h` if the header was read, else `0`). Other decode failures are
  `UnsupportedImage`. Invalid specs are `Config` errors: a zero width/height, an empty name, a
  duplicate name, or a `height` without `crop` (a bounding box is not implemented).
- **Orientation**: EXIF orientation is applied; `Processed.width/height` are post-orientation.
- **Metadata**: output is re-encoded, so variants carry no EXIF, XMP or GPS (no keep option
  yet). The **stored original is kept byte for byte, EXIF (GPS included) and all**, as WordPress
  does (M2a-2b R11): a photo's location stays readable from its original URL. Optional
  location-metadata stripping on upload is an M4 item (ROADMAP).
  The **ICC profile** is kept only if its colour-space signature (header bytes 16..20) matches
  the output pixels: `RGB ` for JPEG and colour PNG, `GRAY` for grayscale PNG. A mismatching
  profile (for example CMYK) is dropped, as is one the encoder rejects or one larger than 1 MiB
  (M1b decision: keep colour fidelity).
- **Output formats (decision: no WebP output in v1)**: JPEG sources -> JPEG quality 82;
  PNG, GIF and WebP sources -> PNG. GIF input yields its first frame as PNG. WebP is decoded
  but never written. Animated-WebP behaviour is not verified. WebP/AVIF output is deferred. Reason given for the decision: the `image`
  crate's WebP encoder is lossless-only, and lossy WebP would need a C dependency.
- **Variant rules**: no upscaling.
  - Width-bounded spec (`height: None`, `crop: false`): keeps the aspect ratio; skipped
    entirely (no variant emitted) when the source is not wider than the target.
  - `crop` spec: cover-resize and center-crop to `width x height` (height defaults to width).
    If the source is smaller than the target in either dimension, nothing is upscaled and the
    result is the center-cropped square of `min(w, h)` capped at the target size. A non-square
    target therefore degrades to a square in that case.
- **Defaults** (`default_variants()`): `thumbnail` 150x150 crop, `medium` 768 wide, `large` 1600 wide.
- **Memory** (upper bounds per call): the decoded buffer (<= 512 MiB); an EXIF rotation copies it
  (<= 512 MiB more), and the JPEG RGB conversion copies it as RGB8 (<= 300 MB at 100 MP). Each
  size then holds the resize's `f32` RGBA intermediate (source width × target height × 16 bytes,
  never more than 16 bytes per source pixel) and its output (never larger than the decoded
  buffer: nothing is upscaled). With the core sizes the intermediate is at most
  50 000 × 1 600 × 16 B ≈ 1.3 GB (a tall image and `large`), so one call stays under about
  0.5 + 1.3 + 0.5 ≈ **2.4 GB**. A theme size wider than 1 600 can raise the intermediate to
  16 B × 100 MP = 1.6 GB (≈ 2.7 GB). Typical photos need far less (a 24 MP JPEG: about 72 MB
  decoded, ≈ 0.2 GB peak). `process_image` does not limit concurrency; the `media.variants`
  handler does, with a per-node semaphore of `media.image_workers` slots (M1b R4, see below),
  so a node needs about 2.4 GB × `image_workers` of headroom for worst-case uploads. The
  100 MP cap is kept as designed.
- **Decode time** is not bounded: the `image` crate has no time limit. The dimension, pixel and
  allocation limits bound the work per image, and the handler's slots bound how many run at
  once.

### Variants as built

- `media.variants` maps name -> `{hash, w, h, mime}`; each variant is an artifact (kind
  `variant`) stored with `put_artifact`. Since migration `0003_media_variants` the store also
  keeps a derived `media_variants(hash, item_id, name)` table, indexed by hash and kept in sync
  by the media repository in the same transaction (ruling R29, [02 §4](02-storage.md#4-logical-schema)).
- Templates get variants through `image(media, size)` and `post.featured.variants`
  ([06 §4](06-themes-rendering.md#4-minijinja-environment)); an unknown size falls back to the
  original, and `srcset` lists one candidate per width.
- **Recording uploads (M2a-2b, `ferrite_core::MediaService`).** The bytes are staged
  outside any transaction by `ferrite-http`'s uploader, which also decides the type from the
  content (P3/P4). `record_upload` then writes, in one transaction: the `artifacts` row (unless
  the hash exists), the media item (kind `media`, title = the given title or the file name
  without its extension, empty body), its publish (uploads are published at once, P5), the
  `media` row (no dimensions, `variants` `{}`, `sanitized` only for sanitiser output) and, for
  a raster original, one `media.variants` job (payload `{"v":1,"item":"<id>","artifact":"<hash>"}`,
  unique key `media.variants:<item>:<artifact>`, P8). Every one of these writes emits
  `post:<media id>` in that transaction (M1c-2 R11). Documents (PDF and the R1 classes) get no
  job. The client's file name keeps only its last `/` or `\` segment (1–255 bytes, no control
  characters) and is stored as text, never used as a path; a failed or refused upload leaves
  no row, event or job, and its blob for M4's GC (M1b R2).
- **The `media.variants` handler** (`crates/ferrite/src/jobs/media.rs`, M2a-2b P8, F17), on
  every node's job runner:
  1. A payload that does not parse or names an invalid hash is `Permanent` (dead at once).
  2. One of the node's `media.image_workers` slots (default 1, 1–16) is taken **without
     waiting**. With none free the job is `Busy`: put back with the attempt refunded and due
     again after 1–5 s, so image jobs never hold every runner worker. It is taken first,
     so a busy re-claim costs no store read and no (S3) `HEAD`.
  3. The `media` row is read; when it is gone or holds another original than the job's, the job
     is done without decoding.
  4. An original over 64 MiB (`MAX_PROCESS_BYTES`, by `head`) gets no variants: done, logged.
     Templates fall back to the original.
  5. The original is read (`get`, at most 64 MiB) and decoded with `process_image` on the
     blocking pool. The slot's permit moves into the blocking closure: the runner may drop the
     handler at any await (lease loss, timeout, shutdown), but a blocking task cannot be
     stopped. The slot therefore stays taken until the decode ends, and never more than
     `image_workers` images decode at once on a node.
  6. The sizes are `ferrite_http::media::variant_specs(theme)`: the core sizes plus the
     `[images]` of the theme active on that node when the job runs. A theme size whose name is
     taken, that has a height without crop, or that has a zero dimension is skipped. Each variant
     is stored with `put_artifact` (kind `variant`), then `record_variants(item, artifact, width,
     height, variants, artifacts)` runs. In one transaction, and only while the `media` row still
     holds that original (else `false`, nothing written), it writes the variant `artifacts` rows,
     the row's dimensions and `variants`, and `post:<item>`. The row is read with
     `MediaRepo::get_for_update` (Postgres `FOR UPDATE`), as the alt-text edit reads it, so the
     two read-modify-writes of the unversioned row cannot lose each other's change.

  An image that is over the limits, does not decode or makes the decoder panic (the blocking
  task's panic is caught as a `JoinError`) is `Permanent`. Its job goes `dead` at the first
  attempt with the reason in `last_error`, the worker carries on, and the original stays
  served. Store and media-backend errors are `Retry`. Variants are re-encoded, so EXIF (GPS
  included) and XMP never reach them; the stored original keeps its EXIF, GPS included (R11). Regenerating variants after a theme change is M4 (P8).
- `ferrite seed demo` still runs `process_image` in-process, with the same
  `variant_specs(theme)`. It writes the `media` and `artifacts` rows in one transaction that
  also emits `post:<media id>` (`crates/ferrite/src/seed.rs`).

## 5. Serving

**App-served** (`/_media/<hash>/<name>`), as built in `crates/ferrite-http/src/app/media.rs`:

- **Published only.** A file is served only when `hash` is the original or a variant of a
  public media item (published, `published_at <= now`): `SiteReader::media_file` calls
  `MediaRepo::find_by_artifact`, which returns nothing without reading `media` unless the
  `artifacts` row of the hash exists with kind `media` or `variant`, then matches the original
  by `media.artifact_hash` and variants through `media_variants` (both indexed; R29), at most
  100 rows, whose status is checked with one `get_many`. Anything else is a plain-text `404`
  (no theme render, `Cache-Control: no-store`).
- **Name.** The last segment must be the item's `original_name` after `sanitize_filename`
  (`crates/ferrite-media/src/name.rs`: `/`, `\` and control characters become `_`; an empty,
  `.` or `..` name becomes `file`), the same function `MediaStore::public_url` uses. Another
  name is a `301` to the canonical one.
- **Streaming** through `MediaStore::open_read` (never the buffered `get`); never in the page
  cache and never compressed.
- **Headers:** `ETag: "<hash>"` (strong: the bytes never change), `If-None-Match` → `304`,
  `Cache-Control: public, max-age=31536000, immutable`, `Accept-Ranges: bytes`,
  `X-Content-Type-Options: nosniff`, `Content-Length`.
- **Types:** `inline_safe(mime)` types (JPEG, PNG, GIF, WebP, MP3/Ogg/WAV, MP4/WebM, PDF) are
  served inline with their type (`Content-Disposition: inline`). `application/pdf` also gets
  `Content-Security-Policy: sandbox` (F14). An SVG whose row is marked `media.sanitized` is
  served inline as `image/svg+xml` with `Content-Security-Policy: sandbox; default-src 'none';
  img-src data:; style-src 'unsafe-inline'` (P7, below). Everything else is
  `application/octet-stream` with `Content-Disposition: attachment; filename="<ASCII
  fallback>"; filename*=UTF-8''<encoded>`. That covers an unmarked SVG, the R1 documents
  (docx, xlsx, pptx, odt, ods, odp, zip, txt, csv) and any other type. Every response carries
  `nosniff`.
- **Ranges:** one range only. `bytes=a-b`, `bytes=a-` and the suffix `bytes=-n` give `206`
  with `Content-Range`; a start at or past the end, or `bytes=-0`, is `416` with
  `Content-Range: bytes */<size>`. Several ranges, a malformed or non-`bytes` range, an
  inverted range, and a range whose `If-Range` does not equal the ETag are answered with
  `200` and the full body (no multipart responses).
- **HEAD** ignores `Range` and answers with the full-size headers from one metadata request,
  without opening a stream.
- **Deferred:** the small in-memory LRU for hot small files (default 64 MB in the first
  draft) is not built (ROADMAP M4).

**Direct** (`[media.s3] public_base_url` set): `public_url` returns `<base>/<prefix>artifacts/…`
URLs and templates link there; the app does not proxy. The operator configures the
bucket/CDN for public read of `artifacts/` only. Without `public_base_url`, S3 media is
app-served like local media (`crates/ferrite/src/site.rs`, `media_config`).

None of the app-served rules above apply in Direct mode:

- **No published-only check.** Every object under `artifacts/` is readable by anyone who
  knows its hash: unpublished or trashed media, variants of unpublished media, and plugin
  `.wasm` artifacts as well. Unpublishing or trashing an item does not revoke access to a
  URL that was already shared.
- **No attachment, `nosniff` or PDF CSP headers from the app.** Whatever the bucket or CDN
  sends is what the browser gets: the object's stored `Content-Type` and the CDN's header
  rules. An SVG or HTML upload served inline from the media origin is the operator's risk.
  In particular the download-only rule for documents (M2a-2b R1: `attachment` + `nosniff`)
  and the sandbox CSP of sanitised SVG (P7) hold for app-served media only (F15).
- **The operator must restrict the bucket**: public read of `artifacts/` only (never
  `tmp/` or the prefix root), the security headers set at the CDN, and ideally a separate
  origin for media so inline content cannot reach the site's cookies.

Use app-served mode when unpublishing must revoke access. Signed or scoped direct media URLs
(short-lived signatures, or published files only) are planned for M4 (ROADMAP).

**SVG.** Uploaded SVG is sanitised by our own allowlist over quick-xml 0.41's namespace-aware
reader (`sanitize_svg`, M2a-2b P7; no maintained sanitiser crate exists). Only rows marked
`media.sanitized` with `mime = image/svg+xml` may be served inline, and then with
`Content-Security-Policy: sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'`
and `nosniff`. Every other SVG stays an attachment: an unmarked row (an older binary, a hand
edit) is never trusted. The sanitiser:

- **Refuses** (`SvgError`): input over 2 MiB (`TooLarge`); anything that is not UTF-8, including
  UTF-16 with or without a BOM and an XML declaration naming another encoding (`NotUtf8`; a
  UTF-8 BOM is accepted); malformed XML, characters XML 1.0 forbids (also as `&#…;`), text or
  elements outside the single root, or an XML version other than 1.0 (`NotXml`, whose message
  is a fixed description plus a byte offset and never echoes input); a root other than `svg` in
  `http://www.w3.org/2000/svg` (`NotSvg`); every DOCTYPE except one standard SVG DOCTYPE before
  the root (`svg PUBLIC` with one of the SVG 1.0/1.1 public ids, as Illustrator writes, and no
  internal subset), which is accepted and not written out, so entity declarations, billion
  laughs, external (XXE) and parameter entities never reach a parser and no DTD is ever fetched
  (`Doctype`, R6); a reference other than `lt gt amp apos quot` or a numeric one, in text or
  attribute values, kept or dropped (`UnknownEntity`, the name capped at 64 characters); depth
  over 64 (`TooDeep`), more than 50 000 elements (`TooManyElements`), more than 64 attributes on
  an element (`TooManyAttributes`), a value over 256 KiB (`ValueTooLong`), more than 128 kept
  `<use>` elements, or a `<use>` whose `#id` names a `<use>` or an element containing one,
  including its own ancestor (`TooManyUses`, R6). Limits count dropped elements too. Without
  use chains every `<use>` instantiates a use-free subtree, so instancing is bounded by
  128 × the document; markers, patterns and filters can still be costly to render (residual,
  client-side only).
- **Keeps** elements in the SVG namespace by exact local name: `svg g defs symbol use title desc
  path rect circle ellipse line polyline polygon text tspan textPath linearGradient
  radialGradient stop pattern clipPath mask marker filter feBlend feColorMatrix
  feComponentTransfer feComposite feFlood feGaussianBlur feMerge feMergeNode feOffset feFuncR
  feFuncG feFuncB feFuncA image`. Every other element (`script`, `foreignObject`, `style`, `a`,
  `animate*`, `set`, `iframe`, `embed`, `object`, `feImage`, unknown, wrong-case or
  wrong-namespace elements, e.g. an XHTML `script`) is dropped with its whole subtree.
- **Keeps** unprefixed geometry and presentation attributes from one allowlist (see
  `ATTRIBUTES` in `svg.rs`), `xml:space` (`default`/`preserve`), and `href` / `xlink:href`
  (XLink resolved by namespace, whatever the prefix) only as a local `#id`, or on `image` as a
  `data:image/(png|jpeg|gif|webp);base64,…` URI. Every `on*` attribute, every other prefixed
  attribute and input namespace declarations are dropped. Values are read through
  `normalized_value`, so entity-encoded `javascript:` is caught; a value is dropped when it
  contains `\`, `javascript:`, `vbscript:`, `expression(`, `@import`, `src(`, or the CSS image
  functions `image-set(`, `image(`, `cross-fade(`, `element(` (ignoring case, whitespace and
  control characters, so `-webkit-` forms match too) or a `url(` that is not `url(#…)`. `mask`,
  `clip-path`, `filter` and `marker-start/mid/end`, as attributes or `style` declarations, keep
  only `none` or exactly one `url(#id)` (R6); `fill` and `stroke` keep colours, and any `url(` in
  them must be local. A `style` value is also
  dropped when it contains `{ } < > /* @`; otherwise only declarations whose property is a
  presentation property (fill, stroke…, font-*, opacity, display, visibility, transform, the
  marker/clip/mask/filter properties) and whose quotes balance are kept.
- **Drops** comments, processing instructions and CDATA; text is kept only inside `title desc
  text tspan textPath`.
- **Writes** a canonical document: `<?xml version="1.0" encoding="UTF-8"?>`, unprefixed element
  names, the root declaring `xmlns="http://www.w3.org/2000/svg"` (plus
  `xmlns:xlink="http://www.w3.org/1999/xlink"` when an `xlink:href` is kept), hrefs written as
  `href` or `xlink:href`, the first of duplicate output names kept, values and text escaped.
  Sanitising the output again yields the same bytes.

Cost: `<style>` sheets, animation, links, external fonts and images are stripped.

## 6. Garbage collection

Artifacts are referenced by `media`, `media.variants`, `plugins`, `themes`, revisions
(featured images in fields). A periodic job `artifacts.gc` (daily, single-runner via
unique job key) deletes artifacts unreferenced for > 7 days (grace period protects
in-flight uploads and rollbacks). Dry-run: `ferrite media gc --dry-run`.

## 7. WordPress media import

The importer ([08](08-wp-import.md)) reads files from `--uploads` and goes through the
same `put_artifact` path, so importing into an S3-backed site works without a local
copy.

## 8. Migration between backends

`ferrite media sync --from local:./site/media --to s3://bucket/prefix` copies all
artifacts (skips existing keys). Because keys are content hashes, the DB needs no
rewrite when switching backends; only `[media]` config changes.
