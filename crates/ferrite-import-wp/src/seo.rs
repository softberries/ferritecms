//! Yoast and Rank Math SEO meta → an item's SEO fields (P10).
//!
//! Keys (verified in the plugins' source and exports A and B):
//! - Yoast: `_yoast_wpseo_title`, `_yoast_wpseo_metadesc`, `_yoast_wpseo_canonical`,
//!   `_yoast_wpseo_meta-robots-noindex` (`1` noindex, `2` index, `0` the type's default),
//!   `_yoast_wpseo_meta-robots-nofollow` (`1` nofollow), `_yoast_wpseo_meta-robots-adv` (a
//!   comma list).
//! - Rank Math: `rank_math_title`, `rank_math_description`, `rank_math_canonical_url`,
//!   `rank_math_robots` (a PHP-serialised array).
//!
//! Title and description variables are substituted: Yoast `%%title%%`, `%%sitename%%`,
//! `%%sep%%`, Rank Math `%title%`, `%sitename%`, `%sep%`, with `-` (the plugins' default
//! separator) for the separator. Any other variable is removed and noted. A canonical
//! equal to the item's old `<link>` is dropped. A value core would refuse is dropped and
//! noted, so one bad value never fails the item.

use std::collections::BTreeSet;

use ferrite_core::content::seo::{MAX_SEO_DESCRIPTION, MAX_SEO_TITLE};
use ferrite_core::content::{Robots, SeoPatch};

use crate::model::{Channel, Item};
use crate::pass::meta;
use crate::php::unserialize;

/// The SEO plugins' meta keys this module reads; the content pass keeps them out of
/// `wp.*` meta.
pub const SEO_KEYS: [&str; 10] = [
    "_yoast_wpseo_title",
    "_yoast_wpseo_metadesc",
    "_yoast_wpseo_canonical",
    "_yoast_wpseo_meta-robots-noindex",
    "_yoast_wpseo_meta-robots-nofollow",
    "_yoast_wpseo_meta-robots-adv",
    "rank_math_title",
    "rank_math_description",
    "rank_math_canonical_url",
    "rank_math_robots",
];

/// The separator `%%sep%%` / `%sep%` stands for.
const SEP: &str = "-";

/// Which plugin's variables a template uses.
#[derive(Clone, Copy)]
enum Vars {
    /// `%%name%%`.
    Yoast,
    /// `%name%`.
    RankMath,
}

/// The SEO patch of `item` (`None` when it has no SEO meta) and notes on what was
/// dropped. Yoast wins over Rank Math where both set a member.
pub fn map(item: &Item, channel: &Channel) -> (Option<SeoPatch>, Vec<String>) {
    let mut notes = Vec::new();
    let text = |key: &str| meta(item, key).map(str::trim).filter(|v| !v.is_empty());
    let mut fill = |yoast: &str, rank: &str| {
        text(yoast)
            .map(|t| substitute(t, Vars::Yoast, item, channel, &mut notes))
            .or_else(|| {
                text(rank).map(|t| substitute(t, Vars::RankMath, item, channel, &mut notes))
            })
            .filter(|t| !t.is_empty())
    };
    let title = fill("_yoast_wpseo_title", "rank_math_title");
    let description = fill("_yoast_wpseo_metadesc", "rank_math_description");
    let canonical = text("_yoast_wpseo_canonical")
        .or_else(|| text("rank_math_canonical_url"))
        .filter(|c| c.trim_end_matches('/') != item.link.trim().trim_end_matches('/'))
        .map(str::to_owned);
    let robots = robots(item);

    let mut patch = SeoPatch::default();
    let mut any = false;
    for (member, value, max, name) in [
        (&mut patch.title, title, MAX_SEO_TITLE, "title"),
        (
            &mut patch.description,
            description,
            MAX_SEO_DESCRIPTION,
            "description",
        ),
    ] {
        if let Some(v) = value {
            let cleaned: String = v.chars().filter(|c| !c.is_control()).collect();
            if cleaned.chars().count() > max {
                notes.push(format!(
                    "item {}: the SEO {name} is longer than {max} characters; cut",
                    item.wp_id
                ));
            }
            *member = Some(Some(cleaned.chars().take(max).collect()));
            any = true;
        }
    }
    if let Some(c) = canonical {
        let candidate = SeoPatch {
            canonical: Some(Some(c.clone())),
            ..SeoPatch::default()
        };
        if candidate.check().is_ok() {
            patch.canonical = Some(Some(c));
            any = true;
        } else {
            notes.push(format!(
                "item {}: the SEO canonical {c:?} is not an absolute http(s) URL; dropped",
                item.wp_id
            ));
        }
    }
    if !robots.is_empty() {
        patch.robots = Some(Some(robots.into_iter().collect()));
        any = true;
    }
    (any.then_some(patch), notes)
}

