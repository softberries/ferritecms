//! Turns content rows into [`PageItem`]s with batch loads only: one query per
//! repository per batch of up to 200 ids, never one per item.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    num::NonZeroU32,
};

use ferrite_domain::{ArtifactHash, ContentId, ContentItem, Media, Term, Timestamp};
use ferrite_store::{MAX_ID_FILTER, Repos};
use serde_json::Value;

use super::{
    MEDIA_KIND, MediaUrlFn, is_public,
    model::{
        DepKeys, MediaPublic, MediaVariant, PageItem, PublicItem, TermPublic, UserPublic, srcset,
    },
};
use crate::{
    CoreError, MAX_PARENT_DEPTH,
    blocks::{self, InlineImage, RenderCtx, Responsive},
    content::EXCERPT_CHARS,
    events::{post_key, term_key, user_key},
    media::RASTER_MIMES,
    permalink::{PermalinkInput, Permalinks, primary_term, term_path},
};

/// Field holding the featured image's media item id (a UUID string).
const FEATURED_FIELD: &str = "featured_image";
/// Most UUID strings per item's fields checked against content; any beyond are
/// stripped unchecked.
const MAX_FIELD_REFS: usize = 50;
/// Field values nested deeper than this are dropped.
const MAX_FIELD_DEPTH: usize = 64;

/// Content items already known by id, as (parent, slug): seeds the ancestor walk.
pub(super) type Nodes = HashMap<ContentId, (Option<ContentId>, String)>;

/// Adds every missing ancestor of `items` to `nodes`, one level per query.
pub(super) async fn load_ancestors(
    repos: &dyn Repos,
    items: &[ContentItem],
    nodes: &mut Nodes,
) -> Result<(), CoreError> {
    for i in items {
        nodes
            .entry(i.id)
            .or_insert_with(|| (i.parent_id, i.slug.clone()));
    }
    let mut missing: Vec<ContentId> = unique(
        items
            .iter()
            .filter_map(|i| i.parent_id)
            .filter(|p| !nodes.contains_key(p)),
    );
    for _ in 0..MAX_PARENT_DEPTH {
        if missing.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for chunk in missing.chunks(MAX_ID_FILTER) {
            for p in repos.content().get_many(chunk).await? {
                if let Some(gp) = p.parent_id
                    && !nodes.contains_key(&gp)
                {
                    next.push(gp);
                }
                nodes.insert(p.id, (p.parent_id, p.slug));
            }
        }
        // A row read in this level can already be the grandparent of another one.
        next.retain(|id| !nodes.contains_key(id));
        missing = unique(next);
    }
    Ok(())
}

pub(super) struct Hydrator<'a> {
    pub repos: &'a dyn Repos,
    pub permalinks: &'a dyn Permalinks,
    pub media_url: &'a MediaUrlFn,
    /// The served theme's widest image size ([`SiteReader::set_image_width`](super::SiteReader::set_image_width)).
    pub image_width: Option<NonZeroU32>,
    pub now: Timestamp,
}

