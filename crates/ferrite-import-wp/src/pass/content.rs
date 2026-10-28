//! Pass 3: posts, pages and custom-type items → content items (P7, P8, P10, P14, P16).
//!
//! Types go in file order; within a type, parents come before their children. Each item
//! becomes an [`ImportContent`]: the status and dates of P7, the WP slug (a trashed item's
//! `_wp_desired_post_slug`), the author and terms through the import map (the primary
//! category of Yoast or Rank Math first, as `%category%` permalinks read the first
//! category), the parent when it is mapped, the body converted with a [`Rewrite`] over the
//! [`MediaIndex`] and the old item URLs, the featured image, ACF values (P16), SEO (P10)
//! and the remaining meta as `wp.*` ([`crate::meta`]).
//!
//! A link or relation to an item of the export that is not written yet keeps its old
//! value, and the item is queued for the fix-up ([`super::fixup`]), which converts it
//! again once every item exists. Each written item's id and path are kept in the IR, so
//! the fix-up and a resumed run have them.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use ferrite_core::fields::{FEATURED_IMAGE, FieldDef, MAX_ROWS, MAX_TEXT};
use ferrite_core::import::{ContentOutcome, ImportBatch, ImportContent, Written};
use ferrite_domain::{ContentId, ContentStatus, ImportMapKind, TermId, Timestamp, UserId};
use futures::TryStreamExt;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::options::Applied;
use super::types::{Schema, ensure, plan, taxonomy_name, type_fields, type_name};
use super::{Census, meta, wp_time};
use crate::ImportError;
use crate::acf::{AcfField, AcfModel, field_defs, is_layout};
use crate::convert::{ImageRef, Notes, Rewrite, body};
use crate::ir::Ir;
use crate::model::{Channel, Item};
use crate::php::unserialize;
use crate::report::SlugChange;
use crate::run::{Report, Session};
use crate::urls::{MediaIndex, key, percent_decode};
use sha2::{Digest, Sha256};

/// The meta holding a post's featured image (an attachment id).
const THUMBNAIL: &str = "_thumbnail_id";
/// The meta holding a trashed item's slug before WordPress appended `__trashed`.
const DESIRED_SLUG: &str = "_wp_desired_post_slug";
/// The SEO plugins' primary category (a WP term id).
const PRIMARY_CATEGORY: [&str; 2] = [
    "_yoast_wpseo_primary_category",
    "rank_math_primary_category",
];
/// Where uploads live under a WordPress site.
const UPLOADS: &str = "/wp-content/uploads/";

/// What links, images and relations resolve against: the export's items and their new
/// ids and paths, the media and the attachments.
pub(crate) struct Links {
    /// Hosts of the old site (`<link>`, `base_site_url`, `base_blog_url`), lower-case.
    hosts: Vec<String>,
    /// The base relative links are read against.
    base: Option<url::Url>,
    /// An item's old `<link>` path → its WP id.
    by_path: HashMap<String, i64>,
    /// Content items of the export: WP id → (post type, WP status).
    items: HashMap<i64, (String, String)>,
    /// Items written so far: WP id → (new id, path).
    pub(crate) written: HashMap<i64, (Uuid, String)>,
    pub(crate) media: MediaIndex,
    /// Imported attachments: WP id → media id.
    attachments: HashMap<i64, Uuid>,
}

impl Links {
    /// Reads the export's items, the paths already written (a resumed run) and the media
    /// index from the IR, and the imported attachments through the import map.
    pub(crate) async fn load(
        session: &Session<'_>,
        channel: &Channel,
    ) -> Result<Links, ImportError> {
        let ir = session.ir();
        let parsed: Vec<url::Url> = [
            &channel.link,
            &channel.base_site_url,
            &channel.base_blog_url,
        ]
        .into_iter()
        .filter_map(|u| url::Url::parse(u.trim()).ok())
        .collect();
        let mut hosts: Vec<String> = parsed
            .iter()
            .filter_map(|u| u.host_str().map(str::to_ascii_lowercase))
            .collect();
        hosts.dedup();
        let mut links = Links {
            hosts,
            base: parsed.into_iter().next(),
            by_path: HashMap::new(),
            items: HashMap::new(),
            written: HashMap::new(),
            media: MediaIndex::load(ir).await?,
            attachments: HashMap::new(),
        };
        let mut attachments = Vec::new();
        let mut all = ir.items(&[]);
        while let Some(item) = all.try_next().await? {
            if item.post_type == "attachment" {
                attachments.push(item.wp_id);
                continue;
            }
            if type_name(&item.post_type).is_none() {
                continue;
            }
            if let Some(path) = links.old_path(&item.link) {
                links.by_path.entry(path).or_insert(item.wp_id);
            }
            links
                .items
                .insert(item.wp_id, (item.post_type.clone(), item.status.clone()));
        }
        drop(all);
        for (wp_id, id, path) in ir.content_paths().await? {
            links.written.insert(wp_id, (id, path));
        }
        for chunk in attachments.chunks(session.batch_items()) {
            let mut batch = session.batch().await?;
            let found = mapped(&mut batch, ImportMapKind::Media, chunk).await;
            links
                .attachments
                .extend(session.finish_batch(batch, found).await?);
        }
        Ok(links)
    }

