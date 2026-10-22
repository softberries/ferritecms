# 10 — Testing Strategy

## 1. Layers

| Layer | Where | What | Runs |
|---|---|---|---|
| Unit | next to code (`#[cfg(test)]`) | pure logic: hierarchy resolution, permalink matching, manifest validation, block rendering, WXR fragments | every `cargo test` |
| Store contract | `crates/ferrite-store-tests` | every repository method on **SQLite and Postgres** | every CI run (PG via service container) |
| Migration | `ferrite-store-tests` | up/down/up roundtrip per migration with seeded data; schema snapshot equality; same version sets on both backends; expand-lint | every CI run |
| Plugin host | `crates/ferrite-plugin-host/tests` | capability linking, limits, hostile plugins ([05 §12](05-plugin-host.md#12-hostile-test-plugins-required-by-claudemd)) | every CI run (test plugins built first) |
| Cluster | `tests/cluster_*.rs` | 2–3 in-process nodes against one Postgres: invalidation propagation, job exclusivity, lease expiry after a "crashed" node, periodic job single-run, rolling version coexistence | every CI run |
| HTTP integration | `tests/http_*.rs` | spawn `serve` on a random port with temp data; public pages, API CRUD, auth/CSRF, cache invalidation | every CI run, both backends |
| Media | `ferrite-media` tests | local folder + in-process S3 server (`s3s-fs`), no Docker | every `cargo test` |
| Import | `ferrite-import-wp` tests | fixture WXRs → golden JSON of resulting content | CI |
| Admin | `admin/` | Vitest unit/component tests; Playwright E2E against a real binary | CI |
| Benchmarks | `benches/` (criterion) | filter call overhead, cached/uncached render, cache invalidation fan-out | manual + nightly, results tracked |
| Backward compat | CI job | previous release's integration tests against current migrations | from first tag |

## 2. Test infrastructure

- `FERRITE_TEST_PG_URL` env var. Missing locally → those
  tests are skipped with a printed notice; in CI (`CI=true`) missing → fail.
- Locally, point `FERRITE_TEST_PG_URL` at any Postgres >= 15. CI uses a `postgres:18` service container (`.github/workflows/ci.yml`).
- **S3 tests need no MinIO or Docker.** `ferrite-media` tests start an `s3s-fs` S3 server
  in-process on `127.0.0.1:0` (bucket = a temp directory) and point the real `AmazonS3`
  client at it (`crates/ferrite-media/tests/common/mod.rs`). The `backend_tests!` macro
  generates, for each test body, a `local` and an `s3` test (`mod <name> { local(); s3(); }`),
  so every storage behaviour is asserted on both backends. Caveat: s3s-fs may not implement
  every S3 feature (for example listing orphan multipart parts), so the orphan-multipart
  case is not covered by tests. The S3 lifecycle rule that backstops it is a planned M4 recommendation.
- Postgres isolation: schema-per-test; SQLite: temp dir per test.
- Test plugins are built by the tests themselves: `crates/ferrite-plugin-host/src/test_support.rs`
  (included by path from `tests/common/mod.rs`) runs
  `cargo build --release --target wasm32-wasip2 --manifest-path plugins/Cargo.toml` once per
  test binary (`OnceLock`) before reading the `.wasm`, and panics with cargo's stderr if it fails.
  This needs the `wasm32-wasip2` target installed. CI also builds the plugins in an explicit step.

## 3. CI (GitHub Actions)

Exists today (`.github/workflows/ci.yml`):

1. `cargo fmt --check` for the workspace and for `plugins/`
2. `cargo clippy --all-targets -- -D warnings` (workspace) and clippy for `plugins/` on `wasm32-wasip2`
3. build test plugins (`wasm32-wasip2`)
4. `cargo test --workspace` with a Postgres 18 service

Planned:

5. admin: `pnpm install --frozen-lockfile`, `pnpm lint`, `pnpm test`, `pnpm build`
6. E2E (Playwright) against release build
7. OpenAPI drift check (generated client == committed)
8. migration set parity + expand-lint

## 4. Definition of done (per checklist item)

- Tests for the behaviour, on both backends where storage is touched.
- fmt + clippy clean.
- Docs updated if behaviour or config changed.
- One reviewable commit (or a small series) per checklist item.

**Known harness quirk.** A Postgres-backed test that ends right after dropping an uncommitted `Tx` stalls about 5 s at schema teardown (the dropped transaction's connection is released late). Commit or roll back explicitly to avoid it.
