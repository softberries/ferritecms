//! What an import run did, as JSON ([`Report::to_json`]) and as one self-contained HTML
//! page ([`Report::to_html`]).
//!
//! The passes fill the report as they go; `finish` then turns what they collected
//! while converting (the run's conversion [`Notes`]) and the `wp.*` meta they kept into
//! the report's own sections, with item titles and new paths read from the IR.
//!
//! Conversion tallies other than unknown blocks and shortcodes all go to
//! `dropped_formatting`, keyed by what was lost: `mark:<name>` (formatting dropped with
//! the text kept), `tag:<name>` (elements with no place in the block tree), `h1 demoted`,
//! `images not imported`, `bad block attributes`, `too deep` (markup kept as text) and
//! `page breaks`.

use std::collections::BTreeMap;

use ferrite_domain::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ImportError;
use crate::convert::{Notes, Tally};
use crate::ir::Ir;

/// At most this many `wp.*` meta keys are listed.
pub const MAX_UNMAPPED_META: usize = 50;

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The import run (`import_runs.id`).
    pub run_id: String,
    /// The normalised `<wp:base_blog_url>`.
    pub source: String,
    pub dry_run: bool,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    /// The new site's URL; item links in the HTML page are built from it.
    pub site_url: String,
    /// Per kind (`users`, `terms`, `taxonomies`, `types`, `media`, a post type,
    /// `comments`, `menus`, `redirects`, `options`, `fixups`).
    pub counts: BTreeMap<String, Counts>,
    /// Gutenberg blocks with no mapping (P12), with up to 5 example items.
    pub unknown_blocks: Vec<Example>,
    /// Shortcodes kept as text (P12), with up to 5 example items.
    pub unknown_shortcodes: Vec<Example>,
    /// What the conversion dropped or kept as text, by kind (see the module docs).
    pub dropped_formatting: BTreeMap<String, u64>,
    /// Attachments whose file was neither in the uploads folder nor fetchable (P13).
    pub missing_media: Vec<MissingMedia>,
    /// The `wp.*` meta keys kept, by how many items have them: the top
    /// [`MAX_UNMAPPED_META`], most first, then by key.
    pub unmapped_meta: Vec<(String, u64)>,
    /// ACF fields of an imported type that map to no field kind (P16); their values
    /// stay as `wp.<name>` meta.
    pub unmapped_acf: Vec<UnmappedAcf>,
    /// The accounts the import created, which must set a password (P9).
    pub users_must_reset: Vec<MustReset>,
    /// The redirects made and refused (P17).
    pub redirects: Redirects,
    /// Items whose WP slug was taken here, so core changed it (P8).
    pub slug_changes: Vec<SlugChange>,
    /// WP `future` items whose date had passed: published at that date (P7).
    pub scheduled_in_past: Vec<ItemRef>,
    /// The post permalink structure inferred from the old site and in effect (P17).
    pub permalink_structure: Option<String>,
    /// Everything else worth a line.
    pub notes: Vec<String>,
    /// What the body conversion lost, gathered while converting; `finish` maps it.
    #[serde(skip)]
    pub(crate) conversion: Notes,
    /// Items per kept `wp.*` meta key, gathered by pass 3; `finish` maps it.
    #[serde(skip)]
    pub(crate) meta_keys: BTreeMap<String, u64>,
    /// WP ids of `scheduled_in_past` items, gathered by pass 3; `finish` maps them.
    #[serde(skip)]
    pub(crate) past_schedules: Vec<i64>,
}

/// The outcomes of one kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
    /// Edited here since the last import: kept (P4).
    pub kept: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// A name (block or shortcode) with how often it came up and some of its items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    pub name: String,
    pub count: u64,
    pub items: Vec<ItemRef>,
}

/// An imported item as the report names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRef {
    pub wp_id: i64,
    pub title: String,
    /// Its path on the new site, when it was imported.
    pub new_path: Option<String>,
}

