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
| Media | `ferrite-media` tests | local + **MinIO** container (S3 API) | CI |
| Import | `ferrite-import-wp` tests | fixture WXRs → golden JSON of resulting content | CI |
| Admin | `admin/` | Vitest unit/component tests; Playwright E2E against a real binary | CI |
| Benchmarks | `benches/` (criterion) | filter call overhead, cached/uncached render, cache invalidation fan-out | manual + nightly, results tracked |
| Backward compat | CI job | previous release's integration tests against current migrations | from first tag |

## 2. Test infrastructure

- `FERRITE_TEST_PG_URL` and `FERRITE_TEST_S3_*` env vars. Missing locally → those
  tests are skipped with a printed notice; in CI (`CI=true`) missing → fail.
- `docker-compose.test.yml` brings up `postgres:18` and `minio` for local runs
  (`just test-all`).
- Postgres isolation: schema-per-test; SQLite: temp dir per test.
- Test plugins are built by a `build.rs` in a dedicated test-support crate or a `just`
  recipe before tests (decided in M0; must not make plain `cargo test` fail confusingly).

## 3. CI (GitHub Actions, planned)

1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
2. build test plugins (`wasm32-wasip2`)
3. `cargo test --workspace` with Postgres + MinIO services
4. admin: `pnpm install --frozen-lockfile`, `pnpm lint`, `pnpm test`, `pnpm build`
5. E2E (Playwright) against release build
6. OpenAPI drift check (generated client == committed)
7. migration set parity + expand-lint

## 4. Definition of done (per checklist item)

- Tests for the behaviour, on both backends where storage is touched.
- fmt + clippy clean.
- Docs updated if behaviour or config changed.
- One reviewable commit (or a small series) per checklist item.
