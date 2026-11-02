# M6-2 — Plugin trust Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plugins can be signed with a publisher's minisign key; install verifies the signature and the permission report names the verified publisher; a plugin name's data is bound to its publisher so a same-named plugin from someone else cannot read it; a signed static registry index (GitHub Pages) gives `ferrite plugin search|install NAME|update`, update checks on the Plugins screen, and CI publishes the three example plugins there.

**Architecture:** Pure formats in `ferrite-domain` (`plugin_trust`, `plugin_registry`: key parsing, owner ids, index types and validation), crypto in `ferrite-plugin-host::trust` (minisign verify, shared with `self-update`), decisions and writes in `ferrite-core::extensions::trust` (inside the existing locked install transaction), fetching and the install pipeline in `ferrite-http` (`registry_fetch`, `extensions::Installer`), the operator commands in `crates/ferrite`, the screens in `admin/src/plugins/`. Data stays keyed by plugin name; a new `plugin_owners` row binds the name to an identity.

**Tech Stack:** Rust 2024, `minisign-verify =0.3.0` (already a dependency of `crates/ferrite`; moves to the workspace), `minisign =0.10.0` (dev-dependency, throwaway keys in tests), sqlx (SQLite + Postgres), axum 0.8 + utoipa, reqwest 0.13 through `plugins::egress`, React/TanStack admin, Vitest, Playwright, `node:crypto` (Ed25519, BLAKE2b-512) in E2E support, Python 3 standard library and `minisign` CLI in CI.

**Spec:** `docs/design/14-plugin-trust.md` (read it first; it wins over this plan on behaviour). Also 05 §2–§3 (lifecycle, fingerprint, data retention), 07 §1/§3 (plugin endpoints and screens), 09 §7 (release workflow).

## Global Constraints

- Migration `0015_plugin_publishers` on **both** backends, `-- ferrite:phase expand`, with `.down.sql`; contract tests on both (`FERRITE_TEST_PG_URL=postgres://ferrite@localhost:5432/ferrite_test`). Migrations `0001`–`0013` are frozen (released). `0014` (M6-1) and `0016` (M6-6) land in parallel: touch no other migration and keep `crates/ferrite-store-tests/src/migrations.rs` edits to one added test function.
- No WIT change; `wit/` is not touched.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`; `thiserror` in libraries, `anyhow` only in `crates/ferrite`; no `allow(dead_code)`; no secret-key handling in non-test code (the binary only verifies).
- Capabilities are checked in core (`ExtensionAdmin`), never in handlers.
- Exact values: signature part/file ≤ 4 KiB (`MAX_SIGNATURE_BYTES`), publisher key ≤ 1 KiB (`MAX_PUBLISHER_KEY_BYTES`), index ≤ 2 MiB (`MAX_INDEX_BYTES`), ≤ 1 000 plugins × 100 versions, index/signature fetch 30 s, component fetch 60 s, ≤ 3 redirects, refresh ≤ 1 per 60 s per site, doctor stale threshold 3 days, payload header `ferrite-plugin-signature/1`.
- Tests use throwaway keys made in the test (`minisign::KeyPair::generate_unencrypted_keypair()` in Rust, `e2e/support/minisign.ts` in E2E, `minisign -G -W` in CI dry runs). Never commit a secret key.
- Untrusted input (manifest text, minisig text, publisher keys, index JSON, registry URLs) gets hostile tests in the task that parses it.
- API changes: DTOs in `crates/ferrite-http/src/api/dto/extensions.rs` with `utoipa`; regenerate `crates/ferrite-http/openapi.json` with `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`; then `pnpm -C admin api:gen` for `admin/src/api/schema.d.ts`.
- Admin: strings in `admin/src/i18n/en/admin.json` under `plugins.trust.*`, `plugins.registry.*`, `plugins.publishers.*`; design rules of 11 (tokens, 44 px targets, axe clean, untrusted strings as text).
- Files other M6 parts also touch (merge carefully, small hunks): `crates/ferrite-store/src/repos.rs` (one trait + one `Repos` method), both `repos/mod.rs`, `crates/ferrite-store/src/catalogue.rs`, `crates/ferrite-http/openapi.json`, `admin/src/api/schema.d.ts`, `crates/ferrite/src/jobs.rs` (`core_periodics`, `handlers`), `crates/ferrite/src/serve.rs`, `docs/ROADMAP.md`.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

- A signed plugin installed, then an update whose signature was **stripped** → refused (`publisher`), the installed version keeps running, its data untouched.
- A plugin whose data belongs to publisher A, and a bundle of the same name signed by key B (valid signature) → refused, even when B is approved as a new key in the same request.
- A signature over other bytes (a component swapped after signing, a manifest with one byte changed) → `422 signature`, never "unsigned".
- An approval made for the unsigned report, replayed with a signature added (or the reverse) → `422 approve` (the fingerprint carries `signer`).
- A registry index with a valid signature but a lower serial than the cached one, or a component whose bytes differ from the index's `sha256` → refused; the cache is unchanged.
- A registry URL (or a redirect) to `http://`, a private address, or a relative file URL with `..` → refused before any byte is read.
- A key revoked with `compromised` → its plugins disabled on every node; without `compromised` → they keep running, updates signed by it are refused.
- Existing installs after `0015`: same data, same secrets, same fingerprints for unsigned uploads; the first signed install of `seo` claims its data with a "claim" line in the report.

---

### Task 1: Minisign primitives and the signing payload

