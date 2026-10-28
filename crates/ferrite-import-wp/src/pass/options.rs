//! Site options from the channel (pass 4, P20): `<title>` → `site.title`,
//! `<description>` → `site.description`, unless the run was told not to
//! (`ImportOptions::site_options`); WXR carries no other options. [`apply`] is the P4
//! rule for every option the import writes, `permalink.post` (P17) included.
//!
//! The import map (kind `option`, a fixed id per key) keeps the SHA-256 of the value the
//! import last wrote. On a re-run an option whose WXR value is unchanged is left alone;
//! a changed one is written only while the site still holds what the import wrote, else
//! the site's value is kept and noted. On the first import every option is written, but
//! `permalink.post` only while the site still has core's default structure (a structure
//! change re-paths every item at once: paths are not stored but computed from it).

use ferrite_core::import::ImportBatch;
use ferrite_core::permalink::{DEFAULT_POST, POST_OPTION};
use ferrite_domain::ImportMapKind;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ImportError;
use crate::run::{Report, Session};

/// The report kind of options.
const KIND: &str = "options";
/// The options the import writes and their map ids.
const IDS: [(&str, i64); 3] = [("site.title", 1), ("site.description", 2), (POST_OPTION, 3)];

/// What [`apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    Created,
    Updated,
    Unchanged,
    /// The site's value was kept (noted).
    Kept,
}

/// Writes the site title and description of the channel.
pub(crate) async fn import(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    if !session.opts().site_options {
        return Ok(());
    }
    let channel = session.ir().channel().await?;
    let rows: Vec<(&str, Value)> = [
        ("site.title", channel.title.trim()),
        ("site.description", channel.description.trim()),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| (k, json!(v)))
    .collect();
    if rows.is_empty() {
        return Ok(());
    }
    let mut batch = session.batch().await?;
    let mut part = Report::default();
    let written = write(&mut batch, &rows, &mut part).await;
    session.finish_batch(batch, written).await?;
    merge(report, part);
    Ok(())
}

async fn write(
    b: &mut ImportBatch<'_>,
    rows: &[(&str, Value)],
    report: &mut Report,
) -> Result<(), ImportError> {
    for (key, value) in rows {
        apply(b, key, value, report).await?;
    }
    Ok(())
}

/// Adds `part`'s option counts and notes to `report`.
pub(crate) fn merge(report: &mut Report, part: Report) {
    if let Some(c) = part.counts.get(KIND) {
        let counts = report.counts_of(KIND);
        counts.created += c.created;
        counts.updated += c.updated;
        counts.unchanged += c.unchanged;
        counts.kept += c.kept;
    }
    report.notes_from(&part);
}

/// Writes option `key` = `value` under the module's P4 rule, counting and noting the
/// outcome in `report`. `permalink.*` needs a schema batch.
pub(crate) async fn apply(
    b: &mut ImportBatch<'_>,
    key: &str,
    value: &Value,
    report: &mut Report,
) -> Result<Applied, ImportError> {
    let Some(&(_, id)) = IDS.iter().find(|(k, _)| *k == key) else {
        return Err(ImportError::Missing {
            what: "a map id for this option",
            offset: 0,
        });
    };
    let wanted = hash(value);
    let current = b.get_option(key).await?;
    let applied = match b.lookup(ImportMapKind::Option, id).await? {
        None if key == POST_OPTION
            && current
                .as_ref()
                .is_some_and(|c| c.as_str() != Some(DEFAULT_POST)) =>
        {
            report.note(format!(
                "{key}: the site has its own structure; kept (the old site's was {value})"
            ));
            Applied::Kept
        }
        None => Applied::Created,
        Some(m) if m.hash == wanted => Applied::Unchanged,
        Some(m) if current.as_ref().map(hash).as_deref() == Some(m.hash.as_str()) => {
            Applied::Updated
        }
        Some(_) => {
            report.note(format!(
                "{key} was changed here since the last import; kept (WordPress now has {value})"
            ));
            Applied::Kept
        }
    };
    if matches!(applied, Applied::Created | Applied::Updated) {
        b.options(&[(key.to_owned(), Some(value.clone()))]).await?;
        b.remember(ImportMapKind::Option, id, Uuid::nil(), &wanted, 0)
            .await?;
    }
    let counts = report.counts_of(KIND);
    match applied {
        Applied::Created => counts.created += 1,
        Applied::Updated => counts.updated += 1,
        Applied::Unchanged => counts.unchanged += 1,
        Applied::Kept => counts.kept += 1,
    }
    Ok(applied)
}

/// The SHA-256 (hex) of a JSON value.
fn hash(value: &Value) -> String {
    Sha256::digest(value.to_string().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
