//! Snapshots of objects (design 17 §4): typed, serialised to canonical JSON, hashed and
//! compressed into an [`Image`].

use ferrite_domain::{
    CommentId, CommentStatus, ContentId, ContentItem, ContentQuery, ContentStatus, ContentType,
    Image, RedirectOrigin, TermId, Timestamp, UserId,
};
use ferrite_store::Repos;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::object::ObjectRef;
use crate::{CoreError, TRASHED_FROM_META, content::Seo};

/// The largest canonical image kept (16 MiB); a bigger one is stored skipped (hash only).
pub const MAX_IMAGE: usize = 16 * 1024 * 1024;
/// At most this many linked item ids are kept in a term's image.
pub(crate) const MAX_TERM_ITEMS: usize = 1_000;
/// zstd level of the images.
const LEVEL: i32 = 3;
/// Items read per page while collecting a term's links.
const TERM_PAGE: u32 = 200;

/// A content item's (or media item's) editable state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ContentSnap {
    pub kind: String,
    pub locale: String,
    pub translation_group: ContentId,
    pub status: ContentStatus,
    pub slug: String,
    pub parent_id: Option<ContentId>,
    pub author_id: UserId,
    pub title: String,
    pub excerpt: String,
    pub body: Value,
    pub fields: Value,
    pub menu_order: i32,
    pub published_at: Option<Timestamp>,
    pub scheduled_at: Option<Timestamp>,
    /// Sorted.
    pub terms: Vec<TermId>,
    pub seo: SeoSnap,
    /// Media only: the `media` row's alt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    /// Media only: the status before a media trash (meta `TRASHED_FROM_META`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed_from: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SeoSnap {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical: Option<String>,
    pub robots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TermSnap {
    pub taxonomy: String,
    pub locale: String,
    pub translation_group: TermId,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent_id: Option<TermId>,
    /// Linked item ids, sorted, at most [`MAX_TERM_ITEMS`].
    pub items: Vec<ContentId>,
    pub items_total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TypeSnap {
    pub label: String,
    pub hierarchical: bool,
    pub has_archive: bool,
    pub fields: Value,
    pub built_in: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RedirectSnap {
    pub source_path: String,
    pub target: String,
    pub code: u16,
    pub origin: RedirectOrigin,
}

/// No body, name or e-mail: personal data stays in `comments` (design 17 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CommentSnap {
    pub item_id: ContentId,
    pub parent_id: Option<CommentId>,
    pub status: CommentStatus,
    pub user_id: Option<UserId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OptionSnap {
    /// As stored: a plugin secret stays sealed.
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ThemeSnap {
    pub name: String,
    pub artifact: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PluginSnap {
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub version: String,
    pub artifact: String,
}

fn to_value<T: Serialize>(snap: &T) -> Result<Value, CoreError> {
    serde_json::to_value(snap).map_err(|_| CoreError::Internal("snapshot not serialisable"))
}

/// Reads the snapshot of `object` through `repos` (in the write's `Tx`); `None` when it
/// does not exist.
pub(crate) async fn snapshot(
    repos: &dyn Repos,
    object: &ObjectRef,
) -> Result<Option<Value>, CoreError> {
    Ok(match object {
        ObjectRef::Content(id) => match repos.content().get(*id).await? {
            Some(item) => Some(to_value(&content_snap(repos, item, false).await?)?),
            None => None,
        },
        ObjectRef::Media(id) => match repos.content().get(*id).await? {
            Some(item) => Some(to_value(&content_snap(repos, item, true).await?)?),
            None => None,
        },
        ObjectRef::Term(id) => match repos.terms().get(*id).await? {
            Some(t) => {
                let (items, items_total) = term_items(repos, *id).await?;
                Some(to_value(&TermSnap {
                    taxonomy: t.taxonomy,
                    locale: t.locale,
                    translation_group: t.translation_group,
                    slug: t.slug,
                    name: t.name,
                    description: t.description,
                    parent_id: t.parent_id,
                    items,
                    items_total,
                })?)
            }
            None => None,
        },
        ObjectRef::Menu(name) => match repos.menus().get(name).await? {
            Some(menu) => Some(to_value(&menu)?),
            None => None,
        },
        ObjectRef::Type(name) => match repos.types().get(name).await? {
            Some(t) => Some(to_value(&type_snap(t))?),
            None => None,
        },
        ObjectRef::Redirect(id) => match repos.redirects().get(*id).await? {
            Some(r) => Some(to_value(&RedirectSnap {
                source_path: r.source_path,
                target: r.target,
                code: r.code,
                origin: r.origin,
            })?),
            None => None,
        },
        ObjectRef::Comment(id) => match repos.comments().get(*id).await? {
            Some(c) => Some(to_value(&CommentSnap {
                item_id: c.item_id,
                parent_id: c.parent_id,
                status: c.status,
                user_id: c.user_id,
            })?),
            None => None,
        },
        ObjectRef::Option(key) => match repos.options().get(key).await? {
            Some(o) => Some(to_value(&OptionSnap { value: o.value })?),
            None => None,
        },
        ObjectRef::ActiveTheme => match repos.themes().active().await? {
            Some(t) => Some(to_value(&ThemeSnap {
                name: t.name,
                artifact: t.artifact_hash.as_str().to_owned(),
            })?),
            None => None,
        },
        ObjectRef::Plugin(name) => match repos.plugins().get(name).await? {
            Some(p) => Some(to_value(&PluginSnap {
                enabled: p.enabled,
                disabled_reason: p.disabled_reason,
                version: p.version,
                artifact: p.artifact_hash.as_str().to_owned(),
            })?),
            None => None,
        },
    })
}

pub(crate) fn type_snap(t: ContentType) -> TypeSnap {
    TypeSnap {
        label: t.label,
        hierarchical: t.hierarchical,
        has_archive: t.has_archive,
        fields: t.fields,
        built_in: t.built_in,
    }
}

async fn content_snap(
    repos: &dyn Repos,
    item: ContentItem,
    media: bool,
) -> Result<ContentSnap, CoreError> {
    let mut terms: Vec<TermId> = repos
        .terms()
        .for_item(item.id)
        .await?
        .into_iter()
        .map(|t| t.id)
        .collect();
    terms.sort();
    let seo = Seo::load(repos, item.id).await?;
    let (alt, trashed_from) = if media {
        let alt = repos.media().get(item.id).await?.map(|m| m.alt);
        let from = match repos.meta().get(item.id, TRASHED_FROM_META).await? {
            Some(Value::String(s)) => Some(s),
            _ => None,
        };
        (Some(alt.unwrap_or_default()), from)
    } else {
        (None, None)
    };
    Ok(ContentSnap {
        kind: item.kind,
        locale: item.locale,
        translation_group: item.translation_group,
        status: item.status,
        slug: item.slug,
        parent_id: item.parent_id,
        author_id: item.author_id,
        title: item.title,
        excerpt: item.excerpt,
        body: item.body,
        fields: item.fields,
        menu_order: item.menu_order,
        published_at: item.published_at,
        scheduled_at: item.scheduled_at,
        terms,
        seo: SeoSnap {
            title: seo.title,
            description: seo.description,
            canonical: seo.canonical,
            robots: seo.robots.iter().map(|r| r.as_str().to_owned()).collect(),
        },
        alt,
        trashed_from,
    })
}

/// The items linked to term `id` (sorted, at most [`MAX_TERM_ITEMS`]) and their count.
async fn term_items(repos: &dyn Repos, id: TermId) -> Result<(Vec<ContentId>, u64), CoreError> {
    let query = ContentQuery {
        term: Some(id),
        limit: TERM_PAGE,
        ..ContentQuery::default()
    };
    let total = repos.content().count(&query).await?;
    let mut items = Vec::new();
    let mut cursor = None;
    loop {
        let page = repos
            .content()
            .list(&ContentQuery {
                cursor,
                ..query.clone()
            })
            .await?;
        items.extend(page.items.into_iter().map(|i| i.id));
        cursor = page.next_cursor;
        if cursor.is_none() || items.len() >= MAX_TERM_ITEMS {
            break;
        }
    }
    items.truncate(MAX_TERM_ITEMS);
    items.sort();
    Ok((items, total))
}

/// The canonical bytes of `value`: object keys sorted recursively, no whitespace, so
/// Postgres `jsonb` (which reorders keys) and SQLite `TEXT` give the same bytes.
pub fn canonical(value: &Value) -> Vec<u8> {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&map[k]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    // `serde_json::Map` keeps insertion order only with `preserve_order`; sorting the
    // keys first makes the output independent of that feature.
    serde_json::to_vec(&sorted(value)).unwrap_or_default()
}

/// SHA-256 hex of the canonical bytes.
pub(crate) fn hash_of(bytes: &[u8]) -> String {
    ferrite_domain::to_hex(&Sha256::digest(bytes))
}

/// The image of a snapshot: absent, present (zstd) or skipped above [`MAX_IMAGE`].
pub(crate) fn image(snapshot: Option<&Value>) -> Result<Image, CoreError> {
    let Some(v) = snapshot else {
        return Ok(Image::Absent);
    };
    let bytes = canonical(v);
    let hash = hash_of(&bytes);
    if bytes.len() > MAX_IMAGE {
        return Ok(Image::Skipped { hash });
    }
    let packed = zstd::bulk::compress(&bytes, LEVEL)
        .map_err(|_| CoreError::Internal("change image not compressible"))?;
    Ok(Image::Present {
        hash,
        bytes: packed,
    })
}

/// The hash a current snapshot would have (`None` when absent).
pub(crate) fn hash_snapshot(snapshot: Option<&Value>) -> Option<String> {
    snapshot.map(|v| hash_of(&canonical(v)))
}

/// Opens an image: `Ok(None)` when absent, `Unavailable` when skipped; decompression is
/// capped at [`MAX_IMAGE`].
pub fn open_image(image: &Image) -> Result<Option<Value>, CoreError> {
    match image {
        Image::Absent => Ok(None),
        Image::Skipped { .. } => Err(CoreError::Unavailable("change image too large to keep")),
        Image::Present { bytes, .. } => {
            let raw = zstd::bulk::decompress(bytes, MAX_IMAGE)
                .map_err(|_| CoreError::Internal("change image unreadable"))?;
            serde_json::from_slice(&raw)
                .map(Some)
                .map_err(|_| CoreError::Internal("change image unreadable"))
        }
    }
}

/// The label of an image: its title, name, source path or key; at most 200 characters.
pub(crate) fn label_of(value: &Value) -> String {
    let text = ["title", "name", "label", "source_path"]
        .iter()
        .find_map(|k| value.get(*k).and_then(Value::as_str))
        .unwrap_or("");
    text.chars().take(200).collect()
}

/// Whether `value` (an image of `kind`) is a content or media snapshot in the trash.
pub(crate) fn is_trashed(value: &Value) -> bool {
    value.get("status").and_then(Value::as_str) == Some(ContentStatus::Trash.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_sorts_keys_recursively() {
        let a = json!({"b": 1, "a": {"z": [ {"y": 1, "x": 2} ], "c": null}});
        let b: Value =
            serde_json::from_str(r#"{"a":{"c":null,"z":[{"x":2,"y":1}]},"b":1}"#).unwrap();
        assert_eq!(canonical(&a), canonical(&b));
        assert_eq!(
            String::from_utf8(canonical(&a)).unwrap(),
            r#"{"a":{"c":null,"z":[{"x":2,"y":1}]},"b":1}"#
        );
        assert_eq!(hash_snapshot(Some(&a)), hash_snapshot(Some(&b)));
    }

    #[test]
    fn image_roundtrips_and_hashes_the_canonical_bytes() {
        let v = json!({"title": "Hello", "body": {"t": "doc"}});
        let img = image(Some(&v)).unwrap();
        assert_eq!(img.hash(), hash_snapshot(Some(&v)).as_deref());
        assert_eq!(open_image(&img).unwrap(), Some(v.clone()));
        assert_eq!(image(None).unwrap(), Image::Absent);
        assert_eq!(open_image(&Image::Absent).unwrap(), None);
        assert_eq!(label_of(&v), "Hello");
        assert_eq!(label_of(&json!({"name": "x".repeat(300)})).len(), 200);
    }

    #[test]
    fn snapshot_leaves_out_volatile_columns() {
        let snap = ContentSnap {
            kind: "post".into(),
            locale: "en".into(),
            translation_group: ContentId::new(),
            status: ContentStatus::Draft,
            slug: "a".into(),
            parent_id: None,
            author_id: UserId::new(),
            title: "A".into(),
            excerpt: String::new(),
            body: json!({}),
            fields: json!({}),
            menu_order: 0,
            published_at: None,
            scheduled_at: None,
            terms: vec![],
            seo: SeoSnap::default(),
            alt: None,
            trashed_from: None,
        };
        let v = to_value(&snap).unwrap();
        for k in [
            "version",
            "updated_at",
            "created_at",
            "body_html",
            "id",
            "alt",
        ] {
            assert!(v.get(k).is_none(), "{k} in {v}");
        }
        let r = to_value(&RedirectSnap {
            source_path: "/a".into(),
            target: "/b".into(),
            code: 301,
            origin: RedirectOrigin::Manual,
        })
        .unwrap();
        assert!(r.get("hits").is_none() && r.get("last_hit_at").is_none());
    }

    #[test]
    fn oversize_image_is_skipped_with_hash() {
        let v = json!({"body": "x".repeat(MAX_IMAGE + 1)});
        let img = image(Some(&v)).unwrap();
        assert!(
            matches!(&img, Image::Skipped { hash } if *hash == hash_snapshot(Some(&v)).unwrap())
        );
        assert!(open_image(&img).is_err());
    }

    #[test]
    fn decompress_refuses_beyond_max_image() {
        // A zstd bomb: a small frame expanding past the cap.
        let huge = vec![b'a'; MAX_IMAGE + 1024];
        let bomb = zstd::bulk::compress(&huge, 19).unwrap();
        assert!(bomb.len() < 64 * 1024);
        let img = Image::Present {
            hash: "x".into(),
            bytes: bomb,
        };
        assert!(matches!(open_image(&img), Err(CoreError::Internal(_))));
    }
}