**Files:**
- Create: `crates/ferrite-domain/src/plugin_trust.rs` (pure), `crates/ferrite-plugin-host/src/trust.rs`, `.github/scripts/plugin-payload.sh`, `crates/ferrite/tests/plugin_payload.rs`
- Modify: workspace `Cargo.toml` (`minisign-verify = { version = "=0.3.0", default-features = false }` and `minisign = { version = "=0.10.0", default-features = false }` under `[workspace.dependencies]`), `crates/ferrite/Cargo.toml` and `crates/ferrite-plugin-host/Cargo.toml` (use the workspace entries; `minisign` as a dev-dependency of plugin-host), `crates/ferrite-domain/src/lib.rs`, `crates/ferrite-plugin-host/src/lib.rs` (`pub mod trust`; `pub use manifest::WORLD` after making it `pub const`), `crates/ferrite/src/update/verify.rs` (use `PublisherKey::parse` + `trust::verify_minisig`; keep `listed_sha256`, `hex`, the placeholder rule and the embedded-key test).

**Interfaces:**
- `ferrite_domain::plugin_trust`: `KeyId` (16 uppercase hex, `Display`, `parse`), `PublisherKey { key_id: KeyId, base64: String }` with `PublisherKey::parse(text) -> Result<Option<PublisherKey>, KeyError>` (minisign key file or bare base64; blank and `#` lines ignored; `None` when nothing is left; algorithm bytes `Ed`; 42 decoded bytes; key id = bytes 2..10 as little-endian u64 `{:016X}`), `PublisherKey::to_file() -> String`, `MAX_SIGNATURE_BYTES = 4096`, `MAX_PUBLISHER_KEY_BYTES = 1024`.
- `ferrite_plugin_host::trust`: `signing_payload(manifest: &[u8], component: &[u8]) -> String` (design §2.1), `verify_minisig(key: &PublisherKey, data: &[u8], signature: &str) -> Result<(), TrustError>` (prehashed only), `signature_key_id(signature: &str) -> Result<KeyId, TrustError>`, `verify_bundle(manifest: &[u8], component: &[u8], signature: &str, candidates: &[PublisherKey]) -> Result<VerifiedSigner { key: PublisherKey }, TrustError>`; `TrustError::{Malformed(String), Legacy, UnknownKey(KeyId), Mismatch, TooLarge}` (`thiserror`).

- [ ] Tests (unit, in both modules): `key_id_matches_minisign_display` (the embedded `release.pub` gives `432051207B9BA065`), `bare_and_file_keys_parse`, `hostile_keys_refused` (wrong algorithm, short, long, not base64, three lines, 2 KiB of text), `payload_is_three_lf_lines`, `bundle_round_trip` (throwaway key via `minisign::sign`), `bundle_other_bytes_mismatch` (one byte of manifest, one of component), `legacy_signature_refused` (`minisign::sign` with prehash off), `unknown_key_names_its_id`, `oversize_signature_refused`, `garbage_signature_malformed`; the existing `update::verify` tests still pass unchanged.
- [ ] `crates/ferrite/tests/plugin_payload.rs`: runs `.github/scripts/plugin-payload.sh` on a temp folder (manifest + `x.wasm`) and asserts its stdout equals `signing_payload` of the same bytes; the script uses `sha256sum`, falling back to `shasum -a 256`; refuses a folder without exactly one `*.wasm`.
- [ ] Implement; `cargo test -p ferrite-domain -p ferrite-plugin-host trust` and `cargo test -p ferrite --lib update::verify` and `--test plugin_payload` pass; clippy clean.
- [ ] Commit `feat(plugin-host): minisign trust primitives and the plugin signing payload`.

### Task 2: Signer in the fingerprint and reserved names

**Files:**
- Modify: `crates/ferrite-plugin-host/src/report.rs` (`permission_report_signed`), `crates/ferrite-plugin-host/src/manifest.rs` (`RESERVED_PLUGIN_NAMES` += `registry`, `publishers`, checked only when `cx.upload`; a stored row named so still loads through `Manifest::from_json`), 05 §2 one sentence on the fingerprint (the rest of 05 is Task 14).

**Interfaces:**
- `Manifest::permission_report_signed(&self, artifact: &str, signer: Option<&KeyId>, current: Option<&Grant>, host: &HostConfig) -> Result<PermissionReport, PluginError>`; `permission_report` = `signed(.., None, ..)`. `PermissionReport.signer: Option<KeyId>`.

- [ ] Tests: the pinned fingerprint test value is unchanged for `None`; `signer_changes_fingerprint`; `same_signer_same_fingerprint`; `reserved_names_refused_on_upload` (`registry`, `publishers`), `reserved_name_row_still_loads` (`from_json`).
- [ ] Implement; `cargo test -p ferrite-plugin-host` passes.
- [ ] Commit `feat(plugin-host): the approval fingerprint covers the signer`.

### Task 3: Storage (migration 0015 and `PluginTrustRepo`)

