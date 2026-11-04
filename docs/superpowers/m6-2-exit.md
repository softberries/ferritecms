# M6-2 exit: plugin trust

**Criterion** ([ROADMAP](../ROADMAP.md), M6-2; design [14](../design/14-plugin-trust.md);
rulings [m6.md](rulings/m6.md) P5): plugins can be signed with a publisher's minisign key;
install verifies the signature and the permission report names the verified publisher; a
plugin name's data is bound to its publisher so a same-named plugin from someone else cannot
read it; a signed static registry gives `ferrite plugin search|install NAME|update`, update
checks on the Plugins screen, and CI publishes the three example plugins there.

**Result: built** on branch `m6-2-plugin-trust` (from `m6`), 2026-11-03. The publishing half
waits for the user's keys (ruling P5): until they exist the release packs the plugins
unsigned and publishes no index, and `crates/ferrite/registry.pub` is a placeholder, so the
default registry is off in every build. Everything was tested with throwaway keys.

## What was built

| Area | Delivered |
|---|---|
| Formats | `ferrite_domain::plugin_trust` (key ids, `PublisherKey`, `MinisigHeader`, owner ids, rows, `SignaturePolicy`), `ferrite_domain::plugin_registry` (index schema 1, every rule of 14 §5.1 with a hostile test, search, newest per world, `is_newer`) |
| Crypto | `ferrite_plugin_host::trust`: the signing payload `ferrite-plugin-signature/1`, `verify_minisig` (prehashed only), `verify_bundle`, `verify_index`; `self-update` uses the same verifier (D5); `minisign-verify` is now a workspace dependency of the plugin host only |
| Fingerprint | `permission_report_signed`: `"signer"` in the fingerprint only when signed (D11; the pinned unsigned values are unchanged); `registry`, `publishers` reserved for uploads |
| Storage | migration `0015_plugin_publishers` on both backends (`plugin_publishers`, `plugin_owners`, `plugin_registry`), `PluginTrustRepo` (own module `ferrite-store/src/plugin_trust.rs`), contract tests on both; keys and owners exported, the cache skipped |
| Core | `extensions::trust::decide` (the 14 §4.2 table, policy, revocations, registry publisher text), run again inside the locked install transaction; `install_plugin_trusted`, `revoke_key` (compromised disables and emits `plugins.changed`), `trust_key`, `unbind`, `store_registry` (serial rule and revocations in one transaction); erase deletes the owner row; audit `plugin.install` gains `signer`, `owner`, `source`; `plugin.publisher.revoke` |
| Config | `[plugins] signatures`, `registry`, `registry_key`, `registry_check` (`off` default, ruling P5), env overrides, the init template |
| HTTP | signed uploads (`signature`, `publisher_key` parts), `TrustDto` on reports, `InstalledTrustDto` on plugins (state, owner, signer, source, update); `registry_fetch` (HTTPS through the egress SSRF rules, ≤ 3 redirects, size caps before and while reading; `file://` registries confined to their folder); registry refresh/view/inspect in the installer; endpoints `GET /plugins/registry`, `POST …/refresh` (1/min/site), `…/inspect`, `…/install`, `GET /plugins/publishers`, `POST …/{key_id}/revoke`; OpenAPI regenerated |
| Jobs, doctor | `plugins.registry_check` (handler always; daily periodic only with `registry_check = "daily"`), doctor `plugins.trust` (26 checks) |
| CLI | signed folders in `check`/`install --dir`, `install NAME[@VERSION]`, `search`, `update [--check]` (exit 2), `digest`, `keys list|trust|revoke`, `unbind` |
| Admin | Publisher block in the report, signed uploads (two optional inputs, drop by name), refusals shown with Approve disabled, publisher state in words, Update chip, Check for updates, Installed/Registry tabs, Publisher keys page with Revoke, the plugin page's Publisher section and Update; Vitest; E2E `plugins-trust.spec.ts` with Node-crypto minisign (D19) |
| CI | `plugin-payload.sh`, `sign-plugins.sh`, `verify-file.sh`, `registry-index.py`; `package-plugins.sh` (`SIGNATURES_DIR`, `ASSETS`), `sums.sh`; `release.yml` `plugins-sign` / `plugins-sign-dry-run`, asset count 9 or 17, Pages dispatch; `pages.yml` places the verified index; CI `packaging` runs the signing path with throwaway keys |
| Docs | design 14 (P5 applied), 05, 07, 09 (§7 jobs and the "Plugin keys" runbook), 01; guides Plugins, Plugin authoring, CLI and Configuration reference, MCP tools note; `site/VERIFIED.md` rows; `CLAUDE.md`; ROADMAP ticked with follow-ups |

