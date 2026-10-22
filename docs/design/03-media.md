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
path, and GCS/Azure nearly free later. Its conditional put (`ETagMatch`, works on R2 and
MinIO per its docs) is not required by our design but is useful for idempotent writes.

Rejected: `aws-sdk-s3` (heavy, AWS-specific), `rust-s3` (smaller community, S3 only).

## 3. `MediaStore` abstraction

```rust
// ferrite-media (shape)
pub struct MediaStore {
    inner: Arc<dyn ObjectStore>,
    public: PublicUrlMode,          // ServeViaApp | Direct { base_url }
    prefix: Path,                   // optional key prefix, e.g. "sites/<site_id>/"
}

impl MediaStore {
    pub async fn put_artifact(&self, bytes: impl Stream<Item=Bytes>, kind: ArtifactKind)
        -> Result<ArtifactRef, MediaError>;            // hashes while streaming, sha256
    pub async fn get(&self, hash: &Hash) -> Result<GetResult, MediaError>;   // streaming, range support
    pub async fn head(&self, hash: &Hash) -> Result<Option<ObjectMeta>, MediaError>;
    pub async fn delete(&self, hash: &Hash) -> Result<(), MediaError>;
    pub fn public_url(&self, hash: &Hash, filename: &str) -> Url;
    pub async fn check(&self) -> Result<(), MediaError>;  // startup: write+read+delete a probe object
}
```

### Key layout (content-addressed)

```
<prefix>artifacts/ab/abcdef…(sha256 hex)        # originals, variants, plugins, themes
<prefix>tmp/<uuid>                              # multipart upload staging (lifecycle rule: 1 day)
```

- Content addressing makes writes idempotent, gives free dedup (WP sites upload the same
  image many times), and makes cache headers trivial (`immutable`).
- Human-friendly URLs are a routing concern, not a storage concern: public URL is
  `/_media/<hash>/<original-filename>` (app-served) or
  `<public_base_url>/artifacts/ab/<hash>` with `Content-Disposition` filename set at
  upload time (direct mode).
- Upload flow: stream to `tmp/<uuid>` while hashing → if `artifacts/<hash>` exists,
  delete tmp; else copy/rename to final key → insert `artifacts` row → insert `media`
  row. On S3, "rename" is copy+delete; for small files (< 8 MB) we buffer and put
  directly to the final key after hashing.

### Local backend

- Root: `data/media/` (configurable). `LocalFileSystem` with the same key layout.
- Files written via temp file + atomic rename (object_store's local impl does this; to
  verify in M1).

### S3 backend

Config (see [01](01-architecture.md#4-configuration)): `bucket`, `region`, `endpoint`,
credentials (static, `*_file`, or env/instance-role via `AmazonS3Builder::from_env`),
`allow_http` (MinIO dev), `virtual_hosted_style` (provider-dependent), `prefix`,
`public_base_url`.

Provider notes to document (each verified when writing the ops docs, not assumed):
R2 path-style URLs, B2 region naming, Hetzner/Scaleway endpoints.

## 4. Images and variants

- On upload of an image (jpeg/png/webp/gif/avif-if-supported): a **job** `media.variants`
  is enqueued ([04](04-cluster.md#job-queue)). Uploads return immediately.
- Variant set defined by theme (`theme.toml` `[images]`) + core defaults
  (`thumbnail 150x150 crop`, `medium 768w`, `large 1600w`), output WebP + original
  format.
- Variants are artifacts too; `media.variants` json maps name → `{hash, w, h, mime}`.
- Until variants exist, templates fall back to the original.
- `image` crate for decode/resize; EXIF orientation applied; EXIF/GPS **stripped** by
  default (privacy) — option to keep.
- Max decoded pixel count guard (default 100 MP) to prevent decompression bombs.

## 5. Serving

- **App-served** (`/_media/<hash>/<name>`): streams from MediaStore with
  `Cache-Control: public, max-age=31536000, immutable`, `ETag: "<hash>"`, range
  requests, `X-Content-Type-Options: nosniff`. Small in-memory LRU for hot small files
  (configurable, default 64 MB).
- **Direct**: templates and API return `public_base_url` URLs; the app never proxies.
  The operator configures the bucket/CDN for public read of `artifacts/` only.
- SVG: served with `Content-Security-Policy: sandbox` and
  `Content-Disposition: inline` only if it passed sanitisation; otherwise `attachment`.

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
