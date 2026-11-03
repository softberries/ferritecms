//! Media facts (design 16 §12): whether each media item an item shows exists, is public
//! and is an image with a blank alt. Nothing else is read or returned, so a check never
//! tells a caller more about media than that.

use std::collections::HashMap;

use ferrite_domain::{ContentId, MEDIA_KIND, Timestamp};
use ferrite_store::{MAX_ID_FILTER, Repos};
use serde_json::json;

use super::{Finding, MAX_ITEM_FINDINGS, MediaRef, rules::known};
use crate::{CoreError, site::is_public};

/// What is known of one media id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaFact {
    /// A media item that the public site shows (not trashed).
    pub public: bool,
    /// Its type is `image/…`.
    pub image: bool,
    /// Its own alt text is blank (themes use it for the featured image).
    pub alt_blank: bool,
}

/// The facts of the media ids an item references; an id absent here names no media item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaFacts {
    facts: HashMap<ContentId, MediaFact>,
}

impl MediaFacts {
    pub fn get(&self, id: ContentId) -> Option<MediaFact> {
        self.facts.get(&id).copied()
    }
}

/// Loads the facts of every media id in `refs` (one item read and one media read per
/// [`MAX_ID_FILTER`] ids).
pub async fn load(repos: &dyn Repos, refs: &[MediaRef]) -> Result<MediaFacts, CoreError> {
    let mut ids: Vec<ContentId> = refs.iter().map(|r| r.id).collect();
    ids.sort();
    ids.dedup();
    let now = Timestamp::now();
    let mut facts = HashMap::new();
    for chunk in ids.chunks(MAX_ID_FILTER) {
        let mut public = Vec::new();
        for item in repos.content().get_many(chunk).await? {
            if item.kind != MEDIA_KIND {
                continue;
            }
            let shown = is_public(&item, now);
            facts.insert(
                item.id,
                MediaFact {
                    public: false,
                    image: false,
                    alt_blank: false,
                },
            );
            if shown {
                public.push(item.id);
            }
        }
        for m in repos.media().get_many(&public).await? {
            facts.insert(
                m.item_id,
                MediaFact {
                    public: true,
                    image: m.mime.starts_with("image/"),
                    alt_blank: super::clean(&m.alt).is_empty(),
                },
            );
        }
    }
    Ok(MediaFacts { facts })
}

/// Adds `media-missing` (no such media item, or not public) and `featured-alt-missing`
/// (a public featured image whose own alt is blank) for `refs` to `findings`, within the
/// item's finding cap.
pub fn apply(facts: &MediaFacts, findings: &mut Vec<Finding>, refs: &[MediaRef]) {
    for r in refs {
        if findings.len() >= MAX_ITEM_FINDINGS {
            return;
        }
        let fact = facts.get(r.id);
        let (rule, reason) = match fact {
            None => ("media-missing", Some("missing")),
            Some(f) if !f.public => ("media-missing", Some("not_public")),
            Some(f) if r.featured && f.image && f.alt_blank => ("featured-alt-missing", None),
            Some(_) => continue,
        };
        let mut detail = json!({ "id": r.id.to_string() });
        if let (Some(reason), Some(obj)) = (reason, detail.as_object_mut()) {
            obj.insert("reason".into(), reason.into());
        }
        findings.push(Finding {
            rule: known(rule),
            location: r.location.clone(),
            key: r.key.clone(),
            context: None,
            detail,
        });
    }
}
