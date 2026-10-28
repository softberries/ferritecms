//! Imported redirects (P17): rows are checked one by one; the refused ones are reported
//! and the rest written with origin `import`.

use ferrite_domain::{ContentItem, ContentStatus, MEDIA_KIND};
use ferrite_store::Repos;

use super::ImportBatch;
use crate::{
    CoreError, MAX_IMPORT_ROWS, MAX_PARENT_DEPTH,
    permalink::{Permalinks, load_input},
    redirects::{all_targets_in, redirect_keys, toggled, validate_rows, write_rows},
};

/// Why a row whose source is a live item's path is refused.
const PUBLISHED_ITEM: &str = "path is a published item";

/// What [`ImportBatch::redirects`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectOutcome {
    /// Rows whose source had no redirect.
    pub created: u32,
    /// Rows that replaced the target and code of an existing redirect.
    pub updated: u32,
    /// Rows equal (target and code) to the stored redirect of their source: nothing
    /// written (a re-run).
    pub unchanged: u32,
    /// Rows not written: (source, reason), in input-row order whichever check refused them.
    pub refused: Vec<(String, String)>,
}

impl ImportBatch<'_> {
    /// Writes `(source, target, code)` rows as redirects with origin `import`, in chunks
    /// of [`MAX_IMPORT_ROWS`]. Each row gets the checks of
    /// [`RedirectService::import`](crate::RedirectService::import) (path rules, router-owned
    /// sources, code, loops and chains, duplicates; against the stored redirects and the
    /// valid rows before it), but a failing row does not fail the call: it is returned in
    /// [`refused`](RedirectOutcome::refused) with its reason and the rest is written.
    /// A row whose source is the path of a published item (the item is live there, so a
    /// redirect would never fire) is refused with "path is a published item"; the lookup
    /// walks the source's segments by slug, for every content type but media.
    /// A valid row equal to the stored redirect of its source is not written
    /// ([`unchanged`](RedirectOutcome::unchanged)). The `redirect:` keys of the written
    /// sources join the batch's event.
    ///
    /// **Errors.** As for [`content`](Self::content): any `Err` (a store failure; row
    /// problems are never one) leaves the batch unusable, and the caller must
    /// [`rollback`](Self::rollback) it.
    pub async fn redirects(
        &mut self,
        rows: &[(String, String, u16)],
    ) -> Result<RedirectOutcome, CoreError> {
        let mut out = RedirectOutcome {
            created: 0,
            updated: 0,
            unchanged: 0,
            refused: Vec::new(),
        };
        for chunk in rows.chunks(MAX_IMPORT_ROWS) {
            let repos = self.tx.as_ref();
            // (index in `chunk`, row) of the rows that are not refused so far.
            let mut candidates: Vec<(usize, (String, String, u16))> =
                Vec::with_capacity(chunk.len());
            let mut refused: Vec<(usize, String, String)> = Vec::new();
            for (i, row) in chunk.iter().enumerate() {
                if published_item_at(repos, self.rules.as_ref(), &row.0).await? {
                    refused.push((i, row.0.clone(), PUBLISHED_ITEM.into()));
                } else {
                    candidates.push((i, row.clone()));
                }
            }
            let kept: Vec<_> = candidates.iter().map(|(_, r)| r.clone()).collect();
            let errors = validate_rows(&kept, all_targets_in(repos).await?);
            let mut failed = vec![false; candidates.len()];
            for e in errors {
                if let Some((i, row)) = candidates.get(e.row) {
                    refused.push((*i, row.0.clone(), e.message));
                    failed[e.row] = true;
                }
            }
            refused.sort_by_key(|(i, _, _)| *i);
            out.refused
                .extend(refused.into_iter().map(|(_, source, why)| (source, why)));
            let candidates: Vec<_> = candidates.into_iter().map(|(_, r)| r).collect();
            let mut valid = Vec::with_capacity(candidates.len());
            for (row, failed) in candidates.into_iter().zip(failed) {
                if failed {
                    continue;
                }
                let same = repos
                    .redirects()
                    .get_by_source(&row.0)
                    .await?
                    .is_some_and(|r| r.target == row.1 && r.code == row.2);
                if same {
                    out.unchanged += 1;
                } else {
                    valid.push(row);
                }
            }
            let report = write_rows(repos, &valid).await?;
            out.created += report.created;
            out.updated += report.updated;
            self.keys.extend(redirect_keys(
                valid.into_iter().map(|(source, _, _)| source),
            ));
            self.items += (report.created + report.updated) as usize;
        }
        Ok(out)
    }
}

/// Whether `source` (or its slash variant) is the permalink of a published item of a
/// content type other than media. Candidates are found by walking the path's trailing
/// segments from the root by slug (`get_by_slug`, the way the site resolves pages), and
/// each is compared by its computed path; a path with a query, a fragment or more than
/// `MAX_PARENT_DEPTH + 1` segments is no item's.
async fn published_item_at(
    repos: &dyn Repos,
    rules: &dyn Permalinks,
    source: &str,
) -> Result<bool, CoreError> {
    if source.contains(['?', '#']) {
        return Ok(false);
    }
    let segments: Vec<String> = source
        .trim_matches('/')
        .split('/')
        .map(|s| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8_lossy()
                .into_owned()
        })
        .collect();
    if segments.iter().any(String::is_empty) || segments.len() > MAX_PARENT_DEPTH + 1 {
        return Ok(false);
    }
    let alternative = toggled(source);
    for ty in repos.types().list().await? {
        if ty.name == MEDIA_KIND {
            continue;
        }
        for skip in 0..segments.len() {
            let Some(item) = walk(repos, &ty.name, &segments[skip..]).await? else {
                continue;
            };
            if item.status != ContentStatus::Published {
                continue;
            }
            let path = rules.path(&load_input(repos, item).await?);
            if path == source || alternative.as_deref() == Some(path.as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The item of `kind` at the end of the slug chain `segments` from the root.
async fn walk(
    repos: &dyn Repos,
    kind: &str,
    segments: &[String],
) -> Result<Option<ContentItem>, CoreError> {
    let mut parent = None;
    let mut found = None;
    for segment in segments {
        found = repos.content().get_by_slug(kind, parent, segment).await?;
        match &found {
            Some(item) => parent = Some(item.id),
            None => return Ok(None),
        }
    }
    Ok(found)
}