    /// The path of an old URL on the old site, decoded, with a trailing `/`; `None` for
    /// another host or a URL with a query (`?p=`, `?page_id=`, `?attachment_id=`,
    /// `?<type>=<slug>`: none of them is the path `/`).
    pub(crate) fn old_path(&self, link: &str) -> Option<String> {
        let url = self.on_site(link)?;
        if url.query().is_some() {
            return None;
        }
        Some(path_of(&url))
    }

    /// `link` as an absolute URL when it is on the old site (or relative to it).
    pub(crate) fn on_site(&self, link: &str) -> Option<url::Url> {
        let link = link.trim();
        let url = match url::Url::parse(link) {
            Ok(u) => u,
            Err(url::ParseError::RelativeUrlWithoutBase) => self.base.as_ref()?.join(link).ok()?,
            Err(_) => return None,
        };
        let host = url.host_str()?.to_ascii_lowercase();
        (matches!(url.scheme(), "http" | "https") && self.hosts.contains(&host)).then_some(url)
    }

    /// The WP item an old link names, and what of the link to keep after the item's new
    /// path: `?p=N` / `?page_id=N` name it by id (only the fragment is kept), any other
    /// link by its path, the query and fragment set aside and kept (`/hello/?utm=x#top`).
    fn target(&self, href: &str) -> Option<(i64, String)> {
        let url = self.on_site(href)?;
        let by_query = url
            .query_pairs()
            .find(|(k, _)| k == "p" || k == "page_id")
            .and_then(|(_, v)| v.parse::<i64>().ok());
        let fragment = url.fragment().map(|f| format!("#{f}")).unwrap_or_default();
        let (wp_id, rest) = match by_query {
            Some(id) => (id, fragment),
            None => {
                let id = *self.by_path.get(&path_of(&url))?;
                let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
                (id, format!("{query}{fragment}"))
            }
        };
        self.items.contains_key(&wp_id).then_some((wp_id, rest))
    }

    /// A URL as it should read on the new site: an old item's link becomes its new path,
    /// an upload its media URL, another URL on the old site its path (with query and
    /// fragment); anything else is kept.
    pub(crate) fn local_url(&self, url: &str) -> String {
        if let Some(m) = self.media.resolve(url) {
            return m.url.clone();
        }
        if let Some((wp_id, rest)) = self.target(url)
            && let Some((_, path)) = self.written.get(&wp_id)
        {
            return format!("{path}{rest}");
        }
        match self.on_site(url) {
            Some(u) => {
                let mut local = u.path().to_owned();
                if let Some(q) = u.query() {
                    local.push('?');
                    local.push_str(q);
                }
                if let Some(f) = u.fragment() {
                    local.push('#');
                    local.push_str(f);
                }
                local
            }
            None => url.trim().to_owned(),
        }
    }

    /// The WP post type and status of export item `wp_id`.
    pub(crate) fn item(&self, wp_id: i64) -> Option<&(String, String)> {
        self.items.get(&wp_id)
    }
}

/// The decoded path of `url`, with a trailing `/` (how old item paths are keyed).
fn path_of(url: &url::Url) -> String {
    let path = percent_decode(url.path());
    if path.ends_with('/') {
        path
    } else {
        format!("{path}/")
    }
}

/// Looks up the map rows of `kind` for `wp_ids`: WP id → target id, for those mapped.
async fn mapped(
    b: &mut ImportBatch<'_>,
    kind: ImportMapKind,
    wp_ids: &[i64],
) -> Result<Vec<(i64, Uuid)>, ImportError> {
    let mut out = Vec::new();
    for wp_id in wp_ids {
        if let Some(m) = b.lookup(kind, *wp_id).await? {
            out.push((*wp_id, m.id));
        }
    }
    Ok(out)
}

/// The resolver of one item's conversion: a [`Rewrite`] that also notes links and
/// relations waiting for later items.
pub(crate) struct Resolver<'a> {
    links: &'a Links,
    pending: Cell<bool>,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(links: &'a Links) -> Self {
        Self {
            links,
            pending: Cell::new(false),
        }
    }

    /// Whether something waits for an item not written yet.
    pub(crate) fn pending(&self) -> bool {
        self.pending.get()
    }

    /// The new id of export item `wp_id`; `None` (noted as pending when it is an item of
    /// the export) when it is not written yet.
    fn item_id(&self, wp_id: i64) -> Option<Uuid> {
        match self.links.written.get(&wp_id) {
            Some((id, _)) => Some(*id),
            None => {
                if self.links.items.contains_key(&wp_id) {
                    self.pending.set(true);
                }
                None
            }
        }
    }
}

