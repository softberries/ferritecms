//! Output validation for filter results (design 05 §7). M0 checks the variant and size
//! caps only; HTML sanitising and head rendering belong to the M1 render pipeline.

use crate::capability::FilterType;
use crate::types::{FilterValue, Head};

/// Cap on the size of a returned `head`: all its strings plus [`HEAD_ELEMENT_COST`] per
/// meta/link/script, so many empty tags cannot slip under it.
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Bytes charged per head element on top of its strings (the host-side `Vec` entry and
/// the rendered tag's markup).
const HEAD_ELEMENT_COST: usize = 64;
/// Most elements of each kind a returned `head` may carry.
const MAX_HEAD_METAS: usize = 256;
const MAX_HEAD_LINKS: usize = 64;
const MAX_HEAD_SCRIPTS: usize = 16;
/// Floor and ceiling of the `html`/`text` cap, which is otherwise 4x the input length.
const MIN_BODY_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Whether `value` is the variant that filters of type `ty` work on.
pub(crate) fn matches_type(ty: FilterType, value: &FilterValue) -> bool {
    matches!(
        (ty, value),
        (FilterType::Head, FilterValue::Head(_))
            | (FilterType::Html, FilterValue::Html(_))
            | (FilterType::Text, FilterValue::Text(_))
    )
}

fn head_bytes(h: &Head) -> usize {
    let opt = |s: &Option<String>| s.as_deref().map_or(0, str::len);
    let metas: usize = h
        .metas
        .iter()
        .map(|m| opt(&m.name) + opt(&m.property) + m.content.len())
        .sum();
    let links: usize = h
        .links
        .iter()
        .map(|l| l.rel.len() + l.href.len() + opt(&l.hreflang) + opt(&l.kind))
        .sum();
    let scripts: usize = h
        .scripts
        .iter()
        .map(|s| opt(&s.src) + opt(&s.json_ld))
        .sum();
    let elements = h.metas.len() + h.links.len() + h.scripts.len();
    opt(&h.title) + metas + links + scripts + elements.saturating_mul(HEAD_ELEMENT_COST)
}

/// Reject a `head` with more elements of one kind than allowed.
fn check_head_counts(h: &Head) -> Result<(), String> {
    for (kind, count, max) in [
        ("metas", h.metas.len(), MAX_HEAD_METAS),
        ("links", h.links.len(), MAX_HEAD_LINKS),
        ("scripts", h.scripts.len(), MAX_HEAD_SCRIPTS),
    ] {
        if count > max {
            return Err(format!("head has {count} {kind}, limit {max}"));
        }
    }
    Ok(())
}

fn body_len(v: &FilterValue) -> usize {
    match v {
        FilterValue::Html(s) | FilterValue::Text(s) => s.len(),
        FilterValue::Head(h) => head_bytes(h),
    }
}

/// Accept `output` of a filter of type `ty` that was given `input`, or say why not.
pub(crate) fn validate_output(
    ty: FilterType,
    input: &FilterValue,
    output: FilterValue,
) -> Result<FilterValue, String> {
    if !matches_type(ty, &output) {
        return Err(format!("output variant does not match filter type {ty:?}"));
    }
    match &output {
        FilterValue::Head(h) => {
            check_head_counts(h)?;
            let size = head_bytes(h);
            if size > MAX_HEAD_BYTES {
                return Err(format!("head is {size} bytes, limit {MAX_HEAD_BYTES}"));
            }
        }
        FilterValue::Html(s) | FilterValue::Text(s) => {
            let cap = body_len(input)
                .saturating_mul(4)
                .clamp(MIN_BODY_BYTES, MAX_BODY_BYTES);
            if s.len() > cap {
                return Err(format!("output is {} bytes, limit {cap}", s.len()));
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(n: usize) -> FilterValue {
        FilterValue::Html("a".repeat(n))
    }

    #[test]
    fn head_over_64_kib_rejected() {
        let input = FilterValue::Head(Head::default());
        let big = FilterValue::Head(Head {
            title: Some("t".repeat(70 * 1024)),
            ..Head::default()
        });
        assert!(validate_output(FilterType::Head, &input, big).is_err());
        let ok = FilterValue::Head(Head {
            title: Some("t".repeat(64 * 1024)),
            ..Head::default()
        });
        assert!(validate_output(FilterType::Head, &input, ok).is_ok());
    }

    use crate::types::{LinkTag, MetaTag, ScriptTag};

    fn meta(content: &str) -> MetaTag {
        MetaTag {
            name: None,
            property: None,
            content: content.into(),
        }
    }

    fn check(h: Head) -> Result<FilterValue, String> {
        validate_output(
            FilterType::Head,
            &FilterValue::Head(Head::default()),
            FilterValue::Head(h),
        )
    }

    #[test]
    fn head_element_counts_are_capped() {
        let metas = |n| Head {
            metas: vec![meta(""); n],
            ..Head::default()
        };
        assert!(check(metas(256)).is_ok());
        assert!(check(metas(257)).is_err());
        assert!(check(metas(500_000)).is_err(), "empty tags are not free");

        let link = LinkTag {
            rel: String::new(),
            href: String::new(),
            hreflang: None,
            kind: None,
        };
        let links = |n| Head {
            links: vec![link.clone(); n],
            ..Head::default()
        };
        assert!(check(links(64)).is_ok());
        assert!(check(links(65)).is_err());

        let script = ScriptTag {
            src: None,
            json_ld: None,
        };
        let scripts = |n| Head {
            scripts: vec![script.clone(); n],
            ..Head::default()
        };
        assert!(check(scripts(16)).is_ok());
        assert!(check(scripts(17)).is_err());
    }

    #[test]
    fn each_head_element_costs_64_bytes() {
        // 256 metas: 256 * (64 + 192) = 64 KiB exactly.
        let full = Head {
            metas: vec![meta(&"c".repeat(192)); 256],
            ..Head::default()
        };
        assert!(check(full.clone()).is_ok());
        let mut over = full;
        over.metas[0].content.push('c');
        assert!(check(over).is_err());
        // A title alone may use the whole budget; one element on top does not fit.
        let title = Head {
            title: Some("t".repeat(64 * 1024 - 64)),
            scripts: vec![ScriptTag {
                src: None,
                json_ld: None,
            }],
            ..Head::default()
        };
        assert!(check(title.clone()).is_ok());
        let mut over = title;
        over.title = over.title.map(|t| t + "t");
        assert!(check(over).is_err());
    }

    #[test]
    fn html_over_four_times_input_rejected() {
        let input = html(100 * 1024);
        assert!(validate_output(FilterType::Html, &input, html(4 * 100 * 1024 + 1)).is_err());
        assert!(validate_output(FilterType::Html, &input, html(4 * 100 * 1024)).is_ok());
    }

    #[test]
    fn small_input_gets_64_kib_floor() {
        let input = html(1);
        assert!(validate_output(FilterType::Html, &input, html(60 * 1024)).is_ok());
        assert!(validate_output(FilterType::Html, &input, html(64 * 1024 + 1)).is_err());
    }

    #[test]
    fn never_above_2_mib() {
        let input = html(1024 * 1024);
        assert!(validate_output(FilterType::Html, &input, html(2 * 1024 * 1024)).is_ok());
        assert!(validate_output(FilterType::Html, &input, html(2 * 1024 * 1024 + 1)).is_err());
    }

    #[test]
    fn wrong_variant_rejected() {
        let input = FilterValue::Head(Head::default());
        let out = FilterValue::Text("x".into());
        assert!(validate_output(FilterType::Head, &input, out).is_err());
        let out = FilterValue::Text("x".into());
        assert!(validate_output(FilterType::Html, &html(1), out).is_err());
    }
}
