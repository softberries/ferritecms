# 14 — Plugin trust: signed plugins, publisher-bound data, the registry

Status: **built in M6-2** (ruling P5 applied; execution notes `R-2-*` in
`docs/superpowers/m6-2-exit.md`) · Date: 2026-11-02

Today a plugin is two files, `ferrite-plugin.toml` and a `.wasm`, and nothing says who
made them. The manifest's `publisher` is unverified text, and plugin data (KV, settings,
sealed secrets) belongs to the plugin *name* only, so a component from anyone, installed
under a name that was used before, reads the earlier plugin's data and secrets (05 §2,
M2a-2a ruling R22 M3). There is also no way to find plugins or learn that a newer version
exists: an admin downloads a tarball from a release page.

M6-2 adds three things:

1. **Signatures.** A publisher signs a plugin with a minisign key, as FerriteCMS signs its
   releases. Install verifies the signature and the permission report names the verified
   publisher (§2, §3).
2. **Publisher-bound data.** The plugin name's data is bound to the publisher that first
   installed it with a signature. A same-named plugin from another publisher (or an
   unsigned one) cannot be installed over it until that data is erased (§4).
3. **A registry.** A signed static JSON index, hosted on GitHub Pages for the official
   plugins: `ferrite plugin search|install NAME|update`, update checks on the Plugins screen,
   and the three example plugins published there by CI (§5–§9).

Nothing here changes the WIT world (CLAUDE.md principle 4): trust is decided by the host
before a component is stored. Themes are not signed (§12).

## 1. Goals and non-goals

Goals:

1. **The report tells the truth about origin.** "Verified: FerriteCMS (registry)", "Signed
   by key 5A1C… (new to this site)" or "Unsigned", never the manifest's own claim.
2. **No silent takeover.** Once a plugin name's data belongs to a publisher, only that
   publisher's signed versions use it. Swapping in other bytes needs an explicit erase.
3. **No data loss on upgrade.** Installed plugins, their KV, settings and sealed secrets
   keep working, unchanged, after migration `0015` (§4.4).
4. **Offline still works** (principle 1). Signing and the registry are optional: a site
   with no network installs unsigned or signed plugins from files as today, with a warning
   for unsigned ones. The registry is one HTTPS (or `file://`) URL, never required.
5. **Standard tools.** Publishers sign with the `minisign` CLI; the binary only verifies
   (no secret keys in `ferrite`). Tests use throwaway keys made in the test.
6. **Both profiles.** Every decision is a database read under the `extensions` lock; any
   node may install, check or revoke.

Non-goals (§12): a hosted submission service for third-party publishers, signed themes,
transparency logs, per-version timestamps, re-keying stored data.

## 2. Signatures

### 2.1 What is signed (D1–D3)

A signature covers a **payload** built from the two files' exact bytes:

```
ferrite-plugin-signature/1
manifest <sha256 hex of ferrite-plugin.toml, lowercase>
component <sha256 hex of the .wasm, lowercase>
```

Three lines, each ending in `\n` (LF), ASCII only, no trailing spaces. The signature is a
minisign signature over the payload's bytes, prehashed (minisign's default since 0.8; a
legacy signature is refused, as for releases: `crates/ferrite/src/update/verify.rs`). The
minisign trusted comment is not used (it is shown nowhere and decides nothing).

- **D1. The payload hashes the exact files, not a parsed form.** The manifest is stored as
  JSON (`plugins.manifest`), so a canonical re-serialisation would need a canonical TOML
  writer on the publisher side; hashing the uploaded bytes needs only `sha256sum`.
- **D2. A fixed text header (`ferrite-plugin-signature/1`).** Domain separation: a payload
  can never be read as a `SHA256SUMS` file or a registry index, so a key used for one thing
  cannot be tricked into vouching for another; `/1` versions the format.
- **D3. Only hashes, no name/version/publisher lines.** They are inside the manifest the
  payload already pins, and leaving them out avoids any escaping question for a
  `publisher` with odd characters.

The payload is printed by `ferrite plugin digest --dir DIR [--out FILE]` (no data folder)
and by `.github/scripts/plugin-payload.sh DIR` (bash + `sha256sum`/`shasum -a 256`, for CI
jobs without a binary); a test runs the script and compares it with
`ferrite_plugin_host::trust::signing_payload` byte for byte. A publisher signs with:

```sh
ferrite plugin digest --dir . --out /tmp/payload
minisign -S -s ~/.minisign/plugins.key -m /tmp/payload -x ferrite-plugin.minisig
cp ~/.minisign/plugins.pub ferrite-plugin.pub     # optional: lets a site trust the key on first use
```

### 2.2 Distribution of a signed plugin

