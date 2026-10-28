//! WXR terms → terms (pass 1), parents first.
//!
//! Slugs are percent-decoded as WordPress stores non-ASCII ones (`%d0%b0`). Terms are
//! written in topological order, so a term's parent already has its map row: the parent
//! is found with `ImportBatch::lookup`, which also works when an earlier run or batch
//! wrote it. A parent that is not in the file, or a cycle, drops the parent (reported).
//! A site term with the same taxonomy and slug is reused as it is (`TermMatched::Slug`).
//!
//! Every batch is a schema batch that first [`ensure`](super::types::ensure)s the types
//! and taxonomies: on a dry run each batch is rolled back, and a later batch's terms
//! still need their taxonomy.

use std::collections::HashMap;

use ferrite_core::import::{ImportBatch, ImportTerm, TermMatched};
use ferrite_core::slug;
use ferrite_domain::{ImportMapKind, TermId};

use super::types::{Schema, ensure, taxonomy_name};
use crate::ImportError;
use crate::model::Term;
use crate::run::{Report, Session};
use crate::urls::percent_decode;

/// One term ready to write: its decoded slug and taxonomy name, and its parent's WP id.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Planned {
    pub term: Term,
    pub taxonomy: String,
    pub parent: Option<i64>,
}

/// Registers `schema` and imports `terms`, `batch_items` per batch.
pub(crate) async fn import(
    session: &Session<'_>,
    schema: &Schema,
    terms: Vec<Term>,
    report: &mut Report,
) -> Result<(), ImportError> {
    let (planned, skipped) = order(terms, report);
    report.counts_of("terms").skipped += skipped;
    let chunks: Vec<&[Planned]> = if planned.is_empty() {
        vec![&[]]
    } else {
        planned.chunks(session.batch_items()).collect()
    };
    for (n, chunk) in chunks.into_iter().enumerate() {
        let mut batch = session.schema_batch().await?;
        let mut part = Report::default();
        let written = write(&mut batch, schema, chunk, (n == 0).then_some(&mut part)).await;
        let outcomes = session.finish_batch(batch, written).await?;
        merge(report, part);
        for (p, refused, how) in outcomes {
            let counts = report.counts_of("terms");
            match how {
                TermMatched::Created => counts.created += 1,
                TermMatched::Mapped | TermMatched::Slug => counts.unchanged += 1,
            }
            if refused {
                report.note(format!(
                    "term {}/{:?}: the slug is not allowed here; one was made from the name",
                    p.taxonomy, p.term.slug
                ));
            }
        }
    }
    Ok(())
}

/// One schema batch: ensures `schema` (counted into `part` when given), then writes
/// `chunk`; per term, whether its slug was refused and how it was matched.
async fn write<'c>(
    b: &mut ImportBatch<'_>,
    schema: &Schema,
    chunk: &'c [Planned],
    part: Option<&mut Report>,
) -> Result<Vec<(&'c Planned, bool, TermMatched)>, ImportError> {
    ensure(b, schema, part).await?;
    let mut out = Vec::with_capacity(chunk.len());
    for p in chunk {
        let parent = match p.parent {
            Some(wp) => b
                .lookup(ImportMapKind::Term, wp)
                .await?
                .map(|m| TermId::from_uuid(m.id)),
            None => None,
        };
        let (slug, refused) = slug_of(&p.term.slug);
        let name = Some(p.term.name.trim())
            .filter(|n| !n.is_empty())
            .unwrap_or(p.term.slug.as_str())
            .to_owned();
        let outcome = b
            .term(ImportTerm {
                wp_id: p.term.wp_id,
                taxonomy: p.taxonomy.clone(),
                slug,
                name,
                description: p.term.description.clone(),
                parent,
            })
            .await?;
        out.push((p, refused, outcome.how));
    }
    Ok(out)
}

/// Adds the counts and notes of `part` to `report`.
fn merge(report: &mut Report, part: Report) {
    for (kind, c) in part.counts {
        let counts = report.counts_of(&kind);
        counts.created += c.created;
        counts.updated += c.updated;
        counts.unchanged += c.unchanged;
        counts.kept += c.kept;
        counts.skipped += c.skipped;
        counts.failed += c.failed;
    }
    report.notes.extend(part.notes);
}

/// The slug to write (empty: core makes one from the name) and whether the WP one was
/// refused.
fn slug_of(decoded: &str) -> (String, bool) {
    if decoded.trim().is_empty() {
        return (String::new(), false);
    }
    match slug::validate(decoded) {
        Ok(s) => (s, false),
        Err(_) => (String::new(), true),
    }
}

