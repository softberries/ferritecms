//! Redirects from the old site's URLs (pass 4, P17).
//!
//! Rows, in this order:
//! - every imported item whose old `<link>` path differs from its new path; only a
//!   published item gets one (a redirect to an item nobody may see would show a 404), the
//!   others are noted as refused;
//! - attachment pages (their `<link>`) → the media's URL;
//! - tag archives `/<tag base>/<slug>/` and category archives `/<category base>/<parent
//!   slugs>/<slug>/` (WordPress nests category paths) → the term's URL, where they differ.
//!
//! A row whose old path equals the new one is not made. Rows core refuses, and the ones
//! refused here, are noted in that input order; none fails the run.

use std::collections::HashMap;

use ferrite_core::import::{ImportBatch, RedirectOutcome};
use ferrite_core::permalink::term_path;
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
                rows.push(Row::Write(old, media.url.clone()));
            }
            continue;
        }
        if type_name(&item.post_type).is_none() {
            continue;
        }
        let Some((_, new)) = links.written.get(&item.wp_id) else {
            continue;
        };
        if *new == old {
            continue;
        }
        if item.status == "publish" {
            rows.push(Row::Write(old, new.clone()));
        } else {
            rows.push(Row::Refused(
                old,
                format!("the item is not published ({})", item.status),
            ));
        }
    }
    drop(items);
    let opts = session.opts();
    let terms = ir.terms().await?;
    let by_slug: HashMap<(&str, &str), &Term> = terms
        .iter()
        .map(|t| ((t.taxonomy.as_str(), t.slug.as_str()), t))
        .collect();
    for term in &terms {
        let (base, core) = match term.taxonomy.as_str() {
            "category" => (opts.category_base.as_str(), "category"),
            "post_tag" => (opts.tag_base.as_str(), "tag"),
            _ => continue,
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
        let old = format!(
            "/{}/{}/",
            base.trim_matches('/'),
            chain
                .iter()
                .map(|s| ferrite_core::permalink::encode_segment(s))
                .collect::<Vec<_>>()
                .join("/")
        );
        let new = term_path(core, &percent_decode(&term.slug));
        if old != new {
            rows.push(Row::Write(old, new));
        }
    }

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
        // Refusals in input order: core's come back in row order, ours are in place.
        let mut core_refused = out.refused.into_iter().peekable();
        for row in chunk {
            match row {
                Row::Refused(source, why) => {
                    report.counts_of(KIND).skipped += 1;
                    report.note(format!("redirect from {source} not made: {why}"));
                }
                Row::Write(source, _) => {
                    if let Some((_, why)) = core_refused.next_if(|(s, _)| s == source) {
                        report.counts_of(KIND).skipped += 1;
                        report.note(format!("redirect from {source} not made: {why}"));
                    }
                }
            }
        }
    }
    Ok(())
}

async fn write(
    b: &mut ImportBatch<'_>,
    rows: &[(String, String, u16)],
) -> Result<RedirectOutcome, ImportError> {
    Ok(b.redirects(rows).await?)
}
