# CMS competitor review (2026-11-02)

The input for [M6](../../ROADMAP.md#m6--editorial-platform). Gathered from vendor docs,
changelogs and pricing pages on 2026-11-02; every row names its source. Contentful's and
Webflow's pricing pages did not load (429 / parse error), so their plan tiers are not
verified. EmDash facts come from trade press, not from its own docs. "None found" means no
source was found, not that the feature is proven absent.

## Per product

| Product | Notable features (tier) | Source |
|---|---|---|
| WordPress 7.0 (2026-05-20), 7.1 (2026-08-19) | AI Client and Connectors screen in core (7.0); real-time collaboration pulled from 7.0 and not in 7.1 (Gutenberg plugin only); Abilities API (6.9); MCP Adapter is a plugin; Notes (block comments, 6.9; @mentions 7.1); visual revisions, per-device block visibility (7.0) | wordpress.org/news/2026/05/wordpress-7-0-armstrong/, searchenginejournal.com/wordpress-7-0-will-ship-without-real-time-collaboration/574330/, wordpress.org/news/2026/08/mary-lou/, github.com/WordPress/mcp-adapter |
| Ghost 6.0 (2025-08) | ActivityPub with a social reader (free, also self-hosted); cookie-free analytics (self-hosted needs a Tinybird key; Ghost(Pro) Publisher+); paid memberships, tips; newsletter designer; recommendations; members-only comments | ghost.org/changelog/6/ |
| Payload 3 | REST, GraphQL and Local APIs; field-level access; live preview; drafts and autosave; plugins for forms, search, SEO, redirects, multi-tenant, MCP, Stripe; Visual Editor, publishing workflows, A/B tests, AI, SSO are Enterprise | payloadcms.com/docs/getting-started/what-is-payload, payloadcms.com/docs/plugins/overview |
| Strapi 5 | i18n, draft & publish, MCP server, RBAC free; Live Preview, Releases, Content History need Growth; Review Workflows and Audit Logs need Enterprise | docs.strapi.io/cms/features.md |
| Directus 11 / v12 (2026-05) | Drafts and publishing, AI translations, MCP over OAuth 2.1 (v12); Visual Editor; Flows automation (events, webhooks, cron); scheduled releases and approvals only "next" | directus.com/resources/v12-built-for-the-whole-team, directus.io/docs/guides/automate/triggers |
| Statamic 6 (2026-01-28) | Command palette, 2FA, passkeys, elevated sessions; Pro ($349/site) for multisite and multilingual, revisions, Git, REST/GraphQL | laravel-news.com/statamic-6-is-officially-released, statamic.com/pricing |
| Craft 5 | Solo (free): multisite, locales, GraphQL, live preview, versioning; Team $279, Pro $399; MCP only through third-party plugins | craftcms.com/pricing, plugins.craftcms.com/mcp |
| Kirby 5 | Multilingual in core; shareable previews of unsaved changes | getkirby.com/docs/guide/languages, getkirby.com/releases/5 |
| Sanity | Real-time collaboration and visual editing on every plan; scheduled drafts, comments, AI Assist on Growth; Content Releases a paid add-on; remote MCP server | sanity.io/pricing, sanity.io/blog/sanity-remote-mcp-server-is-generally-available |
| Contentful | Environments and aliases; Launch/Timeline release bundles; Images API; MCP server; personalisation and A/B (Ninetailed, paid). Tiers not verified | contentful.com/blog/introducing-timeline/, contentful.com/developers/docs/tools/mcp-server/ |
| Webflow | Localize (add-on), Analyze (add-on), Optimize (sales-led), page branching (Team/Enterprise) | webflow.com/feature/analyze, help.webflow.com/hc/en-us/articles/46651751861139-Page-branching |
| Framer | A/B tests via an add-on (Pro+); AI-translated locales ($20 each) | framer.com/pricing |
| EmDash (Cloudflare; 1.0 on 2026-09-28, MIT, Astro) | Sandboxed plugins with admin-approved permissions; signed plugin registry on AT Protocol; MCP server; the sandbox is not available off Cloudflare | cmswire.com/digital-experience/cloudflare-releases-emdash-1-0-cms-with-decentralized-plugin-registry/, siliconangle.com/2026/04/02/cloudflare-debuts-emdash-challenge-aging-wordpress-ai-native-cms/ |

## Offered by none, or only behind enterprise tiers

| Gap | Evidence |
|---|---|
| Approval workflows in a free or open-source tier | Payload and Strapi: Enterprise; Directus: planned |
| Accessibility checks while writing | none found in the 13 products |
| Release bundles with a whole-site preview | Contentful environments (SaaS); Strapi Releases (Growth), Sanity (add-on) without a site preview |
| Reviewing and reverting what an AI assistant changed | none found |
| A plugin sandbox that runs anywhere | EmDash's runs only on Cloudflare |