**Files:**
- Create: `crates/ferrite-store-{sqlite,postgres}/migrations/0015_plugin_publishers.{up,down}.sql`, `crates/ferrite-store-tests/src/plugin_trust.rs`
- Modify: `crates/ferrite-domain/src/plugin_trust.rs` (row types), `crates/ferrite-store/src/repos.rs` (`PluginTrustRepo`, `Repos::plugin_trust`), `crates/ferrite-store-sqlite/src/repos/{mod.rs, plugin_trust.rs (new)}`, `crates/ferrite-store-postgres/src/repos/{mod.rs, plugin_trust.rs (new)}`, both `codec.rs` if they hold the row codecs, `crates/ferrite-store-tests/src/{lib.rs, counting.rs, failing_options.rs, migrations.rs}`, the two backends' contract-test harness lists, `crates/ferrite-store/src/catalogue.rs` (`plugin_publishers`, `plugin_owners` exported rows; `plugin_registry` `Export::Skip` with the reason "a cache, refetched"), any other `Repos` impl found by `rg "fn oauth\(&self\)"` (`ferrite-core/tests/media.rs`, `ferrite-cluster/tests/jobs.rs`, `ferrite-http/tests/app/reload_race.rs`).

**Interfaces:**
- Domain rows: `TrustedKey { key: PublisherKey, label, source: KeySource {Approved, Operator, Registry}, trusted_at, revoked: Option<Revocation { at, reason, compromised }> }`, `PluginOwner { plugin, owner: Option<OwnerId>, signer_key: Option<KeyId>, signature: Option<String>, publisher: String, source: InstallSource {Upload, Folder, Registry}, registry_url: Option<String>, bound_at, updated_at }`, `OwnerId::{Registry { registry_key: KeyId, publisher: String }, Key(KeyId)}` (`reg:<id>:<publisher>` / `key:<id>`, `FromStr`/`Display`), `RegistryCacheRow { url, serial: i64, index_json: String, signature: String, generated_at, fetched_at, checked_at, last_error: Option<String>, last_error_at: Option<Timestamp> }`.
- `PluginTrustRepo` (transactional like the others): `get_key(&KeyId)`, `list_keys()`, `upsert_key(&TrustedKey)`, `revoke_key(&KeyId, &Revocation) -> bool`, `get_owner(&str)`, `list_owners()`, `put_owner(&PluginOwner)`, `delete_owner(&str) -> bool`, `owners_signed_by(&KeyId) -> Vec<PluginOwner>`, `get_registry(&str)`, `put_registry(&RegistryCacheRow)`, `registry_checked(url, at, error: Option<&str>)`.

- [ ] Write `migration_0015_roundtrip` (up, down, up) and contract tests: `owner_round_trip_with_null_owner`, `owner_outlives_plugin_row`, `owners_signed_by_finds_current_signer`, `key_upsert_and_revoke`, `public_key_unique`, `registry_row_replaced_by_url`, `registry_checked_keeps_index`, `export_catalogue_lists_owner_and_key_tables` (and skips `plugin_registry`).
- [ ] Run them on both backends; see them fail; implement; pass (`cargo test -p ferrite-store-sqlite -p ferrite-store-postgres` with `FERRITE_TEST_PG_URL`).
- [ ] Commit `feat(store): plugin publishers, owners and the registry cache (0015_plugin_publishers)`.

### Task 4: The registry index format

**Files:**
- Create: `crates/ferrite-domain/src/plugin_registry.rs`
- Modify: `crates/ferrite-domain/src/lib.rs`, `crates/ferrite-plugin-host/src/trust.rs` (`verify_index`)

**Interfaces:**
- `RegistryIndex { schema, name, serial: u64, generated_at, publishers: Vec<RegistryPublisher { id, name, homepage, keys: Vec<PublisherKey>, revoked_keys: Vec<RevokedKey { key, revoked_at, reason, compromised }> }>, plugins: Vec<RegistryPlugin { name, publisher, description, homepage, license, versions: Vec<RegistryVersion { version: semver-as-String-validated, world, published_at, manifest: RegistryFile, component: RegistryFile, signature: String, yanked: Option<String> }> }> }`, `RegistryFile { url: String, sha256: String, bytes: u64 }`.
- `RegistryIndex::parse(bytes: &[u8]) -> Result<RegistryIndex, RegistryError>` (every rule of design §5.1); `publisher_of_key(&KeyId) -> Option<&RegistryPublisher>`; `revocation(&KeyId) -> Option<&RevokedKey>`; `plugin(name)`; `newest(name, world, include_yanked: bool) -> Option<&RegistryVersion>`; `version(name, &str)`; `search(q, limit) -> Vec<&RegistryPlugin>`; `MAX_INDEX_BYTES`.
- `trust::verify_index(key: &PublisherKey, bytes: &[u8], signature: &str) -> Result<RegistryIndex, TrustError>` (signature first, then parse).

- [ ] Tests (one per rule, hostile inputs): `schema_2_refused`, `unknown_members_ignored`, `duplicate_plugin_refused`, `duplicate_version_refused`, `bad_semver_refused`, `unknown_publisher_refused`, `key_under_two_publishers_refused`, `http_url_refused`, `dotdot_relative_refused`, `leading_slash_and_backslash_refused`, `uppercase_sha_refused`, `component_over_32mib_refused`, `too_many_plugins_refused`, `long_description_refused`, `oversize_index_refused`, `newest_skips_yanked_and_other_worlds`, `search_matches_name_description_publisher_case_insensitively`, `verify_index_refuses_tampered_bytes`.
- [ ] Implement; `cargo test -p ferrite-domain plugin_registry` and `-p ferrite-plugin-host trust` pass.
- [ ] Commit `feat(domain): the signed registry index format (schema 1)`.

### Task 5: Core trust decisions inside the install transaction