/// An attachment whose file is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingMedia {
    pub wp_id: i64,
    /// `_wp_attached_file`.
    pub path: String,
    /// `<wp:attachment_url>`.
    pub url: String,
}

/// An ACF field kept as raw meta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmappedAcf {
    /// The field's (ACF) name.
    pub field: String,
    /// The ACF field type.
    pub kind: String,
    /// Items of the field's types holding a value of it.
    pub items: u64,
}

/// An account created by the import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MustReset {
    pub login: String,
    pub email: String,
    /// `created`, or `created with a placeholder e-mail` when the WXR one was invalid.
    pub how: String,
}

/// The redirects of the run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redirects {
    pub created: u64,
    /// In input order.
    pub refused: Vec<Refused>,
}

/// A redirect that was not made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub source: String,
    pub target: String,
    pub reason: String,
}

/// An item whose slug changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlugChange {
    pub wp_id: i64,
    pub from: String,
    pub to: String,
}

impl Report {
    /// The counts of `kind`, made on first use.
    pub(crate) fn counts_of(&mut self, kind: &str) -> &mut Counts {
        self.counts.entry(kind.to_owned()).or_default()
    }

    pub(crate) fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    /// The report as pretty-printed JSON.
    pub fn to_json(&self) -> String {
        // Strings, numbers, maps with string keys: serialising cannot fail.
        serde_json::to_string_pretty(self)
            .unwrap_or_else(|e| format!("{{\"error\": {:?}}}", e.to_string()))
    }

