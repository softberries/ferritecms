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
  decoder allocation <= 512 MiB. `ImageTooLarge { pixels }` is returned when the header
  reports more than 100 MP, and when the decoder rejects a dimension/allocation limit
  (in that case `pixels` is `w*h` if the header was read, else `0`). Other decode failures are
  `UnsupportedImage`. Invalid specs are `Config` errors: a zero width/height, an empty name, a
  duplicate name, or a `height` without `crop` (a bounding box is not implemented).
- **Orientation**: EXIF orientation is applied; `Processed.width/height` are post-orientation.
- **Metadata**: output is re-encoded, so EXIF, XMP and GPS are dropped (no keep option yet).
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
- **Memory**: peak is roughly 3 x w x h x 4 bytes (decoded buffer, oriented/converted copy,
  resized output); at the 100 MP cap that is on the order of 1.2 GB for one image. The function
  does not limit concurrency. The caller must, and the `media.variants` job runner will do so with a
  semaphore (M1b decision; not built yet, see below). The 100 MP cap is kept as designed.

### Variants as built, and what is still to build

- `media.variants` maps name -> `{hash, w, h, mime}`; each variant is an artifact (kind
  `variant`) stored with `put_artifact`. Since migration `0003_media_variants` the store also
  keeps a derived `media_variants(hash, item_id, name)` table, indexed by hash and kept in sync
  by the media repository in the same transaction (ruling R29, [02 §4](02-storage.md#4-logical-schema)).
- Templates get variants through `image(media, size)` and `post.featured.variants`
  ([06 §4](06-themes-rendering.md#4-minijinja-environment)); an unknown size falls back to the
  original, and `srcset` lists one candidate per width.
- The only writer of media rows in M1 is `ferrite seed demo`, which runs `process_image`
  in-process with the core sizes plus the active theme's `[images]` and writes the `media`
  and `artifacts` rows in one transaction that also emits `post:<media id>`
  (`crates/ferrite/src/seed.rs`).
- **Still to build (M2, with the media service and uploads; needs the M1.5 job queue):** on
  upload of an image a **job** `media.variants` is enqueued ([04](04-cluster.md#job-queue)) so
  uploads return immediately, with an image-processing concurrency semaphore (M1b ruling R4).

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
- **Types:** `inline_safe(mime)` types are served inline with their type (`Content-Disposition:
  inline`); `application/pdf` additionally gets `Content-Security-Policy: sandbox`. Everything
  else, **SVG included**, is `application/octet-stream` with `Content-Disposition: attachment;
  filename="<ASCII fallback>"; filename*=UTF-8''<encoded>`.
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

**SVG:** served inline (with `Content-Security-Policy: sandbox`) only once sanitisation on
upload exists (ROADMAP M2); M1 stores no sanitised marker, so every SVG is an attachment.

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