**Files:**
- Create: `crates/ferrite-core/src/extensions/trust.rs`, `crates/ferrite-core/tests/plugin_trust.rs`
- Modify: `crates/ferrite-core/src/extensions/{mod.rs, admin.rs}` (`write_install_plugin` takes an `InstallTrust`; `delete_options` path deletes the owner row when the marker is deleted; new service and admin methods), `crates/ferrite-core/src/lib.rs` (re-exports), `crates/ferrite-core/src/jobs.rs` only if a constant is shared.

**Interfaces:**
- `InstallTrust { signer: Option<SignerInput { key: PublisherKey, signature: String, approved_new: bool }>, publisher_claim: String, policy: SignaturePolicy, source: InstallSource, registry: Option<RegistryContext { url, registry_key: KeyId }> }`, `InstallTrust::unsigned(claim)` (policy `Any`, source `Upload`).
- `TrustDecision { signer: Option<SignerView { key_id, source: SignerSource {Registry{publisher_id, name}, Approved, Operator, New}, claimed }>, owner_change: OwnerChange {Bind, BindUnsigned, Same, Claim, UnsignedUpdate}, current_owner: Option<OwnerId>, next_owner: Option<OwnerId>, warnings: Vec<TrustWarning {Unsigned, NewKey, UnsignedDataClaimed, UnsignedReadsData}> }`.
- `pub async fn decide(repos: &dyn Repos, name: &str, trust: &InstallTrust) -> Result<TrustDecision, CoreError>` (reads the owner, `plugin_trust` keys, the cached index of `trust.registry` parsed with `RegistryIndex::parse`; refusals as `CoreError::InvalidField { field: "signature" | "publisher", .. }` with design §4.2 messages).
- `ExtensionService`: `trust_preview(name, &InstallTrust) -> Result<TrustDecision, CoreError>` (read connection), `install_plugin_trusted(&Artifact, &InstalledPlugin, &InstallTrust) -> Result<TrustDecision, CoreError>` (`install_plugin` = trusted with `InstallTrust::unsigned`), `revoke_key(&KeyId, Revocation, label_if_new) -> Result<Vec<String> /* disabled */, CoreError>`, `trust_key(PublisherKey, label)` (operator), `unbind(name) -> bool`, `store_registry(RegistryCacheRow, &RegistryIndex) -> Result<RegistryStored {Stored, Unchanged}, CoreError>` (serial rule: lower refused, equal with other bytes refused, as `CoreError::InvalidField { field: "serial" }`), `registry_failed(url, error)`, `apply_registry_revocations(registry_key, &RegistryIndex) -> Vec<String>`, `list_keys()`, `owner(name)`, `owners()`.
- `ExtensionAdmin`: `install_plugin_trusted(actor, ..)` (audit `plugin.install` + `signer`, `owner`, `source`), `revoke_key(actor, ..)` (audit `plugin.publisher.revoke`), `publishers(actor)`, `owners(actor)`; all `manage_plugins` + `extensions` scope.

- [ ] Tests on both backends (core test harness with `FERRITE_TEST_PG_URL`): one per row of design §4.2's table; `stripped_signature_after_signed_owner_refused`; `other_key_same_name_refused_even_if_approved_new`; `registry_identity_survives_key_rotation` (index lists a second key); `registry_publisher_text_must_match`; `policy_signed_refuses_unsigned`; `policy_trusted_refuses_new_key`; `revoked_key_refuses_install`; `compromised_revocation_disables_and_emits_plugins_changed`; `retired_revocation_keeps_running`; `erase_frees_name` (`purge_plugin`); `purge_true_delete_keeps_binding_until_final_sweep`; `retained_data_bound_refuses_other_publisher`; `unbind_keeps_data`; `serial_rollback_refused`; `same_serial_other_bytes_refused`; `concurrent_installs_of_two_publishers_one_wins` (two tasks, the lock serialises; the loser gets `publisher`); `audit_install_names_signer`; `existing_rows_without_owner_are_unsigned_owned` (insert a `plugins` row with no owner, then an unsigned and a signed install).
- [ ] Implement; `cargo test -p ferrite-core --test plugin_trust` and the existing `extensions*` tests pass.
- [ ] Commit `feat(core): bind plugin data to its publisher and decide trust at install`.

### Task 6: Configuration and the embedded registry key

**Files:**
- Create: `crates/ferrite/registry.pub` (comment-only placeholder, the `release.pub` form), `crates/ferrite/src/registry.rs` (resolution)
- Modify: `crates/ferrite-config/src/{model.rs, load.rs, tests.rs}` (`PluginsConfig.trust: PluginTrustConfig { signatures: SignaturePolicy, registry: Option<String>, registry_key: Option<String> }`), `crates/ferrite/src/serve.rs` (the `PluginsConfig` literal in `host_config_from_plugins_section`), `crates/ferrite/src/lib.rs` (`mod registry`).

**Interfaces:**
- Config rules: `signatures` ∈ `any|signed|trusted` (default `any`); `registry` default `DEFAULT_REGISTRY_URL` (`https://softberries.github.io/ferritecms/registry/v1/index.json`), `""` = off, else `https://…` (with a host) or `file:///absolute/…`; `registry_key` a minisign public key (base64 or file text), required when `registry` is not the default; env `FERRITE_PLUGINS__SIGNATURES`, `FERRITE_PLUGINS__REGISTRY`, `FERRITE_PLUGINS__REGISTRY_KEY` work through the existing override path.
- `crates/ferrite/src/registry.rs`: `pub fn configured(cfg: &PluginTrustConfig) -> anyhow::Result<Option<RegistryTarget { url: Url, key: PublisherKey }>>` (embedded key for the default URL; `None` when off or the embedded file is the placeholder and no key is configured).

