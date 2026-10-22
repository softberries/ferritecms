//! Turns content rows into [`PageItem`]s with batch loads only: one query per
//! repository per batch of up to 200 ids, never one per item.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use ferrite_domain::{ArtifactHash, ContentId, ContentItem, Media, Term, Timestamp};
use ferrite_store::{MAX_ID_FILTER, Repos};
use serde_json::Value;

use super::{
    MEDIA_KIND, MediaUrlFn, is_public,
    model::{DepKeys, MediaPublic, MediaVariant, PageItem, PublicItem, TermPublic, UserPublic},
};
use crate::{
    CoreError, MAX_PARENT_DEPTH,
    blocks::{self, RenderCtx},
    content::EXCERPT_CHARS,
    events::{post_key, term_key},
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

pub(super) struct Hydrator<'a> {
    pub repos: &'a dyn Repos,
    pub permalinks: &'a dyn Permalinks,
    pub media_url: &'a MediaUrlFn,
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
        self.load_ancestors(&items, &mut nodes).await?;
        let refs: Vec<MediaRefs> = items.iter().map(MediaRefs::of).collect();
        let refd = self
            .load_refs(unique(refs.iter().flat_map(MediaRefs::all)))
            .await?;
        let media = &refd.media;

        let mut out = Vec::with_capacity(items.len());
        for (item, refs) in items.into_iter().zip(refs) {
            deps.insert(post_key(item.id));
            for id in refs.all() {
                if media.contains_key(&id) {
                    deps.insert(post_key(id));
                }
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

    /// Adds every missing ancestor of `items` to `nodes`, one level per query.
    pub async fn load_ancestors(
        &self,
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
                for p in self.repos.content().get_many(chunk).await? {
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

    /// Looks up the referenced `ids`: which are non-public content items, and the
    /// media rows of the public media items.
    async fn load_refs(&self, ids: Vec<ContentId>) -> Result<Refs, CoreError> {
        let mut out = Refs {
            checked: ids.iter().copied().collect(),
            hidden: HashSet::new(),
            media: HashMap::new(),
        };
        if ids.is_empty() {
            return Ok(out);
        }
        let mut public_media = Vec::new();
        for chunk in ids.chunks(MAX_ID_FILTER) {
            for item in self.repos.content().get_many(chunk).await? {
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
        let url = |id: &str| {
            let id: ContentId = id.parse().ok()?;
            media
                .get(&id)
                .map(|m| (self.media_url)(&m.artifact_hash, &m.original_name))
        };
        blocks::render_html(body, &RenderCtx { media_url: &url })
    }

    fn media_public(&self, m: &Media) -> MediaPublic {
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
        blocks::render_html(&item.body, &RenderCtx { media_url: &record });
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

fn term_public(t: Term) -> TermPublic {
    TermPublic {
        url: term_path(&t.taxonomy, &t.slug),
        id: t.id,
        taxonomy: t.taxonomy,
        slug: t.slug,
        name: t.name,
    }
}

fn dimension(v: Option<&Value>) -> Option<i32> {
    v?.as_i64().and_then(|n| i32::try_from(n).ok())
}

/// Distinct values in first-seen order.
pub(super) fn unique<T: Copy + Eq + std::hash::Hash>(it: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut seen = HashSet::new();
    it.into_iter().filter(|v| seen.insert(*v)).collect()
}