impl Rewrite for Resolver<'_> {
    fn image(&self, src: &str) -> ImageRef {
        if let Some(m) = self.links.media.resolve(src) {
            return ImageRef::Media(m.id);
        }
        let upload =
            self.links.on_site(src).is_some() && key(src).is_some_and(|k| k.starts_with(UPLOADS));
        if upload {
            ImageRef::Missing(src.to_owned())
        } else {
            ImageRef::Url(src.to_owned())
        }
    }

    fn attachment(&self, wp_id: i64) -> Option<Uuid> {
        self.links.attachments.get(&wp_id).copied()
    }

    fn href(&self, href: &str) -> String {
        if let Some(m) = self.links.media.resolve(href) {
            return m.url.clone();
        }
        let Some((wp_id, rest)) = self.links.target(href) else {
            return href.to_owned();
        };
        match self.links.written.get(&wp_id) {
            Some((_, path)) => format!("{path}{rest}"),
            None => {
                self.pending.set(true);
                href.to_owned()
            }
        }
    }
}

/// An item's body and fields, as converted.
pub(crate) struct Converted {
    pub body: Value,
    pub fields: Value,
    pub notes: Notes,
    /// A link or relation waits for an item not written yet.
    pub pending: bool,
}

/// Converts `item`'s body and builds its fields: the featured image, and the mapped
/// ACF values of its type (`acf`, empty for a type without ACF fields).
pub(crate) fn convert(item: &Item, links: &Links, acf: &TypeAcf<'_>) -> Converted {
    let rw = Resolver::new(links);
    let mut notes = Notes {
        item: Some(item.wp_id),
        ..Notes::default()
    };
    let body = body::convert(&item.content, &rw, &mut notes);
    let mut fields = Map::new();
    if let Some(id) = meta(item, THUMBNAIL)
        .and_then(|v| v.trim().parse::<i64>().ok())
        .and_then(|wp| rw.attachment(wp))
    {
        fields.insert(FEATURED_IMAGE.to_owned(), json!(id.to_string()));
    }
    for (field, def) in &acf.mapped {
        let mut values = Values {
            item,
            rw: &rw,
            notes: &mut notes,
        };
        if let Some(v) = values.value(field, def, &field.name) {
            fields.insert(def.name.clone(), v);
        }
    }
    let pending = rw.pending();
    Converted {
        body,
        fields: Value::Object(fields),
        notes,
        pending,
    }
}

/// A post type's mapped ACF fields and the meta keys their values consume (each field's
/// name and, for a repeater, its row keys `<name>_<n>_…`), worked out once per type.
/// Empty for `post` and `page`, whose ACF values stay as meta (P16), and for types
/// without ACF groups.
pub(crate) struct TypeAcf<'a> {
    mapped: Vec<(&'a AcfField, FieldDef)>,
    names: HashSet<String>,
    prefixes: Vec<String>,
}

impl<'a> TypeAcf<'a> {
    pub(crate) fn of(post_type: &str, acf: &'a AcfModel) -> Self {
        let custom = !matches!(post_type, "post" | "page") && acf.has_groups_for(post_type);
        let mapped = if custom {
            type_fields(post_type, acf).mapped
        } else {
            Vec::new()
        };
        let names = mapped.iter().map(|(f, _)| f.name.clone()).collect();
        let prefixes = mapped
            .iter()
            .filter(|(f, _)| f.kind == "repeater")
            .map(|(f, _)| format!("{}_", f.name))
            .collect();
        Self {
            mapped,
            names,
            prefixes,
        }
    }

    /// Whether meta `key` holds one of these fields' values.
    fn consumes(&self, key: &str) -> bool {
        self.names.contains(key) || self.prefixes.iter().any(|p| key.starts_with(p.as_str()))
    }
}

/// Maps ACF values (P16).
struct Values<'a, 'b> {
    item: &'a Item,
    rw: &'a Resolver<'b>,
    notes: &'a mut Notes,
}

