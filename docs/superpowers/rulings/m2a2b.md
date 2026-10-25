# M2a-2b media, comments, system API and hardening — plan and controller rulings

Decisions made while planning and executing docs/superpowers/plans/2026-10-24-m2a2b-media.md (subagent-driven, 16 tasks). Format: ruling — reason — cost if wrong. P = plan rulings, F = pre-flight findings (all accepted as proposed by R3; one line each), R = controller rulings during execution (R1 and R2 are product rulings made before the pre-flight scan). Where the code differs from a plan ruling's text, the line says "as built".

## Plan rulings

- Plan ruling P1: one plan of 16 tasks, no split; the store work rides with the tasks that use it (only `0006`'s tables and repositories come first) — the ROADMAP list fits — cost: Tasks 7 and 12 are the largest.
- Plan ruling P2: migration `0006_media_comments` (both backends, `.down.sql`, expand, `lossy-down`): `idempotency_keys`, `media.sanitized`, comment and job indexes; no CHECK on `comments.status` (SQLite would need a table rebuild), core validates — cost: none. As built it also adds `comments.by_account` (R13) and `jobs_created_idx`/`jobs_kind_created_idx` (F26, R17).
- Plan ruling P3: bytes are staged in `ferrite-http` (`media::Uploader`, outside any `Tx`), rows recorded in core (`MediaService::record_upload`: artifact row, item, media row, event and variants job in one `Tx`); a refused or failed upload never deletes a blob — content addressing means another row may own it — cost: orphan blobs until M4's GC.
- Plan ruling P4: the type is decided from the first 8 KiB before anything is stored, by content only — cost: none. As built (F13) the file name picks within the ZIP and text classes.
- Plan ruling P5: uploads are published media items at once (WordPress attachments are public on upload); every role with `upload_media` also holds `publish_own` — cost: an upload is public by hash before the post using it is published.
- Plan ruling P6: accepted types were `inline_safe` plus sanitised SVG, documents refused (`415`) — cost: no document uploads. **Superseded by R1:** documents are accepted and served download-only.
- Plan ruling P7: SVG sanitising is our own allowlist over quick-xml 0.41 (no maintained sanitiser crate), stored instead of the original and marked `media.sanitized`; only marked SVG is served inline, under `sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'` — cost: `<style>`, animation, links and external fonts are stripped. As built tightened by R5 and R6.
- Plan ruling P8: variants are one `media.variants` job per raster upload (unique per item and artifact), bounded per node by `media.image_workers` (default 1), skipped above 64 MiB, sizes = core + the active theme's on the node that runs the job, recording conditional on the row still holding that artifact — cost: a theme switch leaves older uploads without its new sizes (regeneration is M4).
- Plan ruling P9: `media.max_upload` defaults to 256 MiB, capped at 1 GiB (below S3's 5 GB single-copy ceiling); at most 8 uploads in flight per node — cost: larger media need a multipart-copy path later. As built uploads idle for `media.upload_idle_timeout` (default 30 s) are aborted (Task 4 review).
- Plan ruling P10: `Idempotency-Key` on `POST /media` only, scoped to (user, key), stored as SHA-256; finished replays without reading the body, pending `409`, stale after 1 h, method/path mismatch `422`, failures release the key, rows pruned after 24 h by `sessions.prune` — cost: a reused key with another file gets the first answer. As built with an ownership token (F5) and stored `Location`/`ETag` (F6).
- Plan ruling P11: comments are plain text on `post` items only, rendered by core (escaped, paragraphs, no autolinks), statuses `pending|approved|spam|trash`, replies to approved comments up to depth 5, `comments.enabled` default on, moderators' own comments approved — cost: no anti-spam hook in v1. As built (R2, F19–F21) `comments.moderation` defaults to `known`, not `all`; under `known` anyone who knows a regular guest's exact name and e-mail posts unmoderated as them (WordPress's behaviour), which `all` closes.
- Plan ruling P12: two submit paths, one service: `POST /api/v1/content/{type}/{id}/comments` (JSON; an optional session with CSRF or a token makes it the user's) and the theme form `POST /_comment` (form-encoded, same-origin, always anonymous); success redirects use fragments so cached pages stay shared — cost: logged-in users type their name in the theme form.
- Plan ruling P13: spam defences: per-IP 3/min and 20/h DB-backed limits, a honeypot that silently discards, moderation by default, the link-count rule, keyed IP hashes (`ferrite:comment-ip:v1`); an unknown client address is `503` — cost: a shared NAT address shares the limit. As built (R2) "moderation by default" is `known`: first comments are held.
- Plan ruling P14: comment moderation is not audited (volume; the row carries its status); job retries are (`job.retry`) — cost: no history of who approved a comment.
- Plan ruling P15: system reads need the new `manage_system` capability (admin) with `system:read`, retry `system:write`; no new lock — cost: `Capability::ALL` grows to 16.
- Plan ruling P16: the `http` import refuses a DNS answer when any address is non-public and connects only to the checked addresses (no second lookup, no rebinding window); `https` on 443 only; redirects re-checked per hop; no proxy, cookies, referer or decompression; 5 s, 1 MiB out, 2 MiB in — cost: a host with one private address is unreachable; no plain-HTTP or custom-port APIs.
- Plan ruling P17: reqwest 0.13.5 with rustls (already in the tree through object_store); TLS roots from the platform verifier — cost: the binary depends on the host's CA store. As built (R19) with a webpki-roots fallback, and boot never fails on egress.
- Plan ruling P18: node diagnostics are scrubbed at the source and again on read (`diagnostics::scrub`: URLs, paths, addresses); full errors stay in the node log — cost: operators read the log for exact paths. As built widened by R15/R16 (credentials, AWS ids, buckets, `host:port`).
- Plan ruling P19: the jobs list never returns payloads; `handled` comes from `nodes.meta.job_kinds`; retry is dead → queued with attempts reset and `last_error` kept — cost: payload debugging needs the database.
- Plan ruling P20: metrics are a node-local snapshot (plugin call stats per hook, cluster-wide failure buckets for 24 h, renders, page-cache stats, DB job counts); the registry, `/metrics`, `node_metrics` and time series stay M4 — cost: the M2b dashboard shows only the answering node until M4.
- Plan ruling P21: one HTML URL policy (`ferrite_domain::html`) for content blocks and plugin `html`: `http`, `https`, `mailto`, root-relative, fragment, query and path-relative; `//…`, backslashes, `ftp:`, `tel:`, `data:`, `javascript:` dropped — cost: `tel:` links need a later setting.
- Plan ruling P22: R24 N1/N2: head script sources resolved with `Url::join` against `site.url` before the `/_p/`/`/_media/` refusal; a slot-wait-shortened call counts as a failure once the plugin ran at least half its deadline — cost: a slow-but-legitimate plugin under load accumulates failures sooner. As built per R20.
- Plan ruling P23: `__Host-ferrite_session` on `https` sites, no transition read — cost: one re-login after upgrading a pre-release site.
- Plan ruling P24: zeroization of `SiteKey`, subkeys, HMAC state, `Secret` and API password inputs; serde_json's request buffer and argon2's internals are not wiped (documented, 07 §2) — cost: residual copies in freed buffers.
- Plan ruling P25: term archives list every type of the taxonomy (`ContentQuery.kinds`); theme `query(term=…)` reads exactly `limit` items; term slug changes record 301s — cost: one more store filter on both backends.
- Plan ruling P26: deferred to M3: redirects for `permalink.*` structure changes and `Idempotency-Key` for imports — cost: changing the permalink structure breaks old links until M3.
- Plan ruling P27: deferred to M4: `date` filter time zones, `seed demo` atomicity, variant regeneration (`ferrite media regenerate`), the metrics registry and time series — cost: template dates stay UTC; a failed seed leaves a partial demo.
- Plan ruling P28: new test plugins `fetcher` and hostile `ssrf`; `html-inject` gains a footer hook — cost: longer plugin builds.
- Plan ruling P29: later (after M2): comments on pages and custom types, per-item closing, comment action hooks and admin notification mails, extra upload types — cost: listed in the roadmap. As built (R1) documents are no longer "extra"; further types (`media.extra_types`) stay later.
- Plan ruling P30: the sweep's unauthenticated list grows by the comment submit only, which checks the origin like login unless a bearer token is sent — cost: none.

## Pre-flight findings (accepted by R3)

- F1: incomplete Files lists (test-module registrations, other `impl Repos` wrappers, `FaultyJobs`, `HookRedirects`, migration-version pins, every `ApiParts` site) are in scope of the owning task.
- F2: dependencies `url` (plugin-host, http), `semver` (core normal dep) and tokio `net` (http) are added by the task that first uses them; no new crate.
- F3: `cargo test` takes one positional filter; several filters go after `--`.
- F4: `idempotency_keys.response` is `jsonb` / `TEXT CHECK (json_valid(...))`; the comment cursor tag is `cm:created`.
- F5: idempotency takeover uses an ownership token (`created_at` on the database clock); `finish`/`release` act only on that token.
- F6: the stored response is `{body, headers: {location, etag}}`; a replay restores both plus `Idempotent-Replayed: true`.
- F7: `IDEMPOTENCY_RETENTION` (24 h) and `IDEMPOTENCY_STALE` (1 h) live in `ferrite-domain`.
- F8: `SvgError` gains `TooManyAttributes` and `ValueTooLong` (refuse, not drop).
- F9: the SVG writer emits a canonical document (unprefixed names, the root declares the SVG namespace, `xlink` only when kept).
- F10: `publish_in` takes the pinned permalink rules; `record_upload` pins once.
- F11: the all-or-nothing upload test fails `jobs().enqueue` inside the `Tx` through a test-local store wrapper.
- F12: the media route limit is the config ceiling (1 GiB + 1 MiB); the handler enforces `max_upload + 1 MiB` and streams `file` with `put_stream(.., max_upload)`.
- F13: R1's type table: `inline_safe` ∪ sanitised SVG ∪ ZIP documents by extension ∪ UTF-8 `.txt`/`.csv` not led by `<`; everything else `415`; one HTTP test per class.
- F14: PDF keeps inline + `sandbox`; the classes R1 adds are download-only.
- F15: in Direct S3 mode the download-only rule and the SVG CSP hold only for app-served media (documented in 03 §5).
- F16: `last_error` in the jobs API goes through `diagnostics::scrub`.
- F17: the image slot is `try_acquire_owned` with the permit moved into the blocking closure; none free is `JobError::Busy`.
- F18: a discarded (honeypot) submit answers exactly like a held one (`202`, a fresh never-stored id).
- F19: `known` matches a guest by e-mail and trimmed name on an approved guest comment, never an account's e-mail; a signed-in commenter by `user_id`.
- F20: `comments.moderation` defaults to `known`; `comments.enabled` stays default `true`.
- F21: comment defaults are core constants (`COMMENTS_ENABLED_DEFAULT`, `COMMENTS_MODERATION_DEFAULT`); `GET /settings` shows `null` until set; `SettingsDto` lists both keys.
- F22: comment delete needs `moderate_comments`; a cookie that does not resolve is anonymous, a resolved one needs CSRF; a bearer submit needs `comments:write`.
- F23: `/_comment` reaches `CommentService` through `AppState::with_comments`; `_comment` joins `slug::RESERVED`; `HelperBridge::comments` has a default body.
- F24: the node-diagnostics scrub test runs at binary level with a path-carrying failure.
- F25: `PageCache::stats` is async.
- F26: `jobs_created_idx` added; plugin failure buckets returned as hourly sums per hook.
- F27: reqwest redirects are off; `Egress::fetch` follows at most 3 hops, each re-checked.
- F28: `http_fetch(plugin, hosts, req)` passes the granted list.
- F29: the test TLS server uses the ring provider explicitly.
- F30: the production resolver returns port 0 so the URL's 443 applies; `allow_port` dropped from the test overrides.
- F31: the cookie rename ripples into every test harness; `AppState` derives `secure` from its origin.
- F32: N2 as "a wait-shortened cut is a failure" without a `ran` parameter (superseded by R20).
- F33: the `types().list()` count test needs a counted `Tx` wrapper.
- F34: `cache.search_ttl` allows `0` (not cached).
- F35: the upload-slot test gates the request body client-side; no server seam needed.

## Controller rulings

- Ruling R1 (product, replaces P6): v1 also accepts common documents (pdf already; docx, xlsx, pptx, odt, ods, odp, txt, csv, zip), always served download-only (`attachment`, `nosniff`, never inline, never sniffed) — WordPress users expect document uploads, download-only keeps them from running in the site origin — cost: a wider type table and a test per class. As built PDF stays inline under `sandbox` (F14).
- Ruling R2 (product, P11): comments on by default for posts; `comments.moderation` defaults to `known` (a returning approved commenter posts directly, first comments are held) — WordPress's default; `all` stays available — cost: the spoofing risk documented in 07 §1 (F19).
- Ruling R3: pre-flight findings F1–F35 accepted as proposed, binding — cost if wrong: a fix round in the owning task.
- Ruling R4: Task 1: extra indexes, the F5/F19 interface shapes, Postgres `author_email` collation kept, jsonb key reorder on replay accepted; the SQLite claim rewrite (`INDEXED BY` + `UNION ALL`) verified against the M1.5 job tests — cost: none.
- Ruling R5: Task 2's stricter SVG refusals accepted (non-UTF-8 declarations, XML 1.1, NUL, missing xmlns → `NotSvg`; `<a>` dropped with its subtree) — cost: some odd but valid SVGs are refused.
- Ruling R6: SVG fix round: refuse `image-set(`/`image(`/`cross-fade(`/`element(`; paint/mask/clip/filter/marker references only `none` or `url(#…)`; at most 128 `<use>`, no `use`→`use` chains (`TooManyUses`); a DOCTYPE only without internal subset and with a standard SVG 1.0/1.1 public id, dropped; `NotXml` detail is fixed text plus a position — cost: SVGs using those features are refused or lose them.
- Ruling R7: Task 3: `create_in(&dyn Tx)` with a savepoint slug retry, media writes on the `media:write` scope with content role rules (a media token can upload), `MediaRepo::get_for_update` (lost-update guard, both backends) — cost: none.
- Ruling R8: Task 3 deviations accepted: associated `publish_in`, `trash` returns `ContentItem`, an empty patch bumps the version; Cf characters refused in file names only (titles and alt keep ZWJ) — cost: none.
- Ruling R9: Task 4: the `UploadError` variants, the slot held by `StagedUpload`, `422` for a missing file name, `#!`-led `.txt` refused, orphan blob per M1b R2 — cost: none.
- Ruling R10: Task 5: boxed `Response`, `finish(HeaderMap)`, awaited release on error; documented in 07 §1: a failed finish leaves the key pending ≤ 1 h, an upload past the 1 h stale window can coexist with a takeover's item, a drop during the commit frees the key — cost: each can give a second item for one key.
- Ruling R11: Task 6: GPS EXIF stays in the stored original (as WordPress does); variants never carry EXIF/XMP; documented in 03 §4, ROADMAP M4 "optional location-metadata stripping on upload" — cost: a photo's location is readable from its original URL.
- Ruling R12: refuse images whose decoded buffer (w × h × bytes per pixel) exceeds 512 MiB before decode — image 0.25.10 allocates the main buffer outside `max_alloc` — cost: some 100 MP 16-bit images are refused.
- Ruling R13: Task 7: always invalidate on moderate/delete, orphaned replies shown at depth 0, `comments.by_account` in `0006`; bidi controls (U+202A–202E, U+2066–2069) refused in comment and display names, ZWJ allowed; the two-step delete race accepted — cost: none.
- Ruling R14: Task 8: the `422`/`404`/`400` shapes of the form, reply forms in `<details>` up to depth 5, the pending notice only while the thread is open; comment pagination carried to M2b — cost: a thread shows only the oldest 500 comments.
- Ruling R15: Task 9: the wider scrub (over-matching is the safe direction), `active_theme`, `semver` in `ferrite-http`, the F24 ENOTDIR test — cost: some harmless text is scrubbed.
- Ruling R16: scrubbing errs on over-scrub: spaced paths, S3 bucket/key, AKIA ids, `host:port` and bearer values are scrubbed too — a leak is worse than an unreadable message — cost: operators read the node log more often.
- Ruling R17: Task 10: a retry of an unhandled kind is allowed with `handled = false` as the warning (a node may gain the handler later, M1.5 P5); prunes re-check status and age in their outer `WHERE` (a just-retried dead job is kept); `jobs_kind_created_idx` added — cost: none.
- Ruling R18: Task 11 deviations accepted: the stricter IPv6 table, IP-literal refusal, the post-connect peer check, `proxy-*`/User-Agent rules, rustls as a normal dependency, extra error classes — cost: none.
- Ruling R19: egress roots: platform verifier → webpki-roots fallback (warning) → `http` unconfigured (error); boot never fails on egress (principle 1); also the SIIT range, a 16-slot DNS lookup semaphore, cross-host 307/308 refused — cost: a host without a CA bundle trusts the compiled-in Mozilla roots.
- Ruling R20: N2 keeps the brief's `ended_by_budget(failure, ran)` plus a `#[cfg(test)]` seam, superseding F32; behaviour is the same today because a wait never exceeds the deadline, and the kept version stays correct if waits ever grow; T10's `budget_cut` counter follows the same rule — cost: one extra parameter and a test-only seam.