- [ ] Tests: config `trust_defaults`, `signatures_values`, `registry_off`, `registry_needs_key_when_custom`, `registry_http_refused`, `registry_file_must_be_absolute`, `bad_registry_key_refused`, env overrides; `registry::configured` with the placeholder (off), with a throwaway key (on), default URL with a configured key (on).
- [ ] Implement; `cargo test -p ferrite-config` and `-p ferrite --lib registry` pass.
- [ ] Commit `feat(config): [plugins] signatures, registry and registry_key`.

### Task 7: The install pipeline verifies signatures (upload and folder)

**Files:**
- Modify: `crates/ferrite-http/src/extensions.rs` (`BundleSignature`, `Installer::with_trust`, `inspect_bundle`, `Inspected::{trust, signer}`, `Staged`), `crates/ferrite-http/src/api/plugins.rs` (parts `signature`, `publisher_key` on inspect and install; `install_plugin_trusted`), `crates/ferrite-http/src/api/dto/extensions.rs` (`TrustDto`, `SignerDto`, `OwnerChangeDto`, `InstalledTrustDto`; `PermissionReportDto.trust`, `PluginDto.trust`; `PluginInspectUpload`/`PluginUpload` schema parts), `crates/ferrite-http/src/api/problem.rs` only if a mapping is missing, `crates/ferrite/src/extensions.rs` (`record_plugin` uses `Staged`), `crates/ferrite/src/serve.rs` (`Installer::with_trust` from config), `crates/ferrite-http/openapi.json`, `crates/ferrite-http/tests/plugins/install.rs` (callers of `stage_plugin`), new `crates/ferrite-http/tests/api/plugins_trust.rs` (+ `mod` in `tests/api/main.rs`).

**Interfaces:**
- `BundleSignature { minisig: String, publisher_key: Option<String> }`; `TrustSettings { policy: SignaturePolicy, registry: Option<RegistryTarget> }` (http-side mirror, no `anyhow`); `Installer::with_trust(self, TrustSettings) -> Self`.
- `Installer::inspect_bundle(&self, manifest_toml: &str, wasm: Bytes, signature: Option<BundleSignature>, source: InstallSource) -> Result<Inspected, ExtensionError>`: size caps, `verify_bundle` with candidates = the given key ∪ the site's trusted keys ∪ the cached index's keys, the report with `permission_report_signed`, `ExtensionService::trust_preview`; `inspect_plugin` = `inspect_bundle(.., None, Upload)`.
- `Installer::stage_plugin(..) -> Result<Staged { artifact, row, trust: InstallTrust }, ExtensionError>` (fingerprint recomputed with the signer).
- `ExtensionError::Trust(TrustError)` → `422`, field `signature`.

- [ ] Tests (`tests/api/plugins_trust.rs`, both backends through the API harness): `signed_upload_reports_new_key_and_installs`, `signed_update_same_key_same_owner`, `stripped_signature_refused_publisher`, `other_key_refused_publisher`, `swapped_component_signature_mismatch`, `unknown_key_without_key_part_refused`, `approval_replay_across_signer_refused`, `oversize_signature_part_413`, `garbage_key_part_422`, `policy_signed_refuses_unsigned_upload` (installer built with `signed`), `plugin_dto_shows_trust_state`, `audit_row_names_signer`, `openapi_snapshot_is_current`.
- [ ] Regenerate `openapi.json`; implement; `cargo test -p ferrite-http --test api plugins` and `--test plugins` pass.
- [ ] Commit `feat(http): verify plugin signatures at install and report the publisher`.

### Task 8: The registry fetcher

**Files:**
- Create: `crates/ferrite-http/src/registry_fetch.rs`, `crates/ferrite-http/tests/plugins/registry_fetch.rs` (+ `mod` in `tests/plugins/main.rs`)
- Modify: `crates/ferrite-http/src/plugins/egress.rs` (`registry_client(timeout)` and `registry_client_for_tests(overrides, timeout)`, built like `metadata_client`), `crates/ferrite-http/src/lib.rs`.

**Interfaces:**
- `#[async_trait] pub trait RegistrySource: Send + Sync { async fn get(&self, url: &Url, max: u64, timeout: Duration) -> Result<Bytes, RegistryFetchError>; }`; `RegistryFetchError::{Refused(String), Failed(String), TooLarge, NotFound}`.
- `HttpsRegistry::new(version) / for_tests(EgressOverrides)` (design §5.3); `FileRegistry::new(root: PathBuf)` (canonicalise, prefix check, `max` before reading); `resolve(index_url: &Url, file_url: &str) -> Result<Url, RegistryFetchError>` (relative against the index; absolute `https` only for https registries; refused in file registries); `source_for(url, version) -> Result<Arc<dyn RegistrySource>, String>`.

- [ ] Tests: `https_only` (first URL and redirect), `redirects_capped_at_three`, `credentials_refused`, `private_address_refused` (production rule with a resolver answering 127.0.0.1), `content_length_over_max_refused_before_body`, `streamed_body_over_max_stops`, `local_tls_server_fetch_works` (seam: test resolver + self-signed root, as `cimd_client_fetched_from_local_server`), `file_registry_reads_inside_root`, `file_registry_refuses_dotdot_and_symlink_escape`, `file_registry_refuses_absolute_urls`, `resolve_relative_against_index`.
- [ ] Implement; `cargo test -p ferrite-http --features test-seams --test plugins registry_fetch` passes.
- [ ] Commit `feat(http): SSRF-checked registry fetcher and file registries`.