impl Hydrator<'_> {
    /// Loads terms, authors, ancestors (for URLs) and published media for `items` and
    /// builds their page items, recording `post:`/`term:` keys for everything shown.
    ///
    /// Store calls: none for no items; otherwise terms, users, then one `get_many` per
    /// ancestor level not already in `nodes` (none for flat types), and, only if any
    /// item references an id (media in the body, UUID strings in `fields`), the
    /// referenced content items plus the media rows of the public media among them.
    /// Each is one call per 200 ids.
    pub async fn hydrate(
        &self,
        items: Vec<ContentItem>,
        mut nodes: Nodes,
        deps: &mut DepKeys,
    ) -> Result<Vec<PageItem>, CoreError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<ContentId> = unique(items.iter().map(|i| i.id));
        let mut terms: HashMap<ContentId, Vec<Term>> = HashMap::new();
        for chunk in ids.chunks(MAX_ID_FILTER) {
            terms.extend(self.repos.terms().for_items(chunk).await?);
        }
        let mut authors = HashMap::new();
        for chunk in unique(items.iter().map(|i| i.author_id)).chunks(MAX_ID_FILTER) {
            for u in self.repos.users().get_many(chunk).await? {
                authors.insert(
                    u.id,
                    UserPublic {
                        id: u.id,
                        login: u.login,
                        display_name: u.display_name,
                    },
                );
            }
        }
        load_ancestors(self.repos, &items, &mut nodes).await?;
        let refs: Vec<MediaRefs> = items.iter().map(MediaRefs::of).collect();
        let refd = self
            .load_refs(unique(refs.iter().flat_map(MediaRefs::all)))
            .await?;
        let media = &refd.media;

        let mut out = Vec::with_capacity(items.len());
        for (item, refs) in items.into_iter().zip(refs) {
            deps.insert(post_key(item.id));
            for id in refs.all() {
                // Hidden references count too: publishing one must refresh this page.
                if refd.existing.contains(&id) {
                    deps.insert(post_key(id));
                }
            }
            if authors.contains_key(&item.author_id) {
                deps.insert(user_key(item.author_id));
            }
            let item_terms = terms.remove(&item.id).unwrap_or_default();
            for t in &item_terms {
                deps.insert(term_key(t.id));
            }
            let ancestors = chain(&nodes, item.parent_id);
            for (id, _) in &ancestors {
                deps.insert(post_key(*id));
            }
            let input = PermalinkInput {
                item,
                ancestors,
                primary_term: primary_term(item_terms.clone()),
            };
            let url = self.permalinks.path(&input);
            let item = input.item;
            let featured = refs.featured.and_then(|id| media.get(&id));
            let fields = public_fields(&item.fields, &refd, featured.is_some());
            out.push(PageItem {
                url,
                html: self.render(&item.body, media),
                author: authors.get(&item.author_id).cloned(),
                terms: item_terms.into_iter().map(term_public).collect(),
                featured: featured.map(|m| self.media_public(m)),
                item: PublicItem {
                    excerpt: excerpt(&item),
                    id: item.id,
                    kind: item.kind,
                    slug: item.slug,
                    title: item.title,
                    published_at: item.published_at,
                    updated_at: item.updated_at,
                    menu_order: item.menu_order,
                    parent_id: item.parent_id,
                    fields,
                },
            });
        }
        Ok(out)
    }

    /// Looks up the referenced `ids`: which are non-public content items, and the
    /// media rows of the public media items.
    async fn load_refs(&self, ids: Vec<ContentId>) -> Result<Refs, CoreError> {
        let mut out = Refs {
            checked: ids.iter().copied().collect(),
            existing: HashSet::new(),
            hidden: HashSet::new(),
            media: HashMap::new(),
        };
        if ids.is_empty() {
            return Ok(out);
        }
        let mut public_media = Vec::new();
        for chunk in ids.chunks(MAX_ID_FILTER) {
            for item in self.repos.content().get_many(chunk).await? {
                out.existing.insert(item.id);
                if !is_public(&item, self.now) {
                    out.hidden.insert(item.id);
                } else if item.kind == MEDIA_KIND {
                    public_media.push(item.id);
                }
            }
        }
        for chunk in public_media.chunks(MAX_ID_FILTER) {
            for m in self.repos.media().get_many(chunk).await? {
                out.media.insert(m.item_id, m);
            }
        }
        Ok(out)
    }

    fn render(&self, body: &Value, media: &HashMap<ContentId, Media>) -> String {
        let image = |id: &str| {
            let id: ContentId = id.parse().ok()?;
            media.get(&id).map(|m| self.inline_image(m))
        };
        blocks::render_html(body, &RenderCtx { media: &image })
    }

    /// An inline image of public media `m` (06 §4, the R28 follow-up), as the theme's
    /// `image()` helper renders one: for a raster image whose dimensions are recorded,
    /// `srcset` lists the original and every variant of the same shape (a crop to
    /// another aspect ratio would show a different picture; [`same_shape`]), `src` is
    /// the widest of them no wider than the display slot (the theme's largest size, or
    /// the original's width when that is smaller or the theme has no sizes; the original
    /// when none fits), and `sizes` says the image is at most that wide. Anything else (SVG, other types, an image the
    /// `media.variants` job has not measured yet) is a plain `<img>` of the original.
    fn inline_image(&self, m: &Media) -> InlineImage {
        let public = self.media_public(m);
        let (Some(width), Some(height)) = (m.width, m.height) else {
            return InlineImage::plain(public.url);
        };
        if !RASTER_MIMES.contains(&m.mime.as_str()) || width <= 0 || height <= 0 {
            return InlineImage::plain(public.url);
        }
        let mut shaped = public.clone();
        shaped
            .variants
            .retain(|v| same_shape((width, height), v.width.zip(v.height)));
        let candidates = shaped.srcset_candidates(None);
        // The display slot: the theme's largest size, but never wider than the image.
        let largest = self
            .image_width
            .and_then(|w| i32::try_from(w.get()).ok())
            .map_or(width, |l| l.min(width));
        let src = candidates
            .iter()
            .rev()
            .find(|(w, _)| *w <= largest)
            .map_or(public.url.as_str(), |(_, url)| url);
        InlineImage {
            src: src.to_owned(),
            responsive: Some(Responsive {
                srcset: srcset(&candidates),
                sizes: format!("(max-width: {largest}px) 100vw, {largest}px"),
                width,
                height,
            }),
        }
    }

    pub(super) fn media_public(&self, m: &Media) -> MediaPublic {
        let mut variants: Vec<MediaVariant> = m
            .variants
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(name, v)| {
                let hash = ArtifactHash::parse(v.get("hash")?.as_str()?).ok()?;
                Some(MediaVariant {
                    name: name.clone(),
                    url: (self.media_url)(&hash, &m.original_name),
                    width: dimension(v.get("w")),
                    height: dimension(v.get("h")),
                })
            })
            .collect();
        variants.sort_by(|a, b| a.name.cmp(&b.name));
        MediaPublic {
            id: m.item_id,
            url: (self.media_url)(&m.artifact_hash, &m.original_name),
            alt: m.alt.clone(),
            width: m.width,
            height: m.height,
            variants,
        }
    }
}

