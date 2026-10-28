//! The fix-up (P14): items whose links or relations named items written after them are
//! converted again now that every item exists, and their body and fields replaced
//! ([`ImportBatch::set_body`], [`ImportBatch::set_fields`]).
//!
//! Only items pass 3 created or updated are queued (in the IR, so a resumed run has the
//! queue). Each item gets a batch of its own: an item edited here since pass 3 wrote it is
//! refused by core (`Conflict`) and kept, with a note, without failing the others. A dry
//! run has nothing to fix (its items were rolled back).

use ferrite_core::CoreError;
use ferrite_core::import::ImportBatch;
use ferrite_domain::ContentId;
use serde_json::Value;

use super::content::{Links, convert};
use crate::ImportError;
use crate::acf::field_defs;
use crate::run::{Report, Session};

/// Fixes every queued item.
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
    for wp_id in queue {
        let (Some(item), Some((id, _))) = (ir.item(wp_id).await?, links.written.get(&wp_id)) else {
            continue;
        };
        let converted = convert(&item, &links, &acf);
        let id = ContentId::from_uuid(*id);
        let mut batch = session.batch().await?;
        let fixed = fix(&mut batch, id, wp_id, converted.body, converted.fields).await;
        match session.finish_batch(batch, fixed).await {
            Ok(()) => report.counts_of("fixups").updated += 1,
            Err(ImportError::Core(CoreError::Conflict { .. } | CoreError::NotFound)) => {
                report.counts_of("fixups").kept += 1;
                report.note(format!(
                    "item {wp_id} was edited here before its links were fixed; kept"
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
) -> Result<(), ImportError> {
    b.set_body(id, wp_id, body).await?;
    b.set_fields(id, wp_id, fields).await?;
    Ok(())
}
