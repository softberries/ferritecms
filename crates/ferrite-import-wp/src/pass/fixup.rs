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
    let channel = ir.channel().await?;
    let links = Links::load(session, &channel).await?;
    translations(session, &links, report).await?;
    let queue = ir.fixups().await?;
    if queue.is_empty() {
        return Ok(());
    }
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

/// Joins the imported members of each translation group the file describes
/// ([`crate::lang::groups`]) to the group of the member in the site's default locale
/// (else the first member), one batch per group. Counts (kind `translations`):
/// `updated` for an item moved into its group, `unchanged` when it was in it, `kept`
/// when it was edited here, `skipped` when the group already has an item in its
/// locale or a member was not imported.
async fn translations(
    session: &Session<'_>,
    links: &Links,
    report: &mut Report,
) -> Result<(), ImportError> {
    let ir = session.ir();
    let terms = ir.terms().await?;
    let mut items = Vec::new();
    {
        use futures::TryStreamExt;
        let mut stream = ir.items(&[]);
        while let Some(item) = stream.try_next().await? {
            if crate::lang::item_language(&item).is_some()
                || crate::pass::meta(&item, crate::lang::WPML_GROUP).is_some()
            {
                items.push(item);
            }
        }
    }
    let (groups, notes) = crate::lang::groups(&terms, &items);
    for note in notes {
        report.note(note);
    }
    for members in groups {
        let written: Vec<(i64, ContentId)> = members
            .iter()
            .filter_map(|wp| {
                links
                    .written
                    .get(wp)
                    .map(|(id, _)| (*wp, ContentId::from_uuid(*id)))
            })
            .collect();
        if written.len() < 2 {
            report.counts_of("translations").skipped += written.len() as u64;
            continue;
        }
        let mut batch = session.batch().await?;
        let joined = join(&mut batch, &written).await;
        match session.finish_batch(batch, joined).await {
            Ok(outcomes) => {
                for (wp_id, outcome) in outcomes {
                    let c = report.counts_of("translations");
                    match outcome {
                        Joined::Moved => c.updated += 1,
                        Joined::Already => c.unchanged += 1,
                        Joined::Kept => {
                            c.kept += 1;
                            report.note(format!(
                                "item {wp_id} was edited here; its translation group was kept"
                            ));
                        }
                        Joined::Refused(why) => {
                            c.skipped += 1;
                            report.note(format!(
                                "item {wp_id} not joined to its translations: {why}"
                            ));
                        }
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// What happened to one member of a group.
enum Joined {
    Moved,
    Already,
    Kept,
    Refused(String),
}

async fn join(
    b: &mut ImportBatch<'_>,
    members: &[(i64, ContentId)],
) -> Result<Vec<(i64, Joined)>, ImportError> {
    let default = b.locales().await?.default.tag;
    let mut root = None;
    for (_, id) in members {
        if let Some(item) = b.get_item(*id).await? {
            if item.locale == default {
                root = Some(item.translation_group);
                break;
            }
            root.get_or_insert(item.translation_group);
        }
    }
    let Some(group) = root else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (wp_id, id) in members {
        let outcome = match b.join_group(*id, *wp_id, group).await {
            Ok(true) => Joined::Moved,
            Ok(false) => Joined::Already,
            Err(CoreError::Conflict { .. }) => Joined::Kept,
            Err(CoreError::InUse(why)) => Joined::Refused(why),
            Err(CoreError::NotFound) => Joined::Refused("no longer this import's".into()),
            Err(e) => return Err(e.into()),
        };
        out.push((*wp_id, outcome));
    }
    Ok(out)
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