## Keys and secrets the user must create (ruling P5)

Nothing here was created or stored by the agent. Once, by a maintainer (09 §7 "Plugin keys"):

1. **Plugin publisher key** (`ferritecms.dev`, signs the three example plugins):
   `minisign -G -p plugins.pub -s plugins.key`. Put the second line of `plugins.pub` (the
   base64 key) into `registry/publishers.json` → `publishers[0].keys` and commit it. Store the
   contents of `plugins.key` as the secret `PLUGINS_SECRET_KEY` and its password as
   `PLUGINS_PASSWORD` in the GitHub environment `release`.
2. **Registry index key** (signs `index.json`): `minisign -G -p registry.pub -s registry.key`;
   `cp registry.pub crates/ferrite/registry.pub` (replaces the placeholder; commit it). Store
   `registry.key` as `REGISTRY_SECRET_KEY` and its password as `REGISTRY_PASSWORD` in `release`.
3. Keep offline copies of both secret keys and passwords. Pages must stay on "GitHub Actions"
   (already set for the docs site); `release.yml` asks for `actions: write` to start
   `pages.yml` itself.

Until all four secrets and both public halves are in place, `plugins-sign` logs a notice and
packs the plugins unsigned; `pages.yml` skips the index with a notice.

## Execution decisions

- **R-2-1** `PluginTrustRepo` lives in its own module (`ferrite-store/src/plugin_trust.rs`); the
  shared `repos.rs` gets one `Repos` method, the backends' `repos/mod.rs` one module line and
  one delegate each.
- **R-2-2** The migration tests no longer pin version lists: `ferrite-store-tests`
  (`versions`, `down_from`, `up_from` in `migrations.rs`, used by `artifact_gc.rs` and
  `plugin.rs`), `ferrite-store-sqlite` (`migrate.rs`), `ferrite` `tests/cli.rs`, `tests/export_cli.rs`,
  `tests/rollback.rs` and `tests/upgrade.rs` derive them from the migration folder, so 0014 and 0016 landing beside
  0015 need no edit there (merge: take these versions). The fabricated schema-10 archive in
  `export_cli.rs` drops the tables newer than schema 10 (`plugin_publishers`,
  `plugin_owners`), as a real schema-10 export would not have them.
- **R-2-3** `store_registry` stores the index and applies its revocations in one transaction
  (the plan had two calls).
- **R-2-4** Core never reads the registry cache itself: the HTTP installer verifies the cached
  index's signature with the configured key on every use (`registry_context`) and passes the
  parsed index in `RegistryContext`, so a cache written under another key is never trusted.
- **R-2-5** A trust refusal at inspection is kept in `Inspected` (`TrustRefusal`) and answered
  as `422` with the report (`report.trust.refusal`); `stage_plugin` refuses it again.
- **R-2-6** A malformed or oversize key part is field `publisher_key`; a key without a
  signature is refused the same way.
- **R-2-7** `config check`'s summary line is unchanged (tests pin it); it warns only when
  `registry_check = "daily"` has no registry. Doctor reports the registry state.
- **R-2-8** Doctor's "older than 3 days" warning applies only with `registry_check = "daily"`
  (ruling P5: an on-demand site would always warn).
- **R-2-9** The registry check is due at 04:00 UTC (after the artifacts GC at 03:00).
- **R-2-10** `TrustDto` gains `refusal`; `InstalledTrustDto` is `{state, owner, signer, source,
  update}`; `PublisherKeyList` carries the policy.
- **R-2-11** CLI trust tests run on SQLite; the Postgres paths of the same code are covered by
  the core, API and job tests on both backends.
- **R-2-12** The Rust registry fixture (`crates/ferrite-http/tests/support/registry_fixture.rs`)
  is shared by three test crates through `#[path]` and carries `#![allow(dead_code)]`, since each
  test binary uses a different part of it.
- **R-2-13** CI signing is one script (`sign-plugins.sh`) for the release, the dry run and the
  CI `packaging` job; `verify-file.sh` (exit 3 for a placeholder key) checks every signature.
  The `plugins` job uploads the wasm (`plugins-wasm`) and the sign jobs pack. `BASE_URL=""`
  makes an index whose files are relative (a `file://` registry from a release folder).