/// The terms to import, parents before children, with decoded slugs; and how many were
/// skipped (taxonomies not imported as such, e.g. menus). Within a taxonomy a parent is
/// found by slug (the first term with it); a parent that is not there or closes a cycle
/// is dropped and noted.
pub(crate) fn order(terms: Vec<Term>, report: &mut Report) -> (Vec<Planned>, u64) {
    let mut skipped = 0;
    let mut planned: Vec<Planned> = Vec::with_capacity(terms.len());
    let mut parent_slugs: Vec<Option<String>> = Vec::with_capacity(terms.len());
    for mut term in terms {
        let Some(taxonomy) = taxonomy_name(&term.taxonomy) else {
            skipped += 1;
            continue;
        };
        term.slug = percent_decode(&term.slug);
        let parent = term
            .parent_slug
            .as_deref()
            .map(percent_decode)
            .filter(|p| !p.is_empty());
        term.parent_slug.clone_from(&parent);
        parent_slugs.push(parent);
        planned.push(Planned {
            term,
            taxonomy,
            parent: None,
        });
    }
    let mut by_slug: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, p) in planned.iter().enumerate() {
        by_slug
            .entry((p.taxonomy.as_str(), p.term.slug.as_str()))
            .or_insert(i);
    }
    let mut parent_of: Vec<Option<usize>> = Vec::with_capacity(planned.len());
    for (p, parent) in planned.iter().zip(&parent_slugs) {
        let found = parent
            .as_deref()
            .and_then(|s| by_slug.get(&(p.taxonomy.as_str(), s)).copied());
        if let (Some(slug), None) = (parent, found) {
            report.note(format!(
                "term {}/{}: its parent {slug:?} is not in the file; imported without a parent",
                p.taxonomy, p.term.slug
            ));
        }
        parent_of.push(found);
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        New,
        Visiting,
        Done,
    }
    let mut state = vec![State::New; planned.len()];
    let mut out = Vec::with_capacity(planned.len());
    for start in 0..planned.len() {
        if state[start] != State::New {
            continue;
        }
        // Walk up to a root (or a term already placed), then place the path top-down.
        let mut path = Vec::new();
        let mut cur = start;
        loop {
            state[cur] = State::Visiting;
            path.push(cur);
            match parent_of[cur] {
                Some(p) if state[p] == State::New => cur = p,
                Some(p) if state[p] == State::Visiting => {
                    parent_of[cur] = None;
                    let t = &planned[cur];
                    report.note(format!(
                        "term {}/{}: its parent makes a cycle; imported without a parent",
                        t.taxonomy, t.term.slug
                    ));
                    break;
                }
                _ => break,
            }
        }
        for i in path.into_iter().rev() {
            state[i] = State::Done;
            out.push(i);
        }
    }
    let wp_ids: Vec<i64> = planned.iter().map(|p| p.term.wp_id).collect();
    let mut slots: Vec<Option<Planned>> = planned.into_iter().map(Some).collect();
    let ordered = out
        .into_iter()
        .filter_map(|i| {
            let mut p = slots[i].take()?;
            p.parent = parent_of[i].map(|j| wp_ids[j]);
            Some(p)
        })
        .collect();
    (ordered, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(id: i64, tax: &str, slug: &str, parent: Option<&str>) -> Term {
        Term {
            wp_id: id,
            taxonomy: tax.into(),
            slug: slug.into(),
            name: slug.to_uppercase(),
            description: String::new(),
            parent_slug: parent.map(Into::into),
        }
    }

    fn shape(planned: &[Planned]) -> Vec<(i64, Option<i64>)> {
        planned.iter().map(|p| (p.term.wp_id, p.parent)).collect()
    }

    #[test]
    fn parents_come_first() {
        let mut report = Report::default();
        let (planned, skipped) = order(
            vec![
                term(1, "category", "c", Some("b")),
                term(2, "category", "b", Some("a")),
                term(3, "category", "a", None),
                term(4, "nav_menu", "main", None),
                term(5, "genre", "%d0%b0", Some("")),
                term(6, "genre", "x", Some("%d0%b0")),
            ],
            &mut report,
        );
        assert_eq!(skipped, 1);
        assert_eq!(
            shape(&planned),
            [
                (3, None),
                (2, Some(3)),
                (1, Some(2)),
                (5, None),
                (6, Some(5))
            ]
        );
        assert_eq!(planned[3].term.slug, "а");
        assert!(report.notes.is_empty(), "{:?}", report.notes);
    }

    #[test]
    fn cycles_and_missing_parents_are_dropped_and_noted() {
        let mut report = Report::default();
        let (planned, _) = order(
            vec![
                term(1, "category", "a", Some("b")),
                term(2, "category", "b", Some("c")),
                term(3, "category", "c", Some("a")),
                term(4, "category", "d", Some("gone")),
                term(5, "post_tag", "e", Some("a")),
                term(6, "category", "self", Some("self")),
            ],
            &mut report,
        );
        // The walk from `a` reaches `c`, whose parent `a` closes the cycle.
        assert_eq!(
            shape(&planned),
            [
                (3, None),
                (2, Some(3)),
                (1, Some(2)),
                (4, None),
                (5, None),
                (6, None)
            ]
        );
        let notes = report.notes.join("\n");
        assert_eq!(notes.matches("cycle").count(), 2, "{notes}");
        assert!(notes.contains("category/d"), "{notes}");
        assert!(notes.contains("tag/e"), "{notes}");
    }

    #[test]
    fn refused_slugs_fall_back_to_the_name() {
        assert_eq!(slug_of("news"), ("news".into(), false));
        assert_eq!(slug_of("a b"), (String::new(), true));
        assert_eq!(slug_of(" "), (String::new(), false));
    }
}
