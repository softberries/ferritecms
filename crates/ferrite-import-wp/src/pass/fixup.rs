//! The fix-up (P14): items whose links or relations named items written after them are
//! converted again now that every item exists, and their body and fields replaced in
//! one write ([`ImportBatch::set_body_and_fields`]).
//!
//! Pass 3 queues the items it created or updated (in the IR, before their batch
//! commits, so a resumed run has the queue). Each item gets a batch of its own: an item
//! edited here since pass 3 wrote it is refused by core (`Conflict`) and kept, with a
//! note, without failing the others. A dry run has nothing to fix (its items were
//! rolled back).

use std::collections::HashMap;

use ferrite_core::CoreError;
use ferrite_core::import::ImportBatch;
use ferrite_domain::ContentId;
use serde_json::Value;

use super::content::{Links, TypeAcf, convert};
use crate::ImportError;
use crate::acf::field_defs;
use crate::run::{Report, Session};

/// Fixes every queued item. Counts (kind `fixups`): `updated` for an item rewritten,
/// `unchanged` when nothing changed, `kept` when it was edited here, `skipped` when it
/// is gone.
pub(crate) async fn run(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    if session.opts().dry_run {
        return Ok(());
    }
    let ir = session.ir();
    let queue = ir.fixups().await?;
    if queue.is_empty() {
        return Ok(());
    }
    let channel = ir.channel().await?;
    let links = Links::load(session, &channel).await?;
    let acf = field_defs(ir).await?;
    let mut types: HashMap<String, TypeAcf<'_>> = HashMap::new();
    for wp_id in queue {
        let (Some(item), Some((id, _))) = (ir.item(wp_id).await?, links.written.get(&wp_id)) else {
            continue;
        };
        let type_acf = types
            .entry(item.post_type.clone())
            .or_insert_with(|| TypeAcf::of(&item.post_type, &acf));
        let converted = convert(&item, &links, type_acf);
        let id = ContentId::from_uuid(*id);
        let mut batch = session.batch().await?;
        let fixed = fix(&mut batch, id, wp_id, converted.body, converted.fields).await;
        match session.finish_batch(batch, fixed).await {
            Ok(true) => report.counts_of("fixups").updated += 1,
            Ok(false) => report.counts_of("fixups").unchanged += 1,
            Err(ImportError::Core(CoreError::Conflict { .. })) => {
                report.counts_of("fixups").kept += 1;
                report.note(format!(
                    "item {wp_id} was edited here before its links were fixed; kept as edited"
                ));
            }
            Err(ImportError::Core(CoreError::NotFound)) => {
                report.counts_of("fixups").skipped += 1;
                report.note(format!(
                    "item {wp_id} is no longer on the site (or no longer this import's); its links were not fixed"
                ));
            }
            Err(e) => return Err(e),
        }
        if converted.pending {
            report.note(format!(
                "item {wp_id} links to items of the file that were not imported; those links keep their old URLs"
            ));
        }
    }
    Ok(())
}

async fn fix(
    b: &mut ImportBatch<'_>,
    id: ContentId,
    wp_id: i64,
    body: Value,
    fields: Value,
) -> Result<bool, ImportError> {
    Ok(b.set_body_and_fields(id, wp_id, body, fields).await?)
}