- **R-2-14** `plugins-trust.spec.ts` is a site-wide spec (runs after the rest, alone): it
  installs and removes a plugin every list shows and revokes the site's registry key.
- **R-2-15** A failed refresh is recorded only on an existing cache row; a first fetch that
  fails leaves "not checked yet" (follow-up in the ROADMAP).
- **R-2-16** Revoking a key the site has no row for (a registry key) inserts a row with source
  `registry` and the publisher's name as its label.
- **R-2-17** The key id of a signature is read in `ferrite-domain` (`MinisigHeader`), since
  `minisign-verify` does not expose it.
- **R-2-18** `registry` and `publishers` are refused for uploads only; `inspect` and `retained`
  stay refused everywhere, as before.

Review fixes (after the first exit; one critical, four minor):

- **R-2-19** (critical) Key-id confusion. A minisign key id is 8 bytes anyone can choose, so
  identity is decided by the whole public key: `RegistryIndex::publisher_of_key` and
  `revocation` take a `&PublisherKey`; `plugin_owners` gains `signer_public_key` (0015,
  unreleased, so edited in place) and `PluginOwner::signer()` returns it only when its id is
  the row's `signer_key`; `verify_bundle` refuses two candidates with one id and different
  bytes (`KeyConflict`); install refuses a signer whose id the cached index lists under other
  bytes (any claim, any name); `keys trust` refuses one (field `key`); every registry
  refresh deletes colliding site key rows, disables the enabled plugins they signed (reason
  names the key and the registry publisher), audits `plugin.publisher.collision` and returns
  `collisions` (CLI warning, Plugins-screen toast). Until a refresh runs, a site that
  approved a forged key refuses every bundle signed by the genuine registry key (the site
  row holds other bytes under that id): fail closed. `update_for` offers an update for a `key:` owner
  only when that id's stored whole key is the registry publisher's.
- **R-2-20** `signatures = "trusted"` refuses an approved (TOFU) key as well as a new one.
- **R-2-21** `plugin_registry` is exported (`Export::Rows`), so `export`/`import-site`/`db copy`
  carry the url and the serial: the copy keeps its rollback floor. The index is re-verified on
  use, so a copied row grants nothing by itself.
- **R-2-22** A refresh verifies the cached row under the current `registry_key` first; a row
  that does not verify (the key changed) is replaced without the serial rule
  (`store_registry(.., stale)` matches the exact cached text, so a concurrent newer row is
  still compared).
- **R-2-23** 0015 is declared `-- ferrite:lossy-down` (the down drops bindings, trusted keys and
  revocations); `rollback --schema` past it needs `--allow-lossy`.

