//! Redirects from the old site's URLs (pass 4, P17).
//!
//! Rows, in this order:
//! - every imported item whose old `<link>` path differs from its new path; only a
//!   published item gets one (a redirect to an item nobody may see would show a 404), the
//!   others are noted as refused;
//! - attachment pages (their `<link>`) → the media's URL;
//! - tag archives `/<tag base>/<slug>/` and category archives `/<category base>/<parent
//!   slugs>/<slug>/` (WordPress nests category paths) → the URL of the term as imported
//!   (its slug may differ from the WP one); terms not imported get none.
//!
//! Sources are paths as the site serves them: each segment percent-encoded as permalinks
//! are, so a non-ASCII path compares (and fires) as the router sees it. Links with a
//! query (`?p=`, `?attachment_id=`, …) have no path to redirect from, and the site root
//! `/` is never a source (it would take over the home page): refused and noted. A row
//! whose old path equals the new one is not made. A redirect edited (or made) here is
//! kept. Refusals are noted in input order; none fails the run.

use std::collections::HashMap;

use ferrite_core::import::{ImportBatch, RedirectOutcome};
use ferrite_core::permalink::{encode_segment, term_path};
use ferrite_domain::{ImportMapKind, TermId};
use futures::TryStreamExt;

use super::content::Links;
use super::types::type_name;
use crate::ImportError;
use crate::model::Term;
use crate::run::{Report, Session};
use crate::urls::percent_decode;

/// The report kind of redirects.
const KIND: &str = "redirects";
/// Moved permanently.
const CODE: u16 = 301;

/// One candidate row, or a refusal made here.
enum Row {
    Write(String, String),
    Refused(String, String),
}

/// A decoded path with each segment encoded as the router compares paths.
fn encoded(path: &str) -> String {
    let segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(encode_segment)
        .collect();
    if segments.is_empty() {
        "/".to_owned()
    } else {
        format!("/{}/", segments.join("/"))
    }
}

/// The row of `old` (a decoded path) → `new`, if one is needed.
fn row(old: &str, new: &str) -> Option<Row> {
    let source = encoded(old);
    if source == "/" {
        return Some(Row::Refused(
            source,
            "the site root is never redirected (it would hide the home page)".into(),
        ));
    }
    (source != new).then(|| Row::Write(source, new.to_owned()))
}