### Task 9: Registry refresh, inspect and install in the pipeline

**Files:**
- Modify: `crates/ferrite-http/src/extensions.rs` (`Installer::{refresh_registry, registry_view, inspect_registry}`; `TrustSettings` gets the `Arc<dyn RegistrySource>`), `crates/ferrite/src/serve.rs` (wire the source), new `crates/ferrite-http/tests/plugins/registry.rs` with a fixture builder `tests/plugins/common.rs::FileRegistryFixture` (writes manifest, wasm, signatures, `index.json`, `.minisig` with throwaway keys).

**Interfaces:**
- `refresh_registry(&self) -> Result<RegistryRefresh { stored: RegistryStored, disabled: Vec<String> }, ExtensionError>`: fetch index + `.minisig` (30 s, `MAX_INDEX_BYTES`), `verify_index`, `ExtensionService::store_registry`, `apply_registry_revocations`; failures recorded with `registry_failed`. New `ExtensionError::{RegistryUnavailable(String), RegistryInvalid(String), NoRegistry}`.
- `registry_view(&self, q: Option<&str>) -> Result<RegistryView, ExtensionError>` (cache only; installed state and update availability per design §8).
- `inspect_registry(&self, name: &str, version: Option<&str>) -> Result<Inspected, ExtensionError>`: entry from the cache (newest supported, non-yanked by default), fetch manifest (64 KiB) and component (32 MiB, 60 s), check `bytes` and `sha256` against the entry, then `inspect_bundle` with the entry's signature and the publisher's keys, `InstallSource::Registry`.

