# M4-3 exit: examples and docs

**Criterion** ([ROADMAP](../ROADMAP.md), M4-3): example plugins (SEO, privacy-friendly
analytics, a contact form), the starter themes `docs` and `business` with the template
helpers a docs theme needs and Markdown import, a docs site built with FerriteCMS itself,
and `ferrite doctor`. M4 ruling P41 split it in two; each half has its own exit.

**Result: done** (2026-10-31, branch `m4-3`). Nothing is released, published or hosted.

| Part | What it delivered | Exit |
|---|---|---|
| M4-3a (rulings P42–P53, R-pf1, X1–X7) | the example plugins `seo`, `analytics` and `contact-form` with their SDK helpers, shipped as the release asset `ferrite-plugins-<version>.tar.gz`; `ferrite doctor` (25 checks, exit 0/2/1/3) | [`m4-3a-exit.md`](m4-3a-exit.md): end to end from the packed asset, doctor in several states on both backends, the final review's fixes; workspace 3656 passed, 0 failed on `m4-3` after its merge |
| M4-3b (rulings P54–P65, R-b1–R-b17) | heading ids and `post.toc`; `page_tree()` (breadcrumbs, previous/next) and `query(order="menu")`; the `docs` and `business` themes; `seed demo --profile business`; `ferrite import markdown` (crate `ferrite-import-md`); the docs site in `site/` with `VERIFIED.md` and the `docs_site` test; axe checks on the three starter themes | [`m4-3b-exit.md`](m4-3b-exit.md): the docs site built and viewed, a real contact-form submit on the business site, the WordPress corpus unchanged, the full gates |

Rulings: [`rulings/m4.md`](rulings/m4.md) ("M4-3a plan rulings", "M4-3a execution
rulings", "M4-3b plan rulings", "M4-3b execution rulings"). No migration in either half
(the schema stays at 12) and no WIT change (the world stays `0.1.0`).

What is not verified, the accepted limits and the follow-ups are in each exit; the
follow-ups are in the ROADMAP under M4-4, "Deferred from M4-3a" and "Deferred from M4-3b",
and the left-out features under "Later".

**For the user** (details in each exit): the release gate of
[`m4-2-exit.md`](m4-2-exit.md) §6, with the plugins asset as the seventh signed asset
(M4-3a §6); where and whether to host the docs site, and the `docs` theme's lack of syntax
highlighting (M4-3b §6).

Next: M4-4, the follow-up lists.
