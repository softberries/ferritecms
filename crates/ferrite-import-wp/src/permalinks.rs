//! The post permalink structure of the old site, inferred from its posts' URLs (P17).
//!
//! WXR carries no options, so the structure is read off the `<link>` of every published
//! post: each path segment must be the post's slug (`%slug%`), the year or two-digit
//! month of its date (`%year%`, `%month%`), its primary category's slug (`%category%`), or
//! a lower-case literal. The structure is set only when every post gives the same one
//! and core accepts it; otherwise the site keeps its own (and redirects cover the rest).

use ferrite_core::PermalinkRules;

use crate::model::Item;
use crate::urls::percent_decode;

/// The structure every post's `<link>` fits (see the module docs), e.g.
/// `/%year%/%month%/%slug%/`. `posts` are the published posts with their primary
/// category's slug. `None` when there are none, they disagree, a link has a query
/// (WordPress's plain `?p=N` links) or core refuses the structure.
pub fn infer(posts: &[(&Item, Option<&str>)]) -> Option<String> {
    let mut found: Option<String> = None;
    for (item, category) in posts {
        let structure = structure_of(item, *category)?;
        match &found {
            Some(f) if *f != structure => return None,
            Some(_) => {}
            None => found = Some(structure),
        }
    }
    let structure = found?;
    PermalinkRules::from_options(Some(&structure), None)
        .ok()
        .map(|_| structure)
}

/// The structure of one post's link.
fn structure_of(item: &Item, category: Option<&str>) -> Option<String> {
    let url = url::Url::parse(item.link.trim()).ok()?;
    if url.query().is_some() {
        return None;
    }
    let slug = percent_decode(&item.post_name);
    let date = item.date_local.trim();
    let (year, month) = (date.get(..4)?, date.get(5..7)?);
    let mut tokens = Vec::new();
    for segment in url.path().split('/').filter(|s| !s.is_empty()) {
        let segment = percent_decode(segment);
        let token = if !slug.is_empty() && segment == slug {
            "%slug%".to_owned()
        } else if segment == year {
            "%year%".to_owned()
        } else if segment == month {
            "%month%".to_owned()
        } else if category.is_some_and(|c| percent_decode(c) == segment) {
            "%category%".to_owned()
        } else if is_literal(&segment) {
            segment
        } else {
            return None;
        };
        tokens.push(token);
    }
    if !tokens.iter().any(|t| t == "%slug%") {
        return None;
    }
    Some(format!("/{}/", tokens.join("/")))
}

/// A lower-case literal segment (`[a-z0-9-_]`, not all digits).
fn is_literal(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !segment.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(link: &str, slug: &str, date: &str) -> Item {
        Item {
            link: link.into(),
            post_name: slug.into(),
            date_local: date.into(),
            ..Item::default()
        }
    }

    #[test]
    fn structures_are_read_off_the_links() {
        let a = post(
            "http://wp.test/2024/03/hello/",
            "hello",
            "2024-03-15 10:30:00",
        );
        let b = post("http://wp.test/2023/11/bye/", "bye", "2023-11-02 08:00:00");
        assert_eq!(
            infer(&[(&a, None), (&b, Some("news"))]).as_deref(),
            Some("/%year%/%month%/%slug%/")
        );
        let c = post("http://wp.test/travel/walk/", "walk", "2021-07-04 18:00:00");
        let d = post("http://wp.test/food/cafe/", "cafe", "2021-08-01 08:00:00");
        assert_eq!(
            infer(&[(&c, Some("travel")), (&d, Some("food"))]).as_deref(),
            Some("/%category%/%slug%/")
        );
        let e = post("http://wp.test/blog/walk/", "walk", "2021-07-04 18:00:00");
        assert_eq!(infer(&[(&e, None)]).as_deref(), Some("/blog/%slug%/"));
    }

    #[test]
    fn mixed_plain_or_no_posts_give_none() {
        let a = post(
            "http://wp.test/2024/03/hello/",
            "hello",
            "2024-03-15 10:30:00",
        );
        let c = post("http://wp.test/travel/walk/", "walk", "2021-07-04 18:00:00");
        assert_eq!(infer(&[(&a, None), (&c, Some("travel"))]), None);
        let plain = post("http://wp.test/?p=5", "hello", "2024-03-15 10:30:00");
        assert_eq!(infer(&[(&plain, None)]), None);
        let odd = post("http://wp.test/Odd/hello/", "hello", "2024-03-15 10:30:00");
        assert_eq!(infer(&[(&odd, None)]), None);
        assert_eq!(infer(&[]), None);
    }
}