impl Values<'_, '_> {
    /// The value of `field` stored under meta `key`; `None` when empty or not mappable.
    fn value(&mut self, field: &AcfField, def: &FieldDef, key: &str) -> Option<Value> {
        let raw = meta(self.item, key)?.trim().to_owned();
        if raw.is_empty() {
            return None;
        }
        let id = |s: &str| s.trim().parse::<i64>().ok();
        match field.kind.as_str() {
            "text" | "textarea" | "email" | "url" | "password" => {
                let v = meta(self.item, key)?.replace('\0', "");
                Some(json!(v.chars().take(MAX_TEXT).collect::<String>()))
            }
            "wysiwyg" => Some(body::convert(&raw, self.rw, self.notes)),
            "number" | "range" => raw.parse::<i64>().map(Value::from).ok().or_else(|| {
                raw.parse::<f64>()
                    .ok()
                    .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            }),
            "true_false" => Some(json!(raw == "1")),
            "date_picker" => ymd(&raw).map(Value::from),
            "date_time_picker" => {
                wp_time(&raw, "").map(|_| json!(format!("{}Z", raw.replacen(' ', "T", 1))))
            }
            "select" | "radio" | "button_group" => def.options.contains(&raw).then(|| json!(raw)),
            "image" | "file" => id(&raw)
                .and_then(|wp| self.rw.attachment(wp))
                .map(|u| json!(u.to_string())),
            "post_object" | "page_link" => id(&raw)
                .and_then(|wp| self.rw.item_id(wp))
                .map(|u| json!(u.to_string())),
            "relationship" | "gallery" | "checkbox" => {
                let sub = def.fields.first()?.name.clone();
                let list: Vec<String> = match unserialize(&raw) {
                    Ok(v) => v
                        .entries()
                        .iter()
                        .filter_map(|(_, v)| v.as_text())
                        .collect(),
                    Err(_) => vec![raw.clone()],
                };
                let rows: Vec<Value> = list
                    .iter()
                    .filter_map(|v| match field.kind.as_str() {
                        "relationship" => id(v)
                            .and_then(|wp| self.rw.item_id(wp))
                            .map(|u| json!(u.to_string())),
                        "gallery" => id(v)
                            .and_then(|wp| self.rw.attachment(wp))
                            .map(|u| json!(u.to_string())),
                        _ => Some(json!(v)),
                    })
                    .take(MAX_ROWS)
                    .map(|v| json!({ sub.clone(): v }))
                    .collect();
                (!rows.is_empty()).then_some(Value::Array(rows))
            }
            "repeater" => {
                let count = raw.parse::<usize>().ok()?.min(MAX_ROWS);
                let subs: Vec<&AcfField> =
                    field.sub.iter().filter(|s| !is_layout(&s.kind)).collect();
                let mut rows = Vec::with_capacity(count);
                for n in 0..count {
                    let mut row = Map::new();
                    for (sub, sub_def) in subs.iter().zip(&def.fields) {
                        let sub_key = format!("{key}_{n}_{}", sub.name);
                        if let Some(v) = self.value(sub, sub_def, &sub_key) {
                            row.insert(sub_def.name.clone(), v);
                        }
                    }
                    rows.push(Value::Object(row));
                }
                (!rows.is_empty()).then_some(Value::Array(rows))
            }
            _ => None,
        }
    }
}

/// An ACF `Ymd` date as UTC midnight (RFC 3339).
fn ymd(raw: &str) -> Option<String> {
    let ok = raw.len() == 8 && raw.bytes().all(|b| b.is_ascii_digit());
    let text = ok.then(|| format!("{}-{}-{}T00:00:00Z", &raw[..4], &raw[4..6], &raw[6..]))?;
    serde_json::from_value::<Timestamp>(json!(text))
        .ok()
        .map(|_| text)
}

/// WP status → core status (P7); `Err` with the reason an item is skipped.
fn status(wp: &str) -> Result<ContentStatus, String> {
    Ok(match wp {
        "publish" => ContentStatus::Published,
        "draft" => ContentStatus::Draft,
        "pending" => ContentStatus::Pending,
        "private" => ContentStatus::Private,
        "trash" => ContentStatus::Trash,
        "future" => ContentStatus::Scheduled,
        other => return Err(format!("status {other:?}")),
    })
}

/// What pass 3 reads beside the items.
struct Context<'a> {
    links: Links,
    channel: Channel,
    /// Author login → WP author id.
    authors: HashMap<String, i64>,
    /// (WP taxonomy, decoded slug) → WP term id.
    terms: HashMap<(String, String), i64>,
    /// Post types with an item that has a parent (their types are hierarchical).
    hierarchical: HashSet<String>,
    /// On a dry run (and only then), pass 1's types and taxonomies, re-registered in
    /// every batch (pass 1's batches were rolled back).
    schema: Option<Schema>,
    admin: UserId,
    session: &'a Session<'a>,
}

/// One item ready to write, and what to report about it.
struct Prepared {
    content: ImportContent,
    notes: Notes,
    lines: Vec<String>,
    pending: bool,
}