- [ ] Tests (file registry, both backends where the store is involved): `refresh_stores_and_lists`, `refresh_tampered_index_keeps_cache`, `refresh_rollback_refused`, `refresh_applies_compromised_revocation`, `inspect_from_registry_reports_verified_publisher`, `registry_install_claims_unsigned_owner` (an unsigned v1 installed first, then the registry's signed v2), `component_hash_mismatch_refused`, `yanked_version_not_default_but_installable_by_version`, `other_world_skipped`, `update_available_only_for_same_publisher`.
- [ ] Implement; `cargo test -p ferrite-http --test plugins registry` passes.
- [ ] Commit `feat(http): install and update plugins from a signed registry`.

### Task 10: Registry and publisher API endpoints

**Files:**
- Modify: `crates/ferrite-http/src/api/plugins.rs` (`get_registry`, `refresh_registry`, `inspect_registry_plugin`, `install_registry_plugin`, `list_publishers`, `revoke_publisher`), `crates/ferrite-http/src/api/mod.rs` (routes, registered before `/plugins/{name}`), `crates/ferrite-http/src/api/dto/extensions.rs` (`RegistryDto`, `RegistryPluginDto`, `RegistryInstallRequest`, `PublisherKeyDto`, `RevokeKeyRequest`), `crates/ferrite-http/src/api/openapi.rs`, `crates/ferrite-core/src/auth/limits.rs` (`registry_refresh` 1 / 60 s per site), `crates/ferrite-http/openapi.json`, `crates/ferrite-http/tests/api/plugins_trust.rs`, `crates/ferrite-http/tests/api/mcp_admin.rs` (`plugins_list` carries `trust`), `site/content/05-reference/02-mcp-tools.md` (one sentence on `plugins_list`).

**Interfaces:** design §7.3; errors `502 registry-unavailable`, `422 registry-invalid`, `404` when no registry is configured for inspect/install (`GET` answers `registry: null`), `429` refresh limit.

- [ ] Tests: `get_registry_without_config_is_null`, `refresh_then_search`, `refresh_rate_limited`, `registry_inspect_then_install_201`, `registry_install_approval_mismatch_422`, `publishers_list_merges_site_and_registry_keys`, `revoke_compromised_disables_plugin`, `revoke_needs_manage_plugins` (editor 403, token without `extensions:write` 403), `reserved_names_route_correctly` (`/plugins/registry` is not a plugin named `registry`), `mcp_plugins_list_includes_trust`, `openapi_snapshot_is_current`.
- [ ] Regenerate `openapi.json`; implement; `cargo test -p ferrite-http --test api` passes.
- [ ] Commit `feat(api): registry search, refresh and install; publisher keys and revocation`.

### Task 11: Daily registry check and doctor

**Files:**
- Create: `crates/ferrite/src/jobs/registry.rs`
- Modify: `crates/ferrite/src/jobs.rs` (`REGISTRY_CHECK_KIND = "plugins.registry_check"`, `registry_check_periodic()` daily, in `core_periodics()`; the handler in `handlers(..)`), `crates/ferrite/src/serve.rs` (handler wiring), `crates/ferrite/src/doctor/db.rs` (`plugins.trust`), `crates/ferrite/src/doctor/mod.rs` (check list), `crates/ferrite/tests/doctor.rs` (count 26), `crates/ferrite/tests/node.rs` or a new `crates/ferrite/tests/registry_job.rs`.

**Interfaces:** `RegistryCheckHandler { installer: Arc<Installer> }`: no registry → done; network error → `JobError::Retry`; invalid index → `JobError::Permanent` after recording the error. Doctor `plugins.trust` per design §9.

- [ ] Tests: `job_refreshes_cache_from_file_registry`, `job_without_registry_is_done`, `job_records_invalid_index_and_stops`, `periodic_is_daily_and_registered`; doctor `trust_ok`, `trust_warns_unsigned_under_signed_policy`, `trust_warns_revoked_signer`, `trust_warns_stale_check`, `trust_info_registry_off`, `doctor_counts_26_checks`.
- [ ] Implement; `cargo test -p ferrite --test doctor --test registry_job` and `--lib jobs` pass.
- [ ] Commit `feat(jobs): daily registry check with revocations; doctor plugins.trust`.

### Task 12: CLI — signed folders, registry commands, keys

**Files:**
- Modify: `crates/ferrite/src/ext_cmd.rs` (`Check`/`Install` read `ferrite-plugin.minisig` and `ferrite-plugin.pub`; `Install { name: Option<String>, dir: Option<PathBuf>, .. }` with `NAME[@VERSION]`; `Search`, `Update`, `Digest`, `Keys(KeysCommand { List, Trust, Revoke })`, `Unbind`; trust lines in the printed report), `crates/ferrite/src/extensions.rs` (`install_plugin_file` and `record_plugin` with trust), new `crates/ferrite/tests/plugin_trust_cli.rs`, `crates/ferrite/tests/cli.rs` (help snapshot if it lists subcommands).

**Interfaces:** design §9 table and exit codes; `digest` needs no `--data`; registry commands build the `Installer` with the configured source and always refresh first.

- [ ] Tests (assert_cmd style as in `tests/extensions.rs`, SQLite and Postgres where they open a store): `check_prints_verified_publisher`, `install_dir_with_signature_records_owner`, `install_dir_stripped_signature_refused_exit_1`, `install_name_from_file_registry`, `install_name_at_version`, `install_name_and_dir_usage_error`, `search_lists_matches`, `update_check_exits_2_with_updates_0_without`, `update_installs_after_yes`, `digest_matches_payload`, `keys_trust_list_revoke`, `unbind_then_other_publisher_installs`, `no_registry_search_exit_1`.
- [ ] Implement; `cargo test -p ferrite --test plugin_trust_cli --test extensions` pass.
- [ ] Commit `feat(cli): signed plugin folders, ferrite plugin search|install NAME|update, keys and unbind`.

### Task 13: Admin — trust in the report, signed uploads, list badges

**Files:**
- Create: `admin/src/plugins/trust-summary.tsx` (+ `.test.tsx`)
- Modify: `admin/src/api/schema.d.ts` (`pnpm -C admin api:gen`), `admin/src/api/plugins.ts` (inspect/install send `signature`, `publisher_key`), `admin/src/plugins/{permission-report.tsx, install-flow.tsx, plugins-list.tsx, plugin-model.ts, fixtures.ts}` and their tests, `admin/src/i18n/en/admin.json`.

**Interfaces:** design §10 (report, dialog, list). The install dialog keeps its two required inputs and adds two optional ones; dropped files are matched by name (`ferrite-plugin.minisig`, `ferrite-plugin.pub`, `*.toml`, `*.wasm`).

- [ ] Vitest: `trust_summary_verified|new_key|unsigned|claim` (text, icon, warning style; no colour-only state), `install_sends_signature_parts`, `install_shows_publisher_refusal_from_422`, `list_shows_publisher_state_as_text`, `list_shows_update_chip`, `hostile_publisher_text_rendered_as_text`.
- [ ] Implement; `pnpm -C admin test`, `pnpm -C admin lint` and `pnpm -C admin typecheck` pass.
- [ ] Commit `feat(admin): verified publisher in the permission report and signed uploads`.

### Task 14: Admin — registry browser, publishers page, plugin detail; E2E

**Files:**
- Create: `admin/src/plugins/registry-browser.tsx`, `admin/src/plugins/publishers.tsx`, `admin/src/routes/_app/plugins.publishers.tsx` (+ tests), `admin/e2e/support/minisign.ts` (+ `minisign.unit.ts`), `admin/e2e/support/registry-fixture.ts`, `admin/e2e/plugins-trust.spec.ts`
- Modify: `admin/src/routes/_app/plugins.index.tsx` (`tab` search param), `admin/src/routeTree.gen.ts` (generated), `admin/src/plugins/{plugin-detail.tsx, install-flow.tsx}` (registry source), `admin/src/api/plugins.ts` (registry and publisher calls), `admin/src/i18n/en/admin.json`, `admin/e2e/support/{global-setup.ts, server.ts}` (a `file://` registry with throwaway keys in the E2E sites' env: `FERRITE_PLUGINS__REGISTRY`, `FERRITE_PLUGINS__REGISTRY_KEY`).

**Interfaces:** design §10. `e2e/support/minisign.ts`: `generateKey() -> { publicFile, keyId, sign(data: Buffer): string }` (Ed25519 over BLAKE2b-512 of the data, `ED` algorithm, trusted comment, global signature), its unit test verifying a signature with the `minisign` format rules (and, when the `minisign` CLI is on the PATH, with it).

- [ ] Vitest: `registry_browser_search_and_empty_states`, `registry_install_uses_report_step`, `publishers_revoke_dialog_requires_reason`, `detail_shows_publisher_and_update`.
- [ ] Playwright (`plugins-trust.spec.ts`, Chromium, both backends): install the signed fixture plugin from the Registry tab (report shows "Verified publisher"), update it to the fixture's v2 from the list's chip, a stripped-signature upload refused with the publisher message, revoke the publisher key as compromised and see the plugin disabled, axe on the Registry tab and the Publishers page; remove plugins and data in `finally`.
- [ ] Implement; `pnpm -C admin test`, `pnpm -C admin e2e --project chromium-sqlite --project chromium-pg plugins-trust` pass (after `pnpm -C admin build && cargo build --release -p ferrite` and the fixture wasm build).
- [ ] Commit `feat(admin): registry browser, publisher keys and plugin updates`.

### Task 15: CI — sign and publish the example plugins and the index

**Files:**
- Create: `registry/publishers.json` (publisher `ferritecms.dev` with a placeholder key note; the user adds the real public key with the secrets), `registry/yanked.json` (`[]`), `.github/scripts/registry-index.py`, `crates/ferrite/tests/registry_script.rs`
- Modify: `.github/scripts/package-plugins.sh` (pack from a staged folder that may hold `ferrite-plugin.minisig`/`.pub`; write `ferrite-plugin-<name>-<version>.{toml,wasm}`), `.github/workflows/release.yml` (`plugins` uploads `plugins-stage`; new `plugins-sign` (environment `release`, `PLUGINS_SECRET_KEY`/`PLUGINS_PASSWORD`, `REGISTRY_SECRET_KEY`/`REGISTRY_PASSWORD`) and `plugins-sign-dry-run` (throwaway keys); `sign`/`sign-dry-run` need them; `release` dispatches `pages.yml`, `actions: write`), `.github/workflows/pages.yml` (download, verify with `crates/ferrite/registry.pub`, place `registry/v1/index.json(.minisig)`; skip with a notice when the latest release has none), `.github/workflows/ci.yml` (`packaging` job: run both scripts on fixtures with throwaway keys), `.github/scripts/verify-sig.sh` only if a second key path is needed.

**Interfaces:** `registry-index.py --previous FILE|- --publishers registry/publishers.json --yanked registry/yanked.json --version V --base-url https://github.com/softberries/ferritecms/releases/download/vV --plugins DIR --out FILE` (design §11; refuses a previous index whose plugins the new one would drop, bumps `serial`).

- [ ] `crates/ferrite/tests/registry_script.rs` (skips with a printed `SKIP` when `python3` is missing, fails when `CI=true`): builds a staged folder signed with a throwaway key, runs the script twice (serial 1, then 2 with the first as previous), signs with a throwaway registry key, and checks the output with `trust::verify_index`, `RegistryIndex::parse` and `verify_bundle` for each version.
- [ ] Run `actionlint` / `shellcheck` on the changed workflow and scripts if available; a manual `release.yml` dry run is listed for the exit (not runnable locally).
- [ ] Commit `ci: sign the example plugins and publish the registry index on Pages`.

### Task 16: Documentation

**Files:**
- Modify: `docs/design/05-plugin-host.md` (§2 "Plugin data is bound to the plugin name only (R22)" replaced by the owner binding, pointing at 14; §2 fingerprint and parts), `docs/design/07-admin-api.md` (§1 endpoint rows, §3 Plugins row), `docs/design/09-operations.md` (§7 release jobs and the two new keys; a "Plugin keys" runbook paragraph: create the keys, put the public halves in `crates/ferrite/registry.pub` and `registry/publishers.json`, store the secrets, revoke), `docs/design/01-architecture.md` (crate map lines), `site/content/02-guides/05-plugins.md` (verified publishers, registry, updates, policies), `site/content/03-developers/02-plugin-authoring.md` (signing with minisign, `ferrite plugin digest`, `ferrite-plugin.pub`), `site/content/05-reference/01-cli.md` (new commands), `site/content/04-operations/01-configuration.md` (`[plugins] signatures|registry|registry_key`), `site/VERIFIED.md` (rows for each new claim), `CLAUDE.md` (status, crate map, commands, doctor count 26).
- [ ] Every command and config key in the docs checked against `--help` output and `ferrite-config` tests; `cargo test -p ferrite --test docs_site` passes.
- [ ] Commit `docs: plugin trust, signing and the registry`.

### Task 17: Exit

**Files:**
- Create: `docs/superpowers/m6-2-exit.md`
- Modify: `docs/ROADMAP.md` (tick the three M6-2 items; follow-ups listed under M6-2), `docs/superpowers/rulings/m6.md` (create or append the M6-2 rulings made during the work)

- [ ] Run the whole suite: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `FERRITE_TEST_PG_URL=… cargo test --workspace`, `pnpm -C admin test`, `pnpm -C admin e2e --project chromium-sqlite --project chromium-pg` including `plugins-trust.spec.ts`, the axe projects.
- [ ] A manual end-to-end check on a local site: sign the three example plugins with a throwaway key, serve a `file://` registry, `ferrite plugin search`, `install seo`, `update --check`, the admin's Registry tab and update chip; record the commands and output.
- [ ] Write `m6-2-exit.md`: what was verified (with commands), what was **not** (the real `release.yml` run with the user's keys, the first Pages publication of the index, GitHub's asset redirect host and the `workflow_dispatch` from `GITHUB_TOKEN`, Firefox/WebKit runs of the new spec if not run, a real-world HTTPS registry fetch), the keys the user must create (`PLUGINS_*`, `REGISTRY_*`, `crates/ferrite/registry.pub`, `registry/publishers.json`), and the follow-ups of design 14 §14.
- [ ] Commit `docs: M6-2 exit`.