    /// The report as one HTML page: inline CSS (light and dark), no scripts, no external
    /// URLs; every value escaped; item links built from `site_url`.
    pub fn to_html(&self) -> String {
        let mut h = Html::default();
        h.raw("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
        h.raw("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
        h.raw("<title>WordPress import report</title><style>");
        h.raw(CSS);
        h.raw("</style></head><body><main>");
        h.raw("<h1>WordPress import report</h1><dl>");
        h.pair("Source", &self.source);
        h.pair("Site", &self.site_url);
        h.pair("Run", &self.run_id);
        h.pair("Dry run", if self.dry_run { "yes" } else { "no" });
        h.pair("Started", &time(self.started_at));
        h.pair("Finished", &time(self.finished_at));
        h.pair(
            "Post permalinks",
            self.permalink_structure.as_deref().unwrap_or("unchanged"),
        );
        h.raw("</dl>");

        h.section("Counts", !self.counts.is_empty());
        h.table(
            &[
                "Kind",
                "Created",
                "Updated",
                "Unchanged",
                "Kept",
                "Skipped",
                "Failed",
            ],
            self.counts.iter().map(|(kind, c)| {
                vec![
                    Cell::Text(kind.clone()),
                    num(c.created),
                    num(c.updated),
                    num(c.unchanged),
                    num(c.kept),
                    num(c.skipped),
                    num(c.failed),
                ]
            }),
        );
        for (title, examples) in [
            ("Unknown blocks", &self.unknown_blocks),
            ("Unknown shortcodes", &self.unknown_shortcodes),
        ] {
            h.section(title, !examples.is_empty());
            h.table(
                &["Name", "Count", "Examples"],
                examples.iter().map(|e| {
                    vec![
                        Cell::Text(e.name.clone()),
                        num(e.count),
                        Cell::Items(e.items.clone()),
                    ]
                }),
            );
        }
        h.section("Dropped formatting", !self.dropped_formatting.is_empty());
        h.table(
            &["What", "Count"],
            self.dropped_formatting
                .iter()
                .map(|(k, v)| vec![Cell::Text(k.clone()), num(*v)]),
        );
        h.section("Missing media", !self.missing_media.is_empty());
        h.table(
            &["WP id", "Path", "Old URL"],
            self.missing_media.iter().map(|m| {
                vec![
                    num_i(m.wp_id),
                    Cell::Text(m.path.clone()),
                    Cell::Text(m.url.clone()),
                ]
            }),
        );
        h.section("Meta kept as wp.*", !self.unmapped_meta.is_empty());
        h.table(
            &["Key", "Items"],
            self.unmapped_meta
                .iter()
                .map(|(k, v)| vec![Cell::Text(k.clone()), num(*v)]),
        );
        h.section("Unmapped ACF fields", !self.unmapped_acf.is_empty());
        h.table(
            &["Field", "Type", "Items"],
            self.unmapped_acf.iter().map(|a| {
                vec![
                    Cell::Text(a.field.clone()),
                    Cell::Text(a.kind.clone()),
                    num(a.items),
                ]
            }),
        );
        h.section(
            "Accounts that must set a password",
            !self.users_must_reset.is_empty(),
        );
        h.table(
            &["Login", "E-mail", "How"],
            self.users_must_reset.iter().map(|u| {
                vec![
                    Cell::Text(u.login.clone()),
                    Cell::Text(u.email.clone()),
                    Cell::Text(u.how.clone()),
                ]
            }),
        );
        h.section("Redirects", true);
        h.raw("<p>");
        h.text(&format!("{} made.", self.redirects.created));
        h.raw("</p>");
        h.table(
            &["Refused source", "Target", "Reason"],
            self.redirects.refused.iter().map(|r| {
                vec![
                    Cell::Text(r.source.clone()),
                    Cell::Text(r.target.clone()),
                    Cell::Text(r.reason.clone()),
                ]
            }),
        );
        h.section("Changed slugs", !self.slug_changes.is_empty());
        h.table(
            &["WP id", "WordPress", "Here"],
            self.slug_changes.iter().map(|c| {
                vec![
                    num_i(c.wp_id),
                    Cell::Text(c.from.clone()),
                    Cell::Text(c.to.clone()),
                ]
            }),
        );
        h.section(
            "Scheduled in the past, published",
            !self.scheduled_in_past.is_empty(),
        );
        h.table(
            &["Items"],
            std::iter::once(vec![Cell::Items(self.scheduled_in_past.clone())])
                .filter(|_| !self.scheduled_in_past.is_empty()),
        );
        h.section("Notes", !self.notes.is_empty());
        h.raw("<ul>");
        for note in &self.notes {
            h.raw("<li>");
            h.text(note);
            h.raw("</li>");
        }
        h.raw("</ul></main></body></html>\n");
        h.finish(&self.site_url)
    }
}

/// The page's CSS: system fonts, light and dark.
const CSS: &str = "\
:root{color-scheme:light dark;--fg:#1d1d1f;--bg:#fff;--muted:#6e6e73;--line:#d2d2d7;--head:#f5f5f7}\
@media (prefers-color-scheme: dark){:root{--fg:#f5f5f7;--bg:#1d1d1f;--muted:#a1a1a6;--line:#424245;--head:#2c2c2e}}\
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,-apple-system,sans-serif}\
main{max-width:60rem;margin:0 auto;padding:2rem 1rem}\
h1{font-size:1.6rem;margin:0 0 1rem}h2{font-size:1.15rem;margin:2rem 0 .5rem}\
dl{display:grid;grid-template-columns:max-content 1fr;gap:.25rem 1rem;margin:0}dt{color:var(--muted)}dd{margin:0;overflow-wrap:anywhere}\
table{border-collapse:collapse;width:100%;font-size:.9rem}th,td{border-bottom:1px solid var(--line);padding:.35rem .5rem;text-align:left;vertical-align:top;overflow-wrap:anywhere}\
th{background:var(--head)}td.n{text-align:right;font-variant-numeric:tabular-nums}\
a{color:inherit}ul{padding-left:1.2rem}li{margin:.2rem 0}";

/// One table cell.
enum Cell {
    Text(String),
    Number(String),
    /// Item references: linked to the new site when imported.
    Items(Vec<ItemRef>),
}

fn num(n: u64) -> Cell {
    Cell::Number(n.to_string())
}

fn num_i(n: i64) -> Cell {
    Cell::Number(n.to_string())
}

fn time(at: Option<Timestamp>) -> String {
    at.map(|t| t.to_rfc3339()).unwrap_or_else(|| "-".into())
}

/// The page being written. Item links are written as markers and resolved against the
/// site URL in `finish`(Html::finish), so they are the only URLs the page holds.
#[derive(Default)]
struct Html {
    out: String,
}

impl Html {
    fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn text(&mut self, s: &str) {
        self.out.push_str(&escape(s));
    }

    fn pair(&mut self, name: &str, value: &str) {
        self.raw("<dt>");
        self.text(name);
        self.raw("</dt><dd>");
        self.text(value);
        self.raw("</dd>");
    }

    /// A section heading; a section with nothing to show says so.
    fn section(&mut self, title: &str, any: bool) {
        self.raw("<h2>");
        self.text(title);
        self.raw("</h2>");
        if !any {
            self.raw("<p>None.</p>");
        }
    }

    fn table(&mut self, head: &[&str], rows: impl Iterator<Item = Vec<Cell>>) {
        let mut rows = rows.peekable();
        if rows.peek().is_none() {
            return;
        }
        self.raw("<table><thead><tr>");
        for h in head {
            self.raw("<th>");
            self.text(h);
            self.raw("</th>");
        }
        self.raw("</tr></thead><tbody>");
        for row in rows {
            self.raw("<tr>");
            for cell in row {
                match cell {
                    Cell::Text(t) => {
                        self.raw("<td>");
                        self.text(&t);
                    }
                    Cell::Number(n) => {
                        self.raw("<td class=\"n\">");
                        self.text(&n);
                    }
                    Cell::Items(items) => {
                        self.raw("<td>");
                        for (i, item) in items.iter().enumerate() {
                            if i > 0 {
                                self.raw(", ");
                            }
                            self.item(item);
                        }
                    }
                }
                self.raw("</td>");
            }
            self.raw("</tr>");
        }
        self.raw("</tbody></table>");
    }

    /// `title (WP id)`, linked to the new site when the item has a path there.
    fn item(&mut self, item: &ItemRef) {
        let label = if item.title.trim().is_empty() {
            format!("#{}", item.wp_id)
        } else {
            format!("{} (#{})", item.title, item.wp_id)
        };
        match &item.new_path {
            Some(path) if path.starts_with('/') && !path.starts_with("//") => {
                self.raw("<a href=\"");
                self.raw(LINK_MARK);
                self.text(path);
                self.raw("\">");
                self.text(&label);
                self.raw("</a>");
            }
            _ => self.text(&label),
        }
    }

    /// The page, with item links on `site_url` (none when it is not an http(s) URL).
    fn finish(self, site_url: &str) -> String {
        let base = site_url.trim().trim_end_matches('/');
        let usable = url::Url::parse(base)
            .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some());
        let base = if usable { escape(base) } else { String::new() };
        self.out.replace(LINK_MARK, &base)
    }
}

/// Stands for the site URL in item links until [`Html::finish`].
const LINK_MARK: &str = "\u{1}SITE\u{1}";

/// Turns what the passes gathered into the report's sections (see the module docs),
/// with titles and new paths of example items read from `ir`, and adds the standing
/// notes.
pub(crate) async fn finish(report: &mut Report, ir: &Ir) -> Result<(), ImportError> {
    let paths: std::collections::HashMap<i64, String> = ir
        .content_paths()
        .await?
        .into_iter()
        .map(|(wp_id, _, path)| (wp_id, path))
        .collect();
    let mut titles: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    let notes = std::mem::take(&mut report.conversion);
    let past = std::mem::take(&mut report.past_schedules);
    let mut wanted: Vec<i64> = notes
        .unknown_blocks
        .values()
        .chain(notes.unknown_shortcodes.values())
        .flat_map(|t| t.items.iter().copied())
        .chain(past.iter().copied())
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    for wp_id in wanted {
        if let Some(item) = ir.item(wp_id).await? {
            titles.insert(wp_id, item.title);
        }
    }
    let item_ref = |wp_id: i64| ItemRef {
        wp_id,
        title: titles.get(&wp_id).cloned().unwrap_or_default(),
        new_path: paths.get(&wp_id).cloned(),
    };
    let examples = |tallies: &BTreeMap<String, Tally>| -> Vec<Example> {
        tallies
            .iter()
            .map(|(name, t)| Example {
                name: name.clone(),
                count: t.count,
                items: t.items.iter().map(|id| item_ref(*id)).collect(),
            })
            .collect()
    };
    report.unknown_blocks = examples(&notes.unknown_blocks);
    report.unknown_shortcodes = examples(&notes.unknown_shortcodes);
    report.dropped_formatting = dropped(&notes);
    report.scheduled_in_past = past.into_iter().map(item_ref).collect();
    report.unmapped_meta = top_meta(&std::mem::take(&mut report.meta_keys));
    if report.counts.get("media").is_some_and(|c| c.created > 0) {
        report.note(
            "image sizes (variants) are made by the media.variants jobs of the next `ferrite serve`",
        );
    }
    report.note("no plugin actions (post.saved, post.published) fired for imported items");
    Ok(())
}

/// The top [`MAX_UNMAPPED_META`] keys of `counts`, most first, then by key.
fn top_meta(counts: &BTreeMap<String, u64>) -> Vec<(String, u64)> {
    let mut top: Vec<(String, u64)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(MAX_UNMAPPED_META);
    top
}

/// The conversion tallies as `dropped_formatting` (see the module docs).
fn dropped(notes: &Notes) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for (name, t) in &notes.dropped_marks {
        out.insert(format!("mark:{name}"), t.count);
    }
    for (name, t) in &notes.unsupported_tags {
        out.insert(format!("tag:{name}"), t.count);
    }
    for (name, t) in [
        ("h1 demoted", &notes.h1_demoted),
        ("images not imported", &notes.missing_media),
        ("bad block attributes", &notes.bad_block_attrs),
        ("too deep", &notes.too_deep),
        ("page breaks", &notes.page_breaks),
    ] {
        if t.count > 0 {
            out.insert(name.to_owned(), t.count);
        }
    }
    out
}