/// The robots directives, sorted and deduplicated: Yoast's flags when the item has any
/// Yoast robots meta (as for the title and description, Yoast wins), else Rank Math's
/// list.
fn robots(item: &Item) -> BTreeSet<Robots> {
    let mut out = BTreeSet::new();
    let yoast = |k: &str| meta(item, k).map(str::trim);
    let has_yoast = [
        "_yoast_wpseo_meta-robots-noindex",
        "_yoast_wpseo_meta-robots-nofollow",
        "_yoast_wpseo_meta-robots-adv",
    ]
    .iter()
    .any(|k| yoast(k).is_some_and(|v| !v.is_empty()));
    if yoast("_yoast_wpseo_meta-robots-noindex") == Some("1") {
        out.insert(Robots::NoIndex);
    }
    if yoast("_yoast_wpseo_meta-robots-nofollow") == Some("1") {
        out.insert(Robots::NoFollow);
    }
    if let Some(adv) = yoast("_yoast_wpseo_meta-robots-adv") {
        out.extend(adv.split(',').filter_map(directive));
    }
    if has_yoast {
        return out;
    }
    if let Some(list) = meta(item, "rank_math_robots").and_then(|v| unserialize(v).ok()) {
        out.extend(
            list.entries()
                .iter()
                .filter_map(|(_, v)| v.as_text())
                .filter_map(|v| directive(&v)),
        );
    }
    out
}

/// A directive core stores; `index`, `follow`, `none` and the rest are not.
fn directive(text: &str) -> Option<Robots> {
    [
        Robots::NoArchive,
        Robots::NoFollow,
        Robots::NoImageIndex,
        Robots::NoIndex,
        Robots::NoSnippet,
    ]
    .into_iter()
    .find(|r| r.as_str() == text.trim().to_ascii_lowercase())
}

/// `template` with its variables substituted; unknown ones removed and noted. Runs of
/// spaces left behind collapse to one.
fn substitute(
    template: &str,
    vars: Vars,
    item: &Item,
    channel: &Channel,
    notes: &mut Vec<String>,
) -> String {
    let delim = match vars {
        Vars::Yoast => "%%",
        Vars::RankMath => "%",
    };
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find(delim) {
        let after = &rest[start + delim.len()..];
        let Some(end) = after.find(delim) else {
            break;
        };
        let name = &after[..end];
        let is_var = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !is_var {
            out.push_str(&rest[..start + delim.len()]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..start]);
        match name {
            "title" => out.push_str(&item.title),
            "sitename" => out.push_str(&channel.title),
            "sep" => out.push_str(SEP),
            other => notes.push(format!(
                "item {}: SEO variable {delim}{other}{delim} has no value here; removed",
                item.wp_id
            )),
        }
        rest = &after[end + delim.len()..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn item(meta: &[(&str, &str)]) -> Item {
        Item {
            wp_id: 3,
            title: "Hello".into(),
            link: "http://wp.test/hello/".into(),
            meta: meta
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            ..Item::default()
        }
    }

    fn channel() -> Channel {
        Channel {
            title: "Site".into(),
            ..Channel::default()
        }
    }

    #[test]
    fn yoast_variables_and_flags() {
        let (patch, notes) = map(
            &item(&[
                (
                    "_yoast_wpseo_title",
                    "%%title%% %%sep%% %%sitename%% %%page%%",
                ),
                ("_yoast_wpseo_metadesc", "About %%title%%."),
                ("_yoast_wpseo_canonical", "http://wp.test/hello"),
                ("_yoast_wpseo_meta-robots-noindex", "2"),
                ("_yoast_wpseo_meta-robots-nofollow", "1"),
                (
                    "_yoast_wpseo_meta-robots-adv",
                    "none,noimageindex, noarchive",
                ),
            ]),
            &channel(),
        );
        let patch = patch.unwrap();
        assert_eq!(patch.title, Some(Some("Hello - Site".into())));
        assert_eq!(patch.description, Some(Some("About Hello.".into())));
        // Equal to the old link: dropped.
        assert_eq!(patch.canonical, None);
        assert_eq!(
            patch.robots,
            Some(Some(vec![
                Robots::NoArchive,
                Robots::NoFollow,
                Robots::NoImageIndex
            ]))
        );
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("%%page%%"));
    }

    #[test]
    fn rank_math_and_refusals() {
        let (patch, notes) = map(
            &item(&[
                ("rank_math_title", "%title% %sep% %sitename% 100% sure"),
                (
                    "rank_math_robots",
                    r#"a:2:{i:0;s:7:"noindex";i:1;s:6:"follow";}"#,
                ),
                ("rank_math_canonical_url", "javascript:alert(1)"),
            ]),
            &channel(),
        );
        let patch = patch.unwrap();
        assert_eq!(patch.title, Some(Some("Hello - Site 100% sure".into())));
        assert_eq!(patch.robots, Some(Some(vec![Robots::NoIndex])));
        assert_eq!(patch.canonical, None);
        assert!(notes.iter().any(|n| n.contains("canonical")), "{notes:?}");
    }

    #[test]
    fn yoast_robots_win_over_rank_math() {
        let (patch, _) = map(
            &item(&[
                ("_yoast_wpseo_meta-robots-nofollow", "1"),
                ("rank_math_robots", r#"a:1:{i:0;s:7:"noindex";}"#),
            ]),
            &channel(),
        );
        assert_eq!(patch.unwrap().robots, Some(Some(vec![Robots::NoFollow])));
    }

    #[test]
    fn nothing_set_is_none() {
        let (patch, notes) = map(
            &item(&[(
                "rank_math_robots",
                r#"a:2:{i:0;s:5:"index";i:1;s:6:"follow";}"#,
            )]),
            &channel(),
        );
        assert_eq!(patch, None);
        assert!(notes.is_empty());
    }
}