/// The ids an item references: its featured image, its inline images and the UUID
/// strings in its fields.
struct MediaRefs {
    featured: Option<ContentId>,
    inline: Vec<ContentId>,
    fields: Vec<ContentId>,
}

/// What the referenced ids turned out to be.
struct Refs {
    /// Every id looked up.
    checked: HashSet<ContentId>,
    /// Ids of existing content items, public or not.
    existing: HashSet<ContentId>,
    /// Ids of existing content items that are not public.
    hidden: HashSet<ContentId>,
    /// Public media items with their rows.
    media: HashMap<ContentId, Media>,
}

impl MediaRefs {
    fn of(item: &ContentItem) -> Self {
        let featured = item
            .fields
            .get(FEATURED_FIELD)
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok());
        // Rendering once with a recording resolver finds exactly the references the
        // real render will resolve. (A single pass is not possible: the URLs, or
        // whether the image is dropped, depend on this lookup.)
        let found = RefCell::new(Vec::new());
        let record = |id: &str| {
            if let Ok(id) = id.parse::<ContentId>() {
                found.borrow_mut().push(id);
            }
            None
        };
        blocks::render_html(&item.body, &RenderCtx { media: &record });
        let mut fields = Vec::new();
        field_ids(&item.fields, 0, &mut fields);
        Self {
            featured,
            inline: found.into_inner(),
            fields,
        }
    }

    fn all(&self) -> impl Iterator<Item = ContentId> + '_ {
        self.featured
            .into_iter()
            .chain(self.inline.iter().copied())
            .chain(self.fields.iter().copied())
    }
}