/// Makes the redirects of the run, `batch_items` rows per batch.
pub(crate) async fn import(
    session: &Session<'_>,
    links: &Links,
    report: &mut Report,
) -> Result<(), ImportError> {
    let ir = session.ir();
    let mut rows: Vec<Row> = Vec::new();
    let mut items = ir.items(&[]);
    while let Some(item) = items.try_next().await? {
        let Some(old) = links.old_path(&item.link) else {
            continue;
        };
        if item.post_type == "attachment" {
            let url = item.attachment_url.as_deref().unwrap_or(&item.guid);
            if let Some(media) = links.media.resolve(url) {
                rows.extend(row(&old, &media.url));
            }
            continue;
        }
        if type_name(&item.post_type).is_none() {
            continue;
        }
        let Some((_, new)) = links.written.get(&item.wp_id) else {
            continue;
        };
        match row(&old, new) {
            Some(Row::Write(source, _)) if item.status != "publish" => rows.push(Row::Refused(
                source,
                format!("the item is not published ({})", item.status),
            )),
            other => rows.extend(other),
        }
    }
    drop(items);
    rows.extend(term_rows(session).await?);

    for chunk in rows.chunks(session.batch_items()) {
        let writes: Vec<(String, String, u16)> = chunk
            .iter()
            .filter_map(|r| match r {
                Row::Write(s, t) => Some((s.clone(), t.clone(), CODE)),
                Row::Refused(..) => None,
            })
            .collect();
        let mut batch = session.batch().await?;
        let written = write(&mut batch, &writes).await;
        let out = session.finish_batch(batch, written).await?;
        let counts = report.counts_of(KIND);
        counts.created += u64::from(out.created);
        counts.updated += u64::from(out.updated);
        counts.unchanged += u64::from(out.unchanged);
        // Notes in input order: core's refusals and kept rows come back in row order,
        // ours are in place.
        let mut refused = out.refused.into_iter().peekable();
        let mut kept = out.kept.into_iter().peekable();
        for row in chunk {
            match row {
                Row::Refused(source, why) => {
                    report.counts_of(KIND).skipped += 1;
                    report.note(format!("redirect from {source} not made: {why}"));
                }
                Row::Write(source, _) => {
                    if let Some((_, why)) = refused.next_if(|(s, _)| s == source) {
                        report.counts_of(KIND).skipped += 1;
                        report.note(format!("redirect from {source} not made: {why}"));
                    } else if kept.next_if(|s| s == source).is_some() {
                        report.counts_of(KIND).kept += 1;
                        report.note(format!(
                            "redirect from {source} was edited here (or made here); kept"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The rows of the tag and category archives, to each imported term's URL.
async fn term_rows(session: &Session<'_>) -> Result<Vec<Row>, ImportError> {
    let opts = session.opts();
    let terms = session.ir().terms().await?;
    let by_slug: HashMap<(&str, &str), &Term> = terms
        .iter()
        .map(|t| ((t.taxonomy.as_str(), t.slug.as_str()), t))
        .collect();
    let wanted: Vec<&Term> = terms
        .iter()
        .filter(|t| matches!(t.taxonomy.as_str(), "category" | "post_tag"))
        .collect();
    let ids: Vec<i64> = wanted.iter().map(|t| t.wp_id).collect();
    let mut batch = session.batch().await?;
    let found = imported_slugs(&mut batch, &ids).await;
    let imported = session.finish_batch(batch, found).await?;
    let mut rows = Vec::new();
    for term in wanted {
        let Some((taxonomy, slug)) = imported.get(&term.wp_id) else {
            continue;
        };
        let base = match term.taxonomy.as_str() {
            "category" => opts.category_base.as_str(),
            _ => opts.tag_base.as_str(),
        };
        let mut chain = vec![percent_decode(&term.slug)];
        let mut parent = term.parent_slug.as_deref().filter(|p| !p.is_empty());
        while let Some(p) = parent {
            if chain.len() > 64 {
                break;
            }
            chain.push(percent_decode(p));
            parent = by_slug
                .get(&(term.taxonomy.as_str(), p))
                .and_then(|t| t.parent_slug.as_deref())
                .filter(|p| !p.is_empty());
        }
        chain.reverse();
        let old = format!("/{}/{}/", base.trim_matches('/'), chain.join("/"));
        rows.extend(row(&old, &term_path(taxonomy, slug)));
    }
    Ok(rows)
}

/// The taxonomy and slug each imported term (by WP id) has here.
async fn imported_slugs(
    b: &mut ImportBatch<'_>,
    wp_ids: &[i64],
) -> Result<HashMap<i64, (String, String)>, ImportError> {
    let mut out = HashMap::new();
    for wp_id in wp_ids {
        let Some(m) = b.lookup(ImportMapKind::Term, *wp_id).await? else {
            continue;
        };
        if let Some(term) = b.get_term(TermId::from_uuid(m.id)).await? {
            out.insert(*wp_id, (term.taxonomy, term.slug));
        }
    }
    Ok(out)
}

async fn write(
    b: &mut ImportBatch<'_>,
    rows: &[(String, String, u16)],
) -> Result<RedirectOutcome, ImportError> {
    Ok(b.redirects(rows).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_encoded_and_never_the_root() {
        assert_eq!(encoded("/аб/"), "/%D0%B0%D0%B1/");
        assert_eq!(encoded("/a b/c"), "/a%20b/c/");
        assert!(matches!(row("/", "/x/"), Some(Row::Refused(s, _)) if s == "/"));
        assert!(row("/аб/", "/%D0%B0%D0%B1/").is_none());
        assert!(
            matches!(row("/old/", "/new/"), Some(Row::Write(s, t)) if s == "/old/" && t == "/new/")
        );
    }
}
