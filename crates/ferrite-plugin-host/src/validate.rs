//! Output validation for filter results (design 05 §7). M0 checks the variant and size
//! caps only; HTML sanitising and head rendering belong to the M1 render pipeline.

use crate::capability::FilterType;
use crate::types::{FilterValue, Head};

/// Cap on the total length of all strings in a returned `head`.
const MAX_HEAD_BYTES: usize = 64 * 1024;
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
    opt(&h.title) + metas + links + scripts
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