/// Collects up to [`MAX_FIELD_REFS`] UUID strings from a fields value.
fn field_ids(v: &Value, depth: usize, out: &mut Vec<ContentId>) {
    if depth > MAX_FIELD_DEPTH || out.len() >= MAX_FIELD_REFS {
        return;
    }
    match v {
        Value::String(s) => {
            if let Ok(id) = s.parse() {
                out.push(id);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| field_ids(x, depth + 1, out)),
        Value::Object(o) => o.values().for_each(|x| field_ids(x, depth + 1, out)),
        _ => {}
    }
}

/// The fields a template may see: `featured_image` only if it resolved to public
/// media, and no UUID string that names a non-public content item or was not checked
/// (beyond [`MAX_FIELD_REFS`]). Values nested deeper than [`MAX_FIELD_DEPTH`] are
/// dropped.
fn public_fields(fields: &Value, refs: &Refs, featured_ok: bool) -> Value {
    let mut out = filter_value(fields, refs, 0).unwrap_or(Value::Null);
    if !featured_ok && let Some(o) = out.as_object_mut() {
        o.remove(FEATURED_FIELD);
    }
    out
}

fn filter_value(v: &Value, refs: &Refs, depth: usize) -> Option<Value> {
    if depth > MAX_FIELD_DEPTH {
        return None;
    }
    match v {
        Value::String(s) => match s.parse::<ContentId>() {
            Ok(id) if !refs.checked.contains(&id) || refs.hidden.contains(&id) => None,
            _ => Some(v.clone()),
        },
        Value::Array(a) => Some(Value::Array(
            a.iter()
                .filter_map(|x| filter_value(x, refs, depth + 1))
                .collect(),
        )),
        Value::Object(o) => Some(Value::Object(
            o.iter()
                .filter_map(|(k, x)| Some((k.clone(), filter_value(x, refs, depth + 1)?)))
                .collect(),
        )),
        other => Some(other.clone()),
    }
}

/// Ancestors of an item with parent `parent`, root first, as far as `nodes` knows them.
pub(super) fn chain(nodes: &Nodes, mut parent: Option<ContentId>) -> Vec<(ContentId, String)> {
    let mut out = Vec::new();
    while let Some(id) = parent {
        let Some((up, slug)) = nodes.get(&id) else {
            tracing::warn!(%id, "ancestor missing while building a permalink");
            break;
        };
        if out.len() >= MAX_PARENT_DEPTH {
            break;
        }
        out.push((id, slug.clone()));
        parent = *up;
    }
    out.reverse();
    out
}

fn excerpt(item: &ContentItem) -> String {
    if item.excerpt.trim().is_empty() {
        blocks::excerpt(&item.body, EXCERPT_CHARS)
    } else {
        item.excerpt.clone()
    }
}

pub(super) fn term_public(t: Term) -> TermPublic {
    TermPublic {
        url: term_path(&t.taxonomy, &t.slug),
        id: t.id,
        taxonomy: t.taxonomy,
        slug: t.slug,
        name: t.name,
    }
}

/// Whether a variant of `size` has the aspect ratio of an original of `original`, up to
/// the rounding of a resize (`|w·oh − h·ow| ≤ max(ow, oh)`); a variant whose size is
/// unknown does not.
fn same_shape(original: (i32, i32), size: Option<(i32, i32)>) -> bool {
    let Some((w, h)) = size else {
        return false;
    };
    let (ow, oh) = (i64::from(original.0), i64::from(original.1));
    let (w, h) = (i64::from(w), i64::from(h));
    w > 0 && h > 0 && (w * oh - h * ow).abs() <= ow.max(oh)
}

fn dimension(v: Option<&Value>) -> Option<i32> {
    v?.as_i64().and_then(|n| i32::try_from(n).ok())
}

/// Distinct values in first-seen order.
pub(super) fn unique<T: Copy + Eq + std::hash::Hash>(it: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut seen = HashSet::new();
    it.into_iter().filter(|v| seen.insert(*v)).collect()
}

#[cfg(test)]
mod tests {
    use super::same_shape;

    #[test]
    fn same_shape_allows_rounding_only() {
        // A 4:3 original: resizes (rounded either way) match, crops do not.
        assert!(same_shape((2000, 1500), Some((768, 576))));
        assert!(same_shape((2000, 1500), Some((1001, 751))));
        assert!(same_shape((1999, 1333), Some((768, 512))));
        assert!(!same_shape((2000, 1500), Some((1280, 640))));
        assert!(!same_shape((2000, 1500), Some((150, 150))));
        assert!(!same_shape((2000, 1500), Some((640, 360))));
        assert!(!same_shape((2000, 1500), None));
        assert!(!same_shape((2000, 1500), Some((0, 0))));
    }
}
