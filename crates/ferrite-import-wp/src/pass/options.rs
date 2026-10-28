//! Site options from the channel (pass 4, P20): `<title>` → `site.title`,
//! `<description>` → `site.description`, unless the run was told not to
//! (`ImportOptions::site_options`). Nothing else: WXR carries no other options.

use ferrite_core::import::ImportBatch;
use serde_json::{Value, json};

use crate::ImportError;
use crate::run::{Report, Session};

/// Writes the site title and description of the channel.
pub(crate) async fn import(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    if !session.opts().site_options {
        return Ok(());
    }
    let channel = session.ir().channel().await?;
    let mut rows: Vec<(String, Option<Value>)> = Vec::new();
    for (key, value) in [
        ("site.title", channel.title.trim()),
        ("site.description", channel.description.trim()),
    ] {
        if !value.is_empty() {
            rows.push((key.to_owned(), Some(json!(value))));
        }
    }
    if rows.is_empty() {
        return Ok(());
    }
    let mut batch = session.batch().await?;
    let written = write(&mut batch, &rows).await;
    session.finish_batch(batch, written).await?;
    report.note(format!(
        "site options set from the file: {}",
        rows.iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    Ok(())
}

async fn write(
    b: &mut ImportBatch<'_>,
    rows: &[(String, Option<Value>)],
) -> Result<(), ImportError> {
    Ok(b.options(rows).await?)
}