/// What a written item came to.
struct Done {
    wp_id: i64,
    kind: String,
    outcome: ContentOutcome,
    path: Option<String>,
    pending: bool,
    /// The WP slug asked for.
    slug: Option<String>,
}

/// Imports every post, page and custom-type item.
pub(crate) async fn import(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    let ir = session.ir();
    let channel = ir.channel().await?;
    let order = order(ir).await?;
    if session.opts().keep_permalinks {
        set_permalinks(session, report).await?;
    }
    let acf = field_defs(ir).await?;
    let dry_run = session.opts().dry_run;
    let schema = if dry_run {
        let census = Census::scan(ir).await?;
        let wp_terms = ir.terms().await?;
        Some(plan(&census, &acf, &wp_terms, &mut Report::default()))
    } else {
        None
    };
    if dry_run {
        report.note(
            "dry run: every batch was rolled back, so later batches did not see the users, \
             terms, media and parents of earlier ones; items fall back to the importing \
             admin and lose those terms, images and parents in this report only; pass 4's \
             counts for comments, menus and redirects are not meaningful either (they \
             need the rolled-back items)",
        );
    }
    let mut cx = Context {
        schema,
        links: Links::load(session, &channel).await?,
        authors: ir
            .authors()
            .await?
            .into_iter()
            .map(|a| (a.login, a.wp_id))
            .collect(),
        terms: ir
            .terms()
            .await?
            .into_iter()
            .map(|t| ((t.taxonomy, percent_decode(&t.slug)), t.wp_id))
            .collect(),
        hierarchical: order
            .iter()
            .filter(|(_, ids)| ids.iter().any(|(_, parent)| *parent != 0))
            .map(|(t, _)| t.clone())
            .collect(),
        channel,
        admin: session.ctx().actor.user,
        session,
    };
    for (post_type, nodes) in &order {
        let Some(kind) = type_name(post_type) else {
            continue;
        };
        if is_acf_on_built_in(post_type, &acf) {
            report.note(format!(
                "ACF values of {post_type} items stay as wp.<name> meta (no fields on built-in types)"
            ));
        }
        let type_acf = TypeAcf::of(post_type, &acf);
        for chunk in parent_first(nodes).chunks(session.batch_items()) {
            let mut items = Vec::with_capacity(chunk.len());
            for wp_id in chunk {
                if let Some(item) = ir.item(*wp_id).await? {
                    items.push(item);
                }
            }
            write_chunk(&mut cx, &kind, &type_acf, items, report).await?;
        }
    }
    Ok(())
}

/// Infers the old site's post structure (P17) and, when there is one, sets
/// `permalink.post` in a schema batch before any item is written, under the P4 rule of
/// [`super::options::apply`] (the first import keeps a site's own structure). Ending
/// that batch swaps the run's rules, so every path pass 3 and the fix-up compute follows
/// it.
async fn set_permalinks(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    let ir = session.ir();
    let categories: HashMap<i64, String> = ir
        .terms()
        .await?
        .into_iter()
        .filter(|t| t.taxonomy == "category")
        .map(|t| (t.wp_id, t.slug))
        .collect();
    let slug_of = |wp: i64| categories.get(&wp).cloned();
    let mut posts: Vec<(Item, Option<String>)> = Vec::new();
    let mut items = ir.items(&["post"]);
    while let Some(item) = items.try_next().await? {
        if item.status != "publish" {
            continue;
        }
        let primary = PRIMARY_CATEGORY
            .iter()
            .find_map(|k| meta(&item, k))
            .and_then(|v| v.trim().parse::<i64>().ok())
            .and_then(slug_of)
            .or_else(|| {
                item.terms
                    .iter()
                    .find(|t| t.taxonomy == "category")
                    .map(|t| t.slug.clone())
            });
        posts.push((item, primary));
    }
    drop(items);
    let refs: Vec<(&Item, Option<&str>)> = posts.iter().map(|(i, c)| (i, c.as_deref())).collect();
    let Some(structure) = crate::permalinks::infer(&refs) else {
        report.note("the old post permalink structure could not be inferred; the site's is kept");
        return Ok(());
    };
    let mut batch = session.schema_batch().await?;
    let mut part = Report::default();
    let written = super::options::apply(
        &mut batch,
        ferrite_core::permalink::POST_OPTION,
        &json!(structure),
        &mut part,
    )
    .await;
    let applied = session.finish_batch(batch, written).await?;
    report.absorb(part);
    if matches!(applied, Applied::Created | Applied::Updated) {
        report.note(format!(
            "post permalinks set to {structure} as on the old site"
        ));
    }
    if applied != Applied::Kept {
        report.permalink_structure = Some(structure);
    }
    Ok(())
}