## Verified

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`: clean.
- `FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test cargo test --workspace
  --no-fail-fast` (Postgres 17): 4 398 passed, 21 failed in the full run; 20 were the
  `rollback.rs` and `upgrade.rs` tests computing `latest - 1` across the version gap (0014 not
  on this branch), fixed by R-2-2 and then 38/38 passing; the remaining one,
  `import_cli::import_site_a_end_to_end_sqlite`, fails only because it serves on the fixed
  port 127.0.0.1:8080, which the user's own local site holds on this machine (unrelated to
  M6-2; not re-run with the port free).
- Admin: `pnpm -C admin typecheck`, `lint`, `test` (Vitest 848/848, including the E2E support
  unit tests: the Node minisign signer verified by the `minisign` CLI 0.12).
- E2E (release build, `pnpm -C admin build`), Chromium on SQLite and Postgres:
  `plugins-trust.spec.ts` 2/2 (registry check, install with Verified publisher, Update chip
  0.1.0 → 0.2.0, unsigned upload refused with the publisher message, compromised revocation
  disables the plugin, axe on the Registry tab and the Publishers page) and `plugins.spec.ts`
  14/14.
- `actionlint` on `release.yml`, `pages.yml`, `ci.yml`; `shellcheck -x .github/scripts/*.sh`.
- `sign-plugins.sh` run locally with throwaway keys (twice: serial 1, then 2 on top);
  `registry_script.rs` checks the script's output with the binary's reader.
- **Manual end-to-end check** (release build, throwaway keys made with `minisign -G -W`):
  `BASE_URL="" EXTRA_KEY=plugins.pub .github/scripts/sign-plugins.sh 0.2.0 reg …` signed the
  three example plugins and wrote a `file://` registry; on a fresh `init` site with
  `registry = "file://…/reg/ferrite-registry-index.json"` and its `registry_key`:
  `ferrite plugin search` listed analytics, contact-form, seo 0.2.0 by "FerriteCMS
  (Softberries)"; `ferrite plugin install seo --yes` printed `publisher: verified
  ferritecms.dev (… registry key …)` and `data: bound to reg:<registry key>:ferritecms.dev`;
  `update --check` printed `no updates` (exit 0); `plugin check --dir` on the unpacked signed
  `analytics` folder from the tarball printed the verified publisher; `doctor --offline`
  printed `ok plugins.trust 1 plugins, 1 verified; registry serial 1 checked 0 min ago`. The
  admin's Registry tab and Update chip were exercised by the E2E spec above.

- Review fixes (R-2-19..23), same gates again: fmt, clippy, doc clean; the full workspace run
  on Postgres 17 gave 4 409 passed, 26 failed. 25 of those were tests that 0015's new
  lossy-down mark affected (migration round trips naming "migration 10", plain
  `rollback --schema` with no `--allow-lossy`). Two more were the GET registry body (now has
  `collisions`) and a registry inspect with a revoked key, which now gets its trust
  refusal back before the publisher check. After the fixes, `contract` 529/529, `rollback`
  18/18, `api` 526/526 and `plugins` 289/289 pass. `import_cli` still fails on the busy
  port 8080, as above. New regression tests: `forged_key_with_registry_key_id_is_not_the_registry`,
  `refresh_removes_colliding_key_and_disables_its_plugins`,
  `trust_key_refuses_a_registry_id_collision`, `policy_trusted_refuses_approved_key`,
  `stale_cache_is_replaced_without_the_serial_rule` (core, both backends),
  `same_id_other_key_is_not_the_registry_key` (domain), `same_id_two_keys_conflict`
  (plugin host), `key_delete_and_owner_signer_key` (store contract),
  `rekeyed_registry_replaces_the_old_cache` (HTTP), `migration_0015_roundtrip` now requires
  `allow_lossy`, the export tests carry `plugin_registry`, and the Vitest
  `registry_check_warns_about_key_collisions`. Admin typecheck, lint and Vitest pass
  (849/849). The E2E specs were not re-run after these fixes. The only admin change is a
  toast shown when `collisions` is non-empty, and the specs never produce a collision.

## Not verified

- A real `release.yml` run: neither the publishing run with the user's keys nor a manual dry
  run on GitHub (not runnable locally; the steps were run as scripts).
- The first Pages publication of the index, GitHub's asset redirect host for the registry
  downloads, and `gh workflow run pages.yml` from `GITHUB_TOKEN`.
- A real-world HTTPS registry fetch (the fetcher was tested against a local TLS server with a
  test resolver and the production address rule).
- Firefox and WebKit runs of the new spec (the full cross-browser run is at the end of M6).
- The CLI trust commands on Postgres (R-2-11); doctor `plugins.trust` on a Postgres site.
- Two cluster nodes: that a compromised revocation unloads the plugin on every node (the
  `plugins.changed` event is asserted; the reload path is the existing one).

## Follow-ups

Listed in the ROADMAP under M6-2: create the keys and run the first signed release; erase
`plugin.<name>.*` content meta with a plugin's data (design 14 §12, P5: Later); record a
failed first registry fetch; design 14 §14 (third-party submissions, signed themes,
transparency log, index expiry).

## Keys created (2026-11-04)

The two keys of ruling P5 exist: the `ferritecms.dev` publisher key (key id
`B1AE6DB7CD1D85B2`, its public line in `registry/publishers.json`) and the registry index key
(key id `B1D92B9335F48FD5`, `crates/ferrite/registry.pub`), each with its own random
password. The secret halves and passwords are kept outside the repository by the user (and
offline copies), and are stored in the `release` environment as `PLUGINS_SECRET_KEY`,
`PLUGINS_PASSWORD`, `REGISTRY_SECRET_KEY`, `REGISTRY_PASSWORD` (2026-11-04). Builds from this
commit on have the default registry on. `release.yml`'s first-index check (no earlier
release carries `ferrite-registry-index.json`) no longer depends on a pipeline's status.
Verified locally with the real keys: `sign-plugins.sh` into a scratch folder,
`verify-file.sh` against the committed `registry.pub`, and a `file://` copy of the index
searched and installed (`seo`, verified publisher `ferritecms.dev`) by a build using only the
embedded key. Not verified until the next `v*` tag: the publishing `release.yml` run with
these secrets, the Pages placement and `gh workflow run pages.yml`, and an HTTPS fetch of
`https://softberries.github.io/ferritecms/registry/v1/index.json`.