A plugin folder (`--dir`, the release tarball's folders) holds `ferrite-plugin.toml`, one
`*.wasm`, and optionally `ferrite-plugin.minisig` (≤ 4 KiB, `MAX_SIGNATURE_BYTES`) and
`ferrite-plugin.pub` (a minisign public key file, ≤ 1 KiB). The admin's install dialog and
`POST /plugins/inspect|POST /plugins` take them as the optional multipart parts
`signature` and `publisher_key` (§7). A registry entry carries the signature inline and
the key in its publisher record (§5).

- **D4. The public key travels beside the plugin, not in the manifest.** A new manifest key
  would make every signed manifest fail on `0.1`/`0.2` binaries (unknown keys are errors,
  05 §3), and the key would vouch for itself anyway. A minisig names only its key id, so a
  key the site does not know yet must come from `ferrite-plugin.pub` or the registry.

### 2.3 Verification

`ferrite_plugin_host::trust` (new; minisign-verify moves from `crates/ferrite` to the
workspace and into `ferrite-plugin-host`):

- `PublisherKey::parse(text)`: a minisign public key file (`untrusted comment:` + base64
  line) or the bare base64 line; algorithm `Ed`; `key_id()` is the 8-byte key number as 16
  uppercase hex, little-endian, exactly as minisign prints it (`432051207B9BA065` for the
  release key). The pure parse (base64, length, key id) lives in `ferrite-domain`
  (`plugin_trust::PublisherKey`, so core and the store can use it without crypto); the
  crypto lives in `ferrite-plugin-host`.
- `verify_minisig(key, data, signature_text) -> Result<(), TrustError>`: prehashed only.
- `verify_bundle(manifest_bytes, wasm, minisig, candidates) -> Result<VerifiedSigner,
  TrustError>`: decodes the minisig, picks the candidate key whose id matches the
  signature's (no match: `UnknownKey(key_id)`), verifies the payload.
- `crates/ferrite/src/update/verify.rs` keeps `listed_sha256` and calls `PublisherKey::parse`
  and `verify_minisig` for the release check, so there is one minisign path (D5).

- **D5. One verifier for releases and plugins.** The roadmap says "minisign, as releases
  use"; sharing the code means one place to get the prehash rule and key-id handling right.

A signature that is present but does not verify, names an unknown key with no key given,
is legacy, or is malformed refuses the install (`422`, `errors[0].field = "signature"`).
It is never treated as unsigned (**D6**: otherwise a broken signature would downgrade to
the weaker path silently).

## 3. Publishers, keys and trust

### 3.1 Identities (D7)

A **publisher identity** is what plugin data is bound to:

| Identity | Written as | Where the key comes from |
|---|---|---|
| Registry publisher | `reg:<REGISTRY_KEY_ID>:<publisher id>` | the configured registry's verified index (§5) lists the key under that publisher |
| Key on first use | `key:<KEY_ID>` | a key the admin approved at install (TOFU), or one the operator added (`ferrite plugin keys trust`) |
| Unsigned | (no identity, `NULL`) | none |

- **D7. A registry publisher is an identity of its own, not a key.** Publishers rotate
  keys; the registry lists a publisher's current keys, so a new key listed there keeps the
  binding. The registry's own key id is part of the identity, so two registries with a
  publisher called `acme` are different publishers.

A signer's identities are `{key:<id>}`, plus `reg:…:<p>` when the cached index of the
configured registry lists that key under publisher `p` and does not revoke it.

For a registry publisher the manifest's `publisher` must equal the registry's publisher id
(`errors[0].field = "publisher"` otherwise), so the text the admin reads is the text the
registry vouches for. For a TOFU key it stays unverified text and is shown as "claims:
example.dev".

### 3.2 Signature policy (D8)

`[plugins] signatures` in `ferrite.toml`:

| Value | Unsigned | Signed, key new to the site | Signed, registry or operator key |
|---|---|---|---|
| `any` (default) | allowed, high-risk warning | allowed: approving trusts the key | allowed |
| `signed` | refused | allowed: approving trusts the key | allowed |
| `trusted` | refused | refused | allowed |

- **D8. The policy is operator configuration, not a site setting.** An admin can already
  install anything, so a policy an admin can switch off protects nothing; `trusted` is
  meant to hold against a stolen admin session. Cluster nodes must carry the same value
  (like `site.url`); `ferrite doctor` warns when installed plugins do not meet it (§9).
  The admin shows it read-only.

The policy applies to installs and updates only; a plugin installed before a stricter
policy keeps running and is flagged by doctor and the Plugins screen (D9: switching a
config value must not take a site's contact form down on restart).

### 3.3 Trust on first use

A signed bundle whose key id is unknown, with a `publisher_key` (folder `ferrite-plugin.pub`
or part `publisher_key`) that matches the signature, is reported as **new key**: "Signed by
key 5A1C3D… (claims: example.dev). This key is new to this site: approving trusts it for
updates of this plugin." Approving the report (its fingerprint covers the key id, §3.5)
records the key (`source = approved`) in the install transaction. Under `trusted` the
report is a refusal instead.

### 3.4 Revocation (D10)

A key can be revoked:

- **by the registry**: the index's `revoked_keys` (§5.1), applied at every successful
  registry check (§8);
- **by an admin**: `POST /api/v1/plugins/publishers/{key_id}/revoke` (`manage_plugins`,
  `extensions:write`, audited `plugin.publisher.revoke`) or `ferrite plugin keys revoke`.

A revocation has a `reason` and `compromised: bool`:

- Every revocation refuses new installs and updates signed by that key.
- `compromised = true` also **disables** every installed plugin whose current version is
  signed by that key, in the same transaction (`disabled_reason = "publisher key revoked:
  <reason>"`, `plugins.changed` per plugin, so every node unloads it). An admin can enable
  it again knowingly.
- **D10. Two levels, not one.** A retired or lost key is no reason to take a working site
  down; a stolen key is. Minisign signatures carry no trustworthy time (the trusted comment
  is signed by the same key), so "signed before the revocation" cannot be told apart.

Revocation never deletes data or the owner binding. An identity `key:<id>` whose key is
revoked can no longer be updated; the operator moves it with `ferrite plugin unbind`
(§4.3). Only the operator re-trusts a key an admin revoked (`ferrite plugin keys trust`); a
registry revocation cannot be overridden locally (use another registry).

### 3.5 The approval fingerprint (D11)

The fingerprint (05 §2) becomes the SHA-256 of the compact JSON `{"artifact", "grant",
"plugin", "version"}` plus `"signer": "<KEY_ID>"` when the bundle is signed.
`Manifest::permission_report_signed(artifact, signer, current, host)` computes it;
`permission_report` is that with `signer = None`.

- **D11. Add `signer` only when signed.** Unsigned fingerprints stay byte-identical (the
  pinned unit test value does not change, as with `request.served` in R23), and an approval
  of a signed report cannot be replayed with the signature stripped or swapped.

The owner change (§4) is not in the fingerprint: it is re-decided inside the install
transaction, which refuses if the state moved against the approval.

## 4. Publisher-bound plugin data

### 4.1 Binding, not re-keying (D12)

Plugin data stays keyed by the plugin name exactly as today: `plugin_kv(plugin, key)`,
options `plugin.<name>.settings.<key>`, sealed secrets under
`plugin-setting:<plugin>:<key>` (`ferrite_core::ext_settings::SECRET_PURPOSE`), the
deletion marker `plugin.<name>.deleted_at`, failure counters, routes `/_p/<name>/`. What is
new is the **owner** of the name: the table `plugin_owners` (§6) records which identity the
name's data belongs to.

- **D12. Bind the name instead of re-keying the data.** Re-keying would touch every KV
  row, every settings option, re-seal every secret under a new purpose and move routes, in
  a migration that must run on both backends without loss, for the same guarantee: a name
  whose data belongs to identity A can only be installed by A. The binding is one row,
  checked and written under the `extensions` lock in the install transaction. It also keeps
  the names stable for themes, `/_p/` links and exports.

### 4.2 The rule

Inside `write_install_plugin`'s transaction, under the `extensions` lock, core reads the
owner row of the name and the signer's identities (§3.1) and decides:

| Owner of the name | Upload | Result |
|---|---|---|
| none (new name, or data fully erased) | signed | **bind** to the signer's preferred identity (`reg:…` if any, else `key:…`) |
| none | unsigned | **bind unsigned** (owner row with `owner = NULL`) |
| unsigned | unsigned | same as today (report warns: "unsigned; it gets the data of the plugin installed under this name before") |
| unsigned | signed | **claim**: bind to the signer ("from now on only updates signed by FerriteCMS use this plugin's data"); this is how the v0.2 example plugins move to signed versions |
| identity X | signed, X ∈ signer's identities | **same** |
| identity X | signed by another identity, or unsigned | **refused** (`422`, `errors[0].field = "publisher"`): "the data of `seo` belongs to publisher FerriteCMS (registry). Delete the plugin and erase its data first, or install it under another name." |

The report shows the decision (`trust.owner`, §7.2) computed from the same function on a
read connection; the transaction runs it again, so an install racing another install or an
erase is decided on committed state.

A preferred identity is fixed at binding: a TOFU key that later appears in the registry
keeps `key:<id>` (it is still one of the signer's identities, so updates pass).

### 4.3 Moving or freeing a name

- **Erase** frees the name: the owner row is deleted in the same transaction that deletes
  the deletion marker (`delete_options` in `ExtensionService::erase_locked` without a kept
  marker): `POST /plugins/{name}/purge`, the 30-day purge job, the final sweep of a
  `purge=true` delete. While a `purge=true` delete's marker is still kept (up to
  `PLUGIN_PURGE_GRACE`, 2 minutes) the name stays bound, so a node's late KV writes are
  swept before another publisher can read them; the refusal then says "this plugin's data
  is being erased; try again in a few minutes".
- **`ferrite plugin unbind NAME`** (operator only, no API) deletes the owner row and keeps
  the data, so the next install binds again. It prints a warning and is the escape hatch
  for a publisher that lost its TOFU key.

### 4.4 Existing installs (migration, D13)

Migration `0015` creates the tables and writes no rows. A name without an owner row is
**unsigned-owned** (the unsigned row of the table above): installed plugins, retained data
and their secrets behave exactly as before, and the first signed install of each name
claims it (with the "claim" line in its report).

- **D13. No backfill.** Absence and an unsigned row mean the same thing, so the migration
  stays expand-only with nothing to convert (and nothing to get wrong on Postgres JSON vs
  SQLite JSON). Owner rows are written by installs from M6-2 on.

Known residual (not changed here): content meta under `plugin.<name>.*` belongs to the
content items and is not erased by a plugin purge (05 §2), so a new owner of a freed name
can read an earlier plugin's meta keys. Listed in §12.

## 5. The registry

### 5.1 Index format (schema 1)

One JSON file and its minisign signature (over the file's exact bytes, prehashed), at
`<url>` and `<url>.minisig`, at most 2 MiB (`MAX_INDEX_BYTES`). Types and validation in
`ferrite_domain::plugin_registry` (pure: serde + checks); the signature check in
`ferrite_plugin_host::trust::verify_index`.

```json
{
  "schema": 1,
  "name": "FerriteCMS plugins",
  "serial": 3,
  "generated_at": "2026-11-20T10:00:00Z",
  "publishers": [
    {
      "id": "ferritecms.dev",
      "name": "FerriteCMS (Softberries)",
      "homepage": "https://softberries.github.io/ferritecms/",
      "keys": ["RWQ…base64…"],
      "revoked_keys": [
        { "key": "RWQ…", "revoked_at": "2026-11-19T00:00:00Z", "reason": "rotated", "compromised": false }
      ]
    }
  ],
  "plugins": [
    {
      "name": "seo",
      "publisher": "ferritecms.dev",
      "description": "Meta tags, Open Graph and JSON-LD",
      "homepage": "https://softberries.github.io/ferritecms/guides/plugins/",
      "license": "AGPL-3.0-only",
      "versions": [
        {
          "version": "0.3.0",
          "world": "ferrite:plugin/plugin@0.1.0",
          "published_at": "2026-11-20T10:00:00Z",
          "manifest":  { "url": "https://github.com/softberries/ferritecms/releases/download/v0.3.0/ferrite-plugin-seo-0.3.0.toml", "sha256": "…", "bytes": 1890 },
          "component": { "url": "https://github.com/softberries/ferritecms/releases/download/v0.3.0/ferrite-plugin-seo-0.3.0.wasm", "sha256": "…", "bytes": 412331 },
          "signature": "untrusted comment: signature from minisign secret key\nRUQ…\ntrusted comment: …\n…\n",
          "yanked": null
        }
      ]
    }
  ]
}
```

Validation (`RegistryIndex::parse`, every rule a unit test with a hostile index):

- `schema` must be `1` (another value: "this binary reads registry schema 1"); unknown
  members are ignored (forward-compatible within schema 1, D14).
- `serial` > 0. Publisher ids `[a-z0-9][a-z0-9.-]{1,63}`, unique; every key parses as a
  minisign key; a key id appears under at most one publisher (keys and revoked keys).
- Plugin names follow `ferrite_domain::valid_plugin_name` and are unique; each names an
  existing publisher; versions are semver and unique per plugin; `world` is a string.
- File entries: `url` at most 2048 bytes, an absolute `https://` URL or a relative path
  (no scheme, no `..` segment, no leading `/`, no backslash); `sha256` 64 lowercase hex;
  `bytes` ≤ 64 KiB (`MAX_MANIFEST_BYTES`) for manifests and ≤ 32 MiB (`MAX_PLUGIN_BYTES`)
  for components. `signature` at most 4 KiB. Text fields capped (description 500,
  name 100, reason 500 chars) and shown as text.
- At most 1 000 plugins and 100 versions each.

- **D14. A static file, a monotonic serial, no expiry.** It can live on GitHub Pages or any
  static host and be mirrored by copying two files. The site keeps the highest serial it
  accepted and refuses a lower one, or the same serial with different bytes (rollback). An
  expiry date would force a re-signing job with the key on a schedule; freeze attacks
  (serving an old but valid index) are accepted (§12).
- **D15. Signatures inline, files by URL.** The publisher signature rides in the index (one
  fetch fewer, and the same `verify_bundle` path as uploads); the files are fetched only on
  install. Each file's `sha256` is checked against the index before the signature, so a
  swapped file fails fast; the publisher signature is still required, so changing an entry's
  hashes alone does not ship code. (A registry key holder can list a new key for one of its
  publishers: that is the registry's role, §12.)

### 5.2 Which registry

`ferrite.toml`:

```toml
[plugins]
registry = "https://softberries.github.io/ferritecms/registry/v1/index.json"  # default; "" = off
registry_key = "RWQ…"   # required for any other registry; default: the embedded key
signatures = "any"      # any | signed | trusted (§3.2)
registry_check = "off"  # off (default) | daily (ruling P5, D18)
```

- The default URL uses the key embedded from `crates/ferrite/registry.pub` (like
  `release.pub`). Until the user creates that key the file is a comment-only placeholder:
  a binary built from it has the default registry off (`config check` and doctor say so),
  and any registry works with an explicit `registry_key`, which is what tests do with a
  throwaway key (D16).
- `file:///abs/path/index.json` is accepted for air-gapped sites and tests: relative file
  URLs resolve inside the index's folder and may not leave it (canonicalised, then a
  prefix check); absolute URLs are refused in a file registry.
- `https://` registries: relative URLs resolve against the index URL; absolute ones must
  be `https`.

- **D16. The registry key is configuration with an embedded default.** Tests and private
  registries pass a key in config; no test seam, no special build. A separate key from the
  release key: the index key is used on every release and by Pages tooling, the release key
  only for binaries.

### 5.3 Fetching (SSRF rules)

`crates/ferrite-http/src/registry_fetch.rs`: `RegistryFetcher` (HTTPS) and a file reader
behind one trait `RegistrySource { async fn get(&self, url: &Url, max: u64) ->
Result<Bytes, RegistryFetchError> }`. The HTTPS client is a new `egress::registry_client()`
built like `metadata_client()`: the checked resolver with `is_public_ip` (the whole answer
refused when any address is not public), the connected peer checked again, no proxy, no
cookies, no `Referer`, no `Authorization`, `https` only, the platform roots with the
`webpki-roots` fallback. Redirects followed by the fetcher (301/302/303/307/308, at most
`MAX_REDIRECTS` = 3, `https` only, credentials refused, IP literals through the address
rule; GitHub release downloads redirect once to GitHub's asset host). One deadline per
file: 30 s for the index and its signature, 60 s for a component. `Content-Length` above
`max` is refused before the body; the body read stops past `max`.
`User-Agent: ferrite/<version> registry`. Test seam (`test-seams`): `for_tests(
egress::EgressOverrides)`, as `OAuthFetcher::for_tests`.

### 5.4 The cache

The last verified index of each registry URL is kept in `plugin_registry` (§6): its bytes,
signature, serial, `generated_at`, `fetched_at`, plus `checked_at` and the last error. All
reads (search, update checks, identities of §3.1) use the cached copy; only a refresh
fetches (§8). A refresh that fails keeps the previous copy and records the error.

## 6. Storage (migration `0015_plugin_publishers`)

Expand-only, both backends (`-- ferrite:phase expand`, `.down.sql` drops the three
tables), contract tests on both. Timestamps as in the other tables (SQLite INTEGER unix
micros, Postgres `timestamptz`); Postgres text keys `COLLATE "C"` (R8).

- `plugin_publishers(key_id TEXT PK, public_key TEXT NOT NULL UNIQUE, label TEXT NOT NULL,
  source TEXT NOT NULL CHECK (source IN ('approved','operator','registry')), trusted_at,
  revoked_at NULL, revoked_reason TEXT NULL, compromised BOOL NOT NULL DEFAULT false)` —
  keys this site trusts on its own (approved at install, added by the operator), and every
  revocation (any source; a registry key appears here only once revoked).
- `plugin_owners(plugin TEXT PK, owner TEXT NULL, signer_key TEXT NULL, signature TEXT
  NULL, publisher TEXT NOT NULL, source TEXT NOT NULL CHECK (source IN
  ('upload','folder','registry')), registry_url TEXT NULL, bound_at, updated_at)` — the
  identity the name's data belongs to (`owner`, `NULL` = unsigned) and how its current
  version arrived: the signing key and minisig text of the installed version (`NULL` when
  unsigned), the manifest's publisher text. No foreign key to `plugins`: it outlives the row
  while data is retained. Indexed on `signer_key` (revocation).
- `plugin_registry(url TEXT PK, serial BIGINT NOT NULL, index_json TEXT NOT NULL,
  signature TEXT NOT NULL, generated_at, fetched_at, checked_at, last_error TEXT NULL,
  last_error_at NULL)`.

Store: one new repo on `Repos`, `plugin_trust() -> &dyn PluginTrustRepo`:
`get_key`, `list_keys`, `upsert_key`, `revoke_key(key_id, at, reason, compromised)`,
`get_owner`, `list_owners`, `put_owner`, `delete_owner`, `owners_signed_by(key_id)`,
`get_registry(url)`, `put_registry(&RegistryCacheRow)`, `registry_checked(url, at,
error: Option<&str>)`. Implemented on both backends and on `CountingStore` /
`FailingOptionsStore` (`ferrite-store-tests`).

Export catalogue (`crates/ferrite-store/src/catalogue.rs`): `plugin_publishers` and
`plugin_owners` are exported rows (a moved or restored site keeps its bindings and
revocations); `plugin_registry` is `Export::Skip` (a cache, refetched).

Domain types (`ferrite-domain/src/plugin_trust.rs`): `PublisherKey`, `KeyId`,
`KeySource`, `TrustedKey`, `OwnerId` (parse/format of `reg:` and `key:`), `PluginOwner`,
`InstallSource`, `SignaturePolicy`, `RegistryCacheRow`. `InstalledPlugin` is unchanged
(D17: a separate table keeps the ~18 places that build `InstalledPlugin` untouched).

## 7. API

All under `/api/v1`, `manage_plugins`; reads `extensions:read`, writes
`extensions:write`. Capabilities checked in core (`ExtensionAdmin`).

### 7.1 Changed endpoints

- `POST /plugins/inspect` and `POST /plugins`: optional parts `signature` (≤ 4 KiB, UTF-8
  minisig text) and `publisher_key` (≤ 1 KiB). `422` with `errors[0].field` =
  `signature` (bad, unknown key, revoked, policy) or `publisher` (owner mismatch, registry
  publisher text mismatch); the body's `report` as for an approval mismatch when a report
  could be built.
- `PermissionReportDto` gains `trust: TrustDto` (§7.2). `PluginDto` (list, get, install)
  gains `trust: InstalledTrustDto { state: "verified"|"signed"|"unsigned"|"revoked",
  owner: OwnerDto | null, signer: SignerDto | null, update: UpdateDto | null }` with
  `UpdateDto { version, publisher_matches: bool }` from the cached index (§8).
- Audit `plugin.install` details gain `signer` (key id or `null`), `owner` and `source`.

### 7.2 `TrustDto`

```json
{
  "policy": "any",
  "signature": "verified",            // "verified" | "unsigned"
  "signer": {
    "key_id": "432051207B9BA065",
    "source": "registry",             // "registry" | "approved" | "operator" | "new"
    "publisher": { "id": "ferritecms.dev", "name": "FerriteCMS (Softberries)" },
    "claimed": "ferritecms.dev"       // the manifest's text
  },
  "owner": { "change": "claim", "current": null, "next": "reg:9F1E…:ferritecms.dev" },
  "source": { "kind": "registry", "url": "https://…/index.json", "version": "0.3.0" },
  "warnings": ["unsigned_data_claimed"]
}
```

`owner.change` is `bind`, `bind_unsigned`, `same`, `claim` or `unsigned_update`. Warning
codes (the admin and CLI word them): `unsigned`, `new_key`, `unsigned_data_claimed`,
`unsigned_reads_data` (an unsigned update or reinstall over existing data).

### 7.3 New endpoints

| Method and path | What |
|---|---|
| `GET /plugins/registry?q=` | `RegistryDto { registry: { url, name, serial, generated_at, checked_at, last_error } \| null, plugins: [RegistryPluginDto { name, publisher {id,name}, description, homepage, license, latest, versions, installed: { version, update_available, publisher_matches } \| null }] }` from the cache; `q` matches name, description and publisher name (case-insensitive, at most 50 results); `registry: null` when none is configured |
| `POST /plugins/registry/refresh` | fetches and verifies now (§8), then as `GET`; `502 registry-unavailable` (the fetch failed; the cache stays), `422 registry-invalid` (signature, schema, serial rollback); at most one refresh per 60 s per site (`429` otherwise; the `rate_limits` table) |
| `POST /plugins/registry/inspect` `{name, version?}` | fetches the files, checks sizes and hashes against the cached index, verifies the signature, inspects: `PermissionReportDto` with `trust.source.kind = "registry"` |
| `POST /plugins/registry/install` `{name, version, approve}` | the same again, then installs (`201 PluginDto`); `422 approve` as for uploads (a file that changed between the two calls changes the fingerprint) |
| `GET /plugins/publishers` | `[PublisherKeyDto { key_id, public_key, label, source, registry_publisher, trusted_at, revoked_at, revoked_reason, compromised, plugins: [name] }]`: the site's keys, the cached registry's keys, the revocations |
| `POST /plugins/publishers/{key_id}/revoke` `{reason, compromised}` | §3.4; `200 PublisherKeyDto`; audited |

`registry` and `publishers` join `RESERVED_PLUGIN_NAMES` (ruling R9: static path segments
under `/plugins/`), checked for uploads only so a stored row of that name still loads.
OpenAPI: DTOs with `utoipa`; `crates/ferrite-http/openapi.json` regenerated.

There is deliberately no API to add a key or to unbind a name (operator only, D8).

### 7.4 MCP

No new tools: installing and deleting extensions stay human (12 §5.4,
`no_tool_installs_or_deletes_extensions_or_users`). `plugins_list` returns `PluginList`, so
it carries `trust` (verified publisher, update available) without a code change in the
tool; the MCP reference page notes it.

## 8. Update checks

- **Job `plugins.registry_check`** (daily periodic only with `registry_check = "daily"`,
  ruling P5; not in `core_periodics()`), handler in
  `crates/ferrite/src/jobs/registry.rs`: when a registry is configured, fetch the index and
  signature, verify, check the serial, store the cache row, then **apply revocations**
  (`ExtensionService::apply_registry_revocations`: insert missing revocation rows, disable
  plugins signed by newly compromised keys, one transaction under the `extensions` lock),
  then record `checked_at`. A failed fetch records `last_error` and is not retried before
  the next day (`JobError::Permanent` for verification failures, retry for network errors
  within `max_attempts`). In a cluster the queue runs it once.
- **On demand**: `POST /plugins/registry/refresh`, the Plugins screen's "Check for
  updates", and every CLI `search|install NAME|update` (operator commands always fetch).
- **Update available** for an installed plugin: the cached index has the same name, a
  non-yanked version with a supported world (`ferrite_plugin_host::WORLD`, the private `manifest::WORLD` made public) newer than the
  installed one, and the index publisher's identity matches the owner (or the owner is
  unsigned: then `publisher_matches = false` and the screen says "the registry's `seo` is
  published by FerriteCMS; updating binds this plugin's data to it").
- Nothing installs by itself. An update is an install with its own report and approval.

- **D18 (changed by ruling P5). The daily check is off by default, nothing is automatic.**
  A site makes no outbound request it was not configured for (as with `self-update
  --check`): `[plugins] registry_check = "daily"` schedules the job (04:00 UTC, after the
  artifacts GC); otherwise the index is fetched only on demand: the Plugins screen's "Check
  for updates" (`POST /plugins/registry/refresh`) and every CLI `search|install NAME|update`.
  Revocations therefore arrive at the next check, scheduled or on demand. Automatic
  installs would skip the permission report (P3). `registry = ""` turns the registry off.

## 9. CLI and doctor

`ferrite plugin …` (operator path, P26; SQLite: `serve` must be stopped except `digest`):

| Command | What |
|---|---|
| `check --dir DIR` / `install --dir DIR` | read `ferrite-plugin.minisig` and `ferrite-plugin.pub` when present; the report starts with the trust lines (`publisher: verified ferritecms.dev (registry key 432051…)`, `! unsigned: …`, `! new publisher key 5A1C… (claims example.dev): approving trusts it`) |
| `install NAME[@VERSION]` | from the configured registry (fresh fetch); same report and `install? [y/N]` / `--yes`; `--enable` |
| `search [QUERY]` | `<name>  <latest>  <publisher>  <description>` (fresh fetch); exit 1 when no registry or the fetch fails |
| `update [NAME…] [--check] [--yes]` | every installed plugin with an update (§8); `--check` prints `<name>  <installed> -> <available>` and exits 2 when there are updates, 0 when none (as `self-update --check`) |
| `digest --dir DIR [--out FILE]` | the signing payload (§2.1); no data folder |
| `keys list` | `<key_id>  <source>  <label>  [revoked: <reason>]` |
| `keys trust FILE --label TEXT` | adds a key (`operator`), or clears a local revocation |
| `keys revoke KEY_ID --reason TEXT [--compromised]` | §3.4 |
| `unbind NAME` | §4.3 |

Exit codes: 0 done, 1 error (refusals included), 2 only for `update --check`. `install`
with both `NAME` and `--dir` is a usage error.

`ferrite doctor` gains one check, `plugins.trust` (after `db.identity`; database only, no
network; works with `--offline`): `warn` when an installed plugin does not meet
`plugins.signatures`, when an installed plugin is signed by a revoked key, or when the
configured registry's last check failed, or (with `registry_check = "daily"`, ruling P5) its
last successful check is older than 3 days or never happened; `info` "registry off" when none
is configured (or this build has no key for the default registry); else `ok` ("3 plugins, 3
verified; registry serial 3 checked 5 h ago"). Doctor's check count becomes 26.

## 10. Admin

Design rules of 11 (tokens only, 44 px targets, axe clean, every manifest and index string
as text). Files under `admin/src/plugins/`; strings in `admin/src/i18n/en/admin.json`
(`plugins.trust.*`, `plugins.registry.*`, `plugins.publishers.*`).

- **Permission report** (`permission-report.tsx` + new `trust-summary.tsx`): a Publisher
  block first. Verified: a check icon, "Verified publisher", the registry publisher's name,
  the key id in mono. New key: warning style, "New publisher key", the claim, and "Approving
  trusts this key for this plugin's updates". Unsigned: high-risk style, "Unsigned plugin:
  nothing confirms who made it". Owner lines: claim ("This plugin's data will be bound to
  FerriteCMS"), bind, same (no line). The policy is named when it shaped the result.
- **Install dialog** (`install-flow.tsx`): two optional inputs after the manifest and the
  component, "Signature (`ferrite-plugin.minisig`)" and "Publisher key
  (`ferrite-plugin.pub`)"; dropping a folder's files picks each by name. A `422` with
  `field = "signature"|"publisher"` shows the server's message in the report step.
- **Plugins list** (`plugins-list.tsx`): the publisher cell shows "FerriteCMS ✓ Verified",
  "example.dev · signed" or "example.dev · Unsigned" (text, not colour only); an "Update
  0.3.0" chip that opens the update flow; a "Check for updates" button with the registry's
  last check time; tabs **Installed | Registry**.
- **Registry tab** (new `registry-browser.tsx`, `/admin/plugins?tab=registry`): search
  box, results with name, publisher (verified), latest version, description, Install (the
  same report step, fed by `POST /plugins/registry/inspect`, then `…/install`), "Installed"
  or "Update" for installed names; an empty state when no registry is configured ("Set
  `[plugins] registry` in ferrite.toml") or the last check failed.
- **Publishers page** (new `publishers.tsx`, route `/admin/plugins/publishers`, linked from
  the list's menu): the keys with source, label, plugins and revocation state; "Revoke"
  opens a dialog with a reason and "The key was stolen or leaked (disables its plugins)";
  the policy shown read-only with the config key that sets it.
- **Plugin page** (`plugin-detail.tsx`): a Publisher section (identity, signer key, how it
  arrived: upload, folder or registry with URL, the claim), and the Update button when one
  is available.

Vitest for each component; Playwright E2E `admin/e2e/plugins-trust.spec.ts` against a
`file://` registry made in global setup with throwaway keys (`e2e/support/minisign.ts`,
Ed25519 and BLAKE2b-512 from `node:crypto`, an independent implementation of the format,
D19), in Chromium on both backends; the axe check of the new screens.

- **D19. E2E signs with Node's crypto.** The release binary under test has no signing
  code; a second implementation of minisign's format in the tests also catches a verifier
  that only agrees with itself.

## 11. Publishing the official plugins (CI)

Keys (made once by the user, like the release key; secrets of the `release` environment).
Ruling P5: until they exist, `crates/ferrite/registry.pub` is a comment-only placeholder,
`registry/publishers.json` lists no key, and `plugins-sign` packs the plugins unsigned and
publishes no index (a notice says so); every test uses throwaway keys. As built, the
signing steps are `.github/scripts/sign-plugins.sh` (payload, `minisign -S`, the tarball and
per-plugin assets through `package-plugins.sh` with `SIGNATURES_DIR` and `ASSETS=1`, the
index through `registry-index.py`, then every signature verified with
`.github/scripts/verify-file.sh`), shared by `plugins-sign`, `plugins-sign-dry-run` and the CI
`packaging` job:

- `PLUGINS_SECRET_KEY` / `PLUGINS_PASSWORD`: the publisher key of `ferritecms.dev`, which
  signs the three example plugins. Its public half is `registry/publishers.json`'s key.
- `REGISTRY_SECRET_KEY` / `REGISTRY_PASSWORD`: the index key. Its public half is
  `crates/ferrite/registry.pub` (embedded).

Repository files: `registry/publishers.json` (the publishers and their keys and
revocations: the registry maintainer's source of truth, reviewed in PRs),
`registry/yanked.json` (`[{name, version, reason}]`, applied to every version listed),
`.github/scripts/plugin-payload.sh`, `.github/scripts/registry-index.py` (Python 3
standard library; merges the previous index with this release's entries, bumps the serial,
validates the result against the same rules, writes `index.json`).

`release.yml`:

1. `plugins` (unchanged inputs) builds the wasm and stages the three folders as the
   artifact `plugins-stage` (instead of packing the tarball).
2. **`plugins-sign`** (environment `release`, publish runs only): for each folder, the
   payload, `minisign -S` with the plugins key → `ferrite-plugin.minisig`, plus
   `ferrite-plugin.pub`; `package-plugins.sh` packs the tarball from the signed folders and
   writes the per-plugin assets `ferrite-plugin-<name>-<version>.{toml,wasm}`; it downloads
   the published index (`https://softberries.github.io/ferritecms/registry/v1/index.json`
   and `.minisig`), **verifies it with `registry.pub`** (a tampered Pages copy is never
   re-signed; a 404 starts at serial 1 only when no earlier release carries the asset
   `ferrite-registry-index.json`, checked with `gh release list`), runs `registry-index.py`, signs
   `ferrite-registry-index.json` with the registry key, verifies the result with
   `registry.pub` and the plugin signatures with the publisher key, and uploads
   `dist-plugins`. **`plugins-sign-dry-run`** does the same with two throwaway keys (their
   `.pub` files go into the artifact), as `sign-dry-run` does for the release key.
3. `sign` / `sign-dry-run` need `plugins-sign*`; `SHA256SUMS` lists the new assets too.
4. `release` uploads them; a new last step runs `gh workflow run pages.yml --ref main`
   (`actions: write`; a `workflow_dispatch` made with `GITHUB_TOKEN` starts a run, other
   events would not).

`pages.yml`: after the export, download `ferrite-registry-index.json` and its `.minisig`
from the latest release (`releases/latest/download/…`), verify them with
`crates/ferrite/registry.pub`, and place them at `registry/v1/index.json` and
`registry/v1/index.json.minisig` in the Pages artifact; a release without the files (v0.2.0
and older) skips the step with a notice. The default registry URL is therefore
`https://softberries.github.io/ferritecms/registry/v1/index.json`.

The CI `packaging` job runs `package-plugins.sh` and `registry-index.py` against fixture
input with throwaway keys made in the job, and `cargo test` includes a test that parses the
script's output with `RegistryIndex::parse` and verifies its signatures (one format, two
writers checked against one reader).

The example plugins' version stays the workspace version (09 §7); the registry keeps
every released version (yanking is an entry in `registry/yanked.json`, applied by the
script on the next release).

## 12. Security notes

- **Threats covered.** A swapped component or manifest (hash in the signed payload); a
  stripped or replaced signature on an update (owner binding + `signer` in the
  fingerprint); a same-named plugin from another publisher reading data or secrets (owner
  binding); a tampered or rolled-back index (index signature + serial); an index file that
  does not match the published bytes (sha256 before signature); SSRF through registry URLs
  (checked resolver, `https` only, public addresses, bounded redirects and sizes; file
  registries confined to their folder); a stolen key (revocation, disable when compromised).
- **Trusted parties.** The registry key holder can list a new key for any of its
  publishers (that is what makes key rotation work); the operator (config, `keys trust`,
  `unbind`, database access). An admin can approve a new TOFU key under `any`/`signed`,
  which is why `trusted` exists.
- **Not covered.** Freeze attacks (an old valid index served forever: updates and
  revocations stop arriving; doctor warns after 3 days without a successful check); a
  malicious publisher (signing says who, not whether it is safe: the permission report and
  the sandbox still do that); content meta `plugin.<name>.*` surviving an erase (§4.4);
  themes (they run no code, principle 3; signing bundles is later).
- Untrusted strings (manifest, index, minisig comments) are rendered as text everywhere;
  the index's text fields are length-capped; nothing from the index is used in a path
  except relative file URLs, which are confined.

## 13. Cluster and profiles

- **Solo**: the tables live in `ferrite.db`; CLI commands need `serve` stopped (SQLite
  lock), the job runs in `serve`.
- **Cluster**: decisions and writes run under the `extensions` lock (cluster-wide on
  Postgres) in one transaction; installs and revocation-disables emit `plugins.changed`, so
  every node reloads (05 §2) and flushes its page cache after the reload, as today. The
  registry cache is in the database, so every node shows the same updates; one node runs
  the daily job. `[plugins] signatures` and `registry*` must match across nodes (documented
  beside `site.url` in 09 §7 and the cluster example).
- No new events, no change to the page cache keys: trust does not change rendered pages.

## 14. Later

A submission flow for third-party publishers (PRs adding `registry/plugins/*.json` with
their own hosted files), a transparency log of index versions, signed theme bundles,
erasing `plugin.<name>.*` content meta with the plugin's data, publisher-announced key
rotation for TOFU keys, an index expiry with a scheduled re-sign, a `ferrite plugin sign`
convenience command (needs secret-key handling in the binary).

## Decisions

D1 payload hashes exact bytes · D2 text header for domain separation · D3 hashes only ·
D4 key beside the plugin, not in the manifest · D5 one minisign verifier for releases and
plugins · D6 a bad signature refuses, never downgrades · D7 registry publisher as an
identity · D8 policy is operator config · D9 policy applies to installs only · D10
revocation with `compromised` · D11 `signer` in the fingerprint only when signed · D12 bind
names, do not re-key data · D13 no backfill · D14 static index, monotonic serial, no expiry
· D15 inline signatures, files by URL · D16 registry key in config with an embedded default
· D17 trust in its own tables, `InstalledPlugin` unchanged · D18 daily check, nothing
automatic · D19 E2E signs with Node's crypto.