fn is_acf_on_built_in(post_type: &str, acf: &AcfModel) -> bool {
    matches!(post_type, "post" | "page") && acf.has_groups_for(post_type)
}

/// Converts and writes one chunk of items of `kind`, then records ids, paths and the
/// fix-up queue.
async fn write_chunk(
    cx: &mut Context<'_>,
    kind: &str,
    acf: &TypeAcf<'_>,
    items: Vec<Item>,
    report: &mut Report,
) -> Result<(), ImportError> {
    let session = cx.session;
    let mut batch = match &cx.schema {
        Some(_) => session.schema_batch().await?,
        None => session.batch().await?,
    };
    let written = match &cx.schema {
        Some(schema) => match ensure(&mut batch, schema, None).await {
            Ok(()) => write_items(&mut batch, cx, kind, acf, items, report).await,
            Err(e) => Err(e),
        },
        None => write_items(&mut batch, cx, kind, acf, items, report).await,
    };
    // The fix-up queue is written before the batch commits: once it is committed, a
    // resumed run sees these items unchanged and would never queue them. A batch that
    // then rolls back leaves extra entries, which the fix-up skips or finds nothing to do
    // for. A dry run writes none (its rows are rolled back).
    let written = match written {
        Ok(done) if !session.opts().dry_run => {
            let queue: Vec<i64> = done
                .iter()
                .filter(|d| {
                    d.pending && matches!(d.outcome.result, Written::Created | Written::Updated)
                })
                .map(|d| d.wp_id)
                .collect();
            session.ir().add_fixups(&queue).await.map(|()| done)
        }
        other => other,
    };
    let done = session.finish_batch(batch, written).await?;
    let mut rows = Vec::new();
    for d in done {
        let counts = report.counts_of(&d.kind);
        match d.outcome.result {
            Written::Created => counts.created += 1,
            Written::Updated => counts.updated += 1,
            Written::Unchanged => counts.unchanged += 1,
            Written::KeptEdited => counts.kept += 1,
        }
        if d.outcome.result == Written::KeptEdited {
            report.note(format!(
                "item {} was edited here since the last import; kept",
                d.wp_id
            ));
        }
        if let Some(slug) = &d.outcome.slug_changed {
            report.add_slug_change(SlugChange {
                wp_id: d.wp_id,
                from: d.slug.clone().unwrap_or_default(),
                to: slug.clone(),
            });
        }
        if d.outcome.schedule_past {
            report.add_past_schedule(d.wp_id);
        }
        if let Some(path) = d.path {
            let id = d.outcome.id.as_uuid();
            cx.links.written.insert(d.wp_id, (id, path.clone()));
            rows.push((d.wp_id, id, path));
        }
    }
    // A dry run's rows were rolled back: nothing for a later run. A crash before this
    // loses nothing: a resumed run records the paths of unchanged items again.
    if !session.opts().dry_run {
        session.ir().put_content_paths(&rows).await?;
    }
    Ok(())
}

/// Writes `items` in batch `b`; skipped items are counted in `report` here.
async fn write_items(
    b: &mut ImportBatch<'_>,
    cx: &Context<'_>,
    kind: &str,
    acf: &TypeAcf<'_>,
    items: Vec<Item>,
    report: &mut Report,
) -> Result<Vec<Done>, ImportError> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(prepared) = prepare(b, cx, kind, acf, &item, report).await? else {
            continue;
        };
        report.merge_conversion(&prepared.notes);
        for line in prepared.lines {
            report.note(line);
        }
        for (key, _) in &prepared.content.meta {
            *report.meta_keys.entry(key.clone()).or_default() += 1;
        }
        let slug = prepared.content.slug.clone();
        let outcome = b.content(prepared.content).await?;
        let path = b.path(outcome.id).await?;
        out.push(Done {
            wp_id: item.wp_id,
            kind: kind.to_owned(),
            outcome,
            path,
            pending: prepared.pending,
            slug,
        });
    }
    Ok(out)
}