/// `text` with `&`, `<`, `>`, `"` and `'` escaped for HTML text and attributes.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // The link marker never comes from a value.
            '\u{1}' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sample() -> Report {
        let item = |wp_id, title: &str, path: Option<&str>| ItemRef {
            wp_id,
            title: title.into(),
            new_path: path.map(Into::into),
        };
        let mut counts = BTreeMap::new();
        counts.insert(
            "post".to_owned(),
            Counts {
                created: 3,
                kept: 1,
                skipped: 2,
                ..Counts::default()
            },
        );
        Report {
            run_id: "0190-run".into(),
            source: "http://wp.test/\"onmouseover=\"x".into(),
            dry_run: false,
            started_at: Some(Timestamp::now()),
            finished_at: Some(Timestamp::now()),
            site_url: "https://new.test".into(),
            counts,
            unknown_blocks: vec![Example {
                name: "acme/callout".into(),
                count: 2,
                items: vec![
                    item(7, "<script>x</script>", Some("/a/")),
                    item(8, "Gone", None),
                ],
            }],
            unknown_shortcodes: vec![Example {
                name: "contact-form".into(),
                count: 1,
                items: vec![item(9, "Contact", Some("/contact/"))],
            }],
            dropped_formatting: [("mark:underline".to_owned(), 4)].into(),
            missing_media: vec![MissingMedia {
                wp_id: 4,
                path: "2024/03/a.png".into(),
                url: "http://wp.test/wp-content/uploads/2024/03/a.png".into(),
            }],
            unmapped_meta: vec![("wp.color".into(), 3)],
            unmapped_acf: vec![UnmappedAcf {
                field: "quotes".into(),
                kind: "repeater".into(),
                items: 1,
            }],
            users_must_reset: vec![MustReset {
                login: "erin".into(),
                email: "erin@example.test".into(),
                how: "created".into(),
            }],
            redirects: Redirects {
                created: 2,
                refused: vec![Refused {
                    source: "/".into(),
                    target: "/home/".into(),
                    reason: "the site root".into(),
                }],
            },
            slug_changes: vec![SlugChange {
                wp_id: 10,
                from: "about".into(),
                to: "about-2".into(),
            }],
            scheduled_in_past: vec![item(11, "Late", Some("/late/"))],
            permalink_structure: Some("/%year%/%slug%/".into()),
            notes: vec!["a <b>note</b>".into()],
            ..Report::default()
        }
    }

    #[test]
    fn report_json_round_trips() {
        let report = sample();
        let json = report.to_json();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
        assert!(json.contains("\"unknown_blocks\""), "{json}");
    }

    #[test]
    fn report_html_escapes_every_value() {
        let html = sample().to_html();
        assert!(html.starts_with("<!doctype html>"), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert!(html.contains("&lt;script&gt;x&lt;/script&gt;"), "{html}");
        assert!(
            html.contains("http://wp.test/&quot;onmouseover=&quot;x"),
            "{html}"
        );
        assert!(html.contains("a &lt;b&gt;note&lt;/b&gt;"), "{html}");
        assert!(html.contains("prefers-color-scheme: dark"), "{html}");
        // An imported item links to the new site; one not imported is not a link.
        assert!(html.contains("<a href=\"https://new.test/a/\">"), "{html}");
        assert!(!html.contains("href=\"/"), "{html}");
    }

    #[test]
    fn report_html_has_no_external_urls() {
        let report = sample();
        let mut html = report.to_html();
        // What may hold `http`: escaped values of the report, and links to the site.
        for value in [
            report.source.as_str(),
            report.site_url.as_str(),
            report.missing_media[0].url.as_str(),
        ] {
            html = html.replace(&escape(value), "");
        }
        html = html.replace("href=\"https://new.test/", "");
        assert!(!html.contains("http"), "{html}");
        for banned in ["<link", "<img", "src=", "url(", "@import"] {
            assert!(!html.contains(banned), "{banned}: {html}");
        }
    }

    #[test]
    fn report_unmapped_meta_is_capped_and_sorted() {
        let mut counts = BTreeMap::new();
        for n in 0..60u64 {
            counts.insert(format!("wp.k{n:02}"), n % 7);
        }
        counts.insert("wp.a".into(), 6);
        let top = top_meta(&counts);
        assert_eq!(top.len(), MAX_UNMAPPED_META);
        assert_eq!(top[0], ("wp.a".to_owned(), 6));
        assert_eq!(top[1], ("wp.k06".to_owned(), 6));
        assert!(
            top.windows(2)
                .all(|w| w[0].1 > w[1].1 || (w[0].1 == w[1].1 && w[0].0 < w[1].0))
        );
    }

    #[test]
    fn report_conversion_tallies_become_dropped_formatting() {
        let mut notes = Notes::default();
        notes.dropped_mark("underline");
        notes.dropped_mark("underline");
        notes.unsupported_tag("font");
        notes.h1_demoted();
        notes.too_deep();
        notes.page_break();
        notes.missing_media();
        notes.bad_block_attrs();
        let dropped = dropped(&notes);
        let expected: BTreeMap<String, u64> = [
            ("bad block attributes", 1),
            ("h1 demoted", 1),
            ("images not imported", 1),
            ("mark:underline", 2),
            ("page breaks", 1),
            ("tag:font", 1),
            ("too deep", 1),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        assert_eq!(dropped, expected);
    }
}