/// Builds the [`ImportContent`] of `item`; `None` (counted as skipped) for a status
/// that is not imported.
async fn prepare(
    b: &mut ImportBatch<'_>,
    cx: &Context<'_>,
    kind: &str,
    acf: &TypeAcf<'_>,
    item: &Item,
    report: &mut Report,
) -> Result<Option<Prepared>, ImportError> {
    let status = match status(&item.status) {
        Ok(s) => s,
        Err(why) => {
            report.counts_of(kind).skipped += 1;
            report.note(format!("item {} skipped: {why}", item.wp_id));
            return Ok(None);
        }
    };
    let mut lines = Vec::new();
    let author = match cx.authors.get(&item.creator_login) {
        Some(wp) => b
            .lookup(ImportMapKind::User, *wp)
            .await?
            .map_or(cx.admin, |m| UserId::from_uuid(m.id)),
        None => cx.admin,
    };
    let parent = parent(b, cx, item, &mut lines).await?;
    let terms = terms(b, cx, item, &mut lines).await?;
    let converted = convert(item, &cx.links, acf);
    let (seo, seo_notes) = crate::seo::map(item, &cx.channel);
    lines.extend(seo_notes);
    let consumed = |k: &str| {
        k == THUMBNAIL
            || PRIMARY_CATEGORY.contains(&k)
            || crate::seo::SEO_KEYS.contains(&k)
            || acf.consumes(k)
    };
    let mut wp_meta = crate::meta::wp_meta(item, consumed);
    if item.is_sticky {
        wp_meta.push(("wp.sticky".into(), json!(true)));
    }
    let slug = match (status, meta(item, DESIRED_SLUG)) {
        (ContentStatus::Trash, Some(desired)) if !desired.trim().is_empty() => desired.to_owned(),
        _ => item.post_name.clone(),
    };
    let slug = percent_decode(slug.trim());
    let (published_at, created_at, updated_at) = dates(item);
    let excerpt = item.excerpt.trim();
    let mut content = ImportContent {
        wp_id: item.wp_id,
        hash: String::new(),
        kind: kind.to_owned(),
        title: item.title.replace('\0', ""),
        slug: Some(slug).filter(|s| !s.is_empty()),
        parent,
        author,
        status,
        published_at,
        created_at,
        updated_at,
        excerpt: (!excerpt.is_empty()).then(|| excerpt.replace('\0', "")),
        body: converted.body,
        fields: converted.fields,
        menu_order: item.menu_order,
        terms,
        seo,
        meta: wp_meta,
    };
    content.hash = content_hash(&content)?;
    Ok(Some(Prepared {
        content,
        notes: converted.notes,
        lines,
        pending: converted.pending,
    }))
}

/// The P4 hash of an item as it will be written: everything but its WP id, so a re-run
/// also updates an item whose WXR record is the same but whose links, images, featured
/// image, author, parent or terms resolve differently now (C2: the uploads given only
/// the second time).
fn content_hash(c: &ImportContent) -> Result<String, ImportError> {
    let id = |x: Option<ContentId>| x.map(|i| i.to_string());
    let json = serde_json::to_vec(&json!({
        "kind": c.kind,
        "title": c.title,
        "slug": c.slug,
        "parent": id(c.parent),
        "author": c.author.to_string(),
        "status": c.status.as_str(),
        "published_at": c.published_at,
        "created_at": c.created_at,
        "updated_at": c.updated_at,
        "excerpt": c.excerpt,
        "body": c.body,
        "fields": c.fields,
        "menu_order": c.menu_order,
        "terms": c.terms.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "seo": c.seo.as_ref().map(ferrite_core::content::SeoPatch::to_json),
        "meta": c.meta,
    }))?;
    Ok(Sha256::digest(&json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `published_at`, `created_at` and `updated_at` of an item (P7): the GMT dates, or the
/// local ones read as UTC when a GMT date is zero; `created_at` falls back to now and
/// `updated_at` to `created_at`.
fn dates(item: &Item) -> (Option<Timestamp>, Timestamp, Timestamp) {
    let published_at = wp_time(&item.date_gmt, &item.date_local);
    let created_at = published_at.unwrap_or_else(Timestamp::now);
    let updated_at = wp_time(&item.modified_gmt, &item.modified_local).unwrap_or(created_at);
    (published_at, created_at, updated_at)
}

/// The item's parent when its type is hierarchical and the parent, of the same type and
/// not trashed, is already written; else none (noted).
async fn parent(
    b: &mut ImportBatch<'_>,
    cx: &Context<'_>,
    item: &Item,
    lines: &mut Vec<String>,
) -> Result<Option<ContentId>, ImportError> {
    if item.parent_wp_id == 0 {
        return Ok(None);
    }
    let usable = cx.hierarchical.contains(&item.post_type)
        && cx
            .links
            .item(item.parent_wp_id)
            .is_some_and(|(t, s)| *t == item.post_type && s != "trash" && status(s).is_ok());
    let found = if usable {
        b.lookup(ImportMapKind::Content, item.parent_wp_id)
            .await?
            .map(|m| ContentId::from_uuid(m.id))
    } else {
        None
    };
    // On a dry run the parent's row was rolled back with its batch (noted once).
    if found.is_none() && cx.schema.is_none() {
        lines.push(format!(
            "item {}: its parent {} was not imported; imported without a parent",
            item.wp_id, item.parent_wp_id
        ));
    }
    Ok(found)
}

/// The item's terms through the import map, the primary category first.
async fn terms(
    b: &mut ImportBatch<'_>,
    cx: &Context<'_>,
    item: &Item,
    lines: &mut Vec<String>,
) -> Result<Vec<TermId>, ImportError> {
    let primary = PRIMARY_CATEGORY
        .iter()
        .find_map(|k| meta(item, k))
        .and_then(|v| v.trim().parse::<i64>().ok());
    let mut wp_ids: Vec<i64> = Vec::new();
    for t in &item.terms {
        if taxonomy_name(&t.taxonomy).is_none() {
            continue;
        }
        match cx.terms.get(&(t.taxonomy.clone(), percent_decode(&t.slug))) {
            Some(wp) => wp_ids.push(*wp),
            None => lines.push(format!(
                "item {}: term {}/{} is not in the file's term list; left out",
                item.wp_id, t.taxonomy, t.slug
            )),
        }
    }
    if let Some(p) = primary.filter(|p| wp_ids.contains(p)) {
        wp_ids.retain(|w| *w != p);
        wp_ids.insert(0, p);
    }
    Ok(mapped(b, ImportMapKind::Term, &wp_ids)
        .await?
        .into_iter()
        .map(|(_, id)| TermId::from_uuid(id))
        .collect())
}

/// Items per post type, in first-seen order of the types: (WP id, parent WP id).
async fn order(ir: &Ir) -> Result<Vec<(String, Vec<(i64, i64)>)>, ImportError> {
    let mut out: Vec<(String, Vec<(i64, i64)>)> = Vec::new();
    let mut items = ir.items(&[]);
    while let Some(item) = items.try_next().await? {
        if type_name(&item.post_type).is_none() {
            continue;
        }
        let node = (item.wp_id, item.parent_wp_id);
        match out.iter_mut().find(|(t, _)| *t == item.post_type) {
            Some((_, nodes)) => nodes.push(node),
            None => out.push((item.post_type.clone(), vec![node])),
        }
    }
    Ok(out)
}

/// The WP ids of `nodes` with each parent (when among them) before its children, in
/// file order otherwise; a cycle is cut where it closes.
pub(crate) fn parent_first(nodes: &[(i64, i64)]) -> Vec<i64> {
    let index: HashMap<i64, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let mut state = vec![0u8; nodes.len()]; // 0 new, 1 on the current path, 2 placed
    let mut out = Vec::with_capacity(nodes.len());
    for start in 0..nodes.len() {
        if state[start] != 0 {
            continue;
        }
        let mut path = Vec::new();
        let mut cur = start;
        loop {
            state[cur] = 1;
            path.push(cur);
            match index.get(&nodes[cur].1) {
                Some(&p) if state[p] == 0 => cur = p,
                _ => break,
            }
        }
        for i in path.into_iter().rev() {
            state[i] = 2;
            out.push(nodes[i].0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parents_first_and_cycles_cut() {
        assert_eq!(
            parent_first(&[(3, 2), (2, 1), (1, 0), (4, 0), (5, 99)]),
            [1, 2, 3, 4, 5]
        );
        assert_eq!(parent_first(&[(1, 2), (2, 1)]), [2, 1]);
    }

    /// "A draft" of site A has zero GMT dates: its dates are its local ones, as UTC.
    #[test]
    fn zero_gmt_dates_read_the_local_ones() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/site-a.xml");
        let bytes = std::fs::read(path).unwrap_or_default();
        let mut draft = None;
        crate::wxr::parse(&bytes[..], |r| {
            if let crate::model::Record::Item(i) = r
                && i.wp_id == 11
            {
                draft = Some(*i);
            }
            Ok(())
        })
        .unwrap_or_default();
        let Some(mut draft) = draft else {
            panic!("no item 11 in site A");
        };
        assert!(draft.modified_gmt.starts_with("0000-00-00"));
        let at = |s: &str| serde_json::from_value::<Timestamp>(json!(s)).ok();
        let (published, created, updated) = dates(&draft);
        assert_eq!(published, at("2026-10-28T10:49:55Z"));
        assert_eq!(Some(created), at("2026-10-28T10:49:55Z"));
        assert_eq!(Some(updated), at("2026-10-28T10:49:55Z"));
        // The modified date is the local one, not the creation date.
        draft.modified_local = "2026-11-01 08:00:00".into();
        assert_eq!(Some(dates(&draft).2), at("2026-11-01T08:00:00Z"));
    }

    #[test]
    fn statuses_and_dates() {
        assert_eq!(status("future"), Ok(ContentStatus::Scheduled));
        assert!(status("auto-draft").is_err());
        assert!(status("inherit").is_err());
        assert_eq!(ymd("20220505").as_deref(), Some("2022-05-05T00:00:00Z"));
        assert_eq!(ymd("20221305"), None);
        assert_eq!(ymd("2022-05-05"), None);
    }
}
