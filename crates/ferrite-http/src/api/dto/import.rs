//! Bodies of the `/import` endpoints (M3-2 P25, P26-1). A run never shows its fencing
//! token (core's reads leave it out); a job never shows its payload.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ferrite_core::{
    import::{ImportJobView, ImportRunView},
    jobs::ImportJobOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

/// The multipart body of `POST /import/wordpress`.
#[derive(ToSchema)]
pub struct ImportStartUpload {
    /// The WordPress export (Tools → Export), at most 512 MiB; stored as sent (not
    /// sniffed). Its part's file name names the run in the admin.
    #[schema(value_type = String, format = Binary)]
    pub wxr: Vec<u8>,
    /// A `.zip` of the old site's `wp-content/uploads`, at most 4 GiB.
    #[schema(value_type = Option<String>, format = Binary)]
    pub uploads: Option<Vec<u8>>,
    /// At most 4 KiB of JSON; every field is optional.
    pub options: Option<ImportOptionsDto>,
}

/// The import's options; a missing field takes the CLI's default.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImportOptionsDto {
    /// Run everything, then roll every write back. Default `false`.
    pub dry_run: bool,
    /// Take the old site's post permalink structure. Default `true`.
    pub keep_permalinks: bool,
    /// Import the site title and description. Default `true`.
    pub site_options: bool,
    /// Fetch attachments missing from `uploads` from the old site. Default `false`.
    pub fetch_media: bool,
    /// The old site's category base, for redirects. Default `category`.
    pub category_base: String,
    /// The old site's tag base, for redirects. Default `tag`.
    pub tag_base: String,
}

impl Default for ImportOptionsDto {
    fn default() -> Self {
        ImportJobOptions::default().into()
    }
}

impl From<ImportJobOptions> for ImportOptionsDto {
    fn from(o: ImportJobOptions) -> Self {
        Self {
            dry_run: o.dry_run,
            keep_permalinks: o.keep_permalinks,
            site_options: o.site_options,
            fetch_media: o.fetch_media,
            category_base: o.category_base,
            tag_base: o.tag_base,
        }
    }
}

impl From<ImportOptionsDto> for ImportJobOptions {
    fn from(o: ImportOptionsDto) -> Self {
        Self {
            dry_run: o.dry_run,
            keep_permalinks: o.keep_permalinks,
            site_options: o.site_options,
            fetch_media: o.fetch_media,
            category_base: o.category_base,
            tag_base: o.tag_base,
        }
    }
}

/// An import job (`GET /import/jobs/{job_id}`): poll it until `status` is `done` or
/// `dead`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportJobDto {
    pub job_id: Uuid,
    /// `queued`, `running`, `done` or `dead`.
    pub status: String,
    /// Claims so far (at most 3).
    pub attempts: i32,
    /// Why the last attempt failed, scrubbed (the node's log has the full text).
    pub last_error: Option<String>,
    /// The run the job started; `null` until it starts one.
    pub run: Option<ImportRunDto>,
}

impl From<ImportJobView> for ImportJobDto {
    fn from(v: ImportJobView) -> Self {
        Self {
            job_id: v.job.id.as_uuid(),
            status: v.job.status.as_str().to_owned(),
            attempts: v.job.attempts,
            last_error: v.job.last_error,
            run: v.run.map(ImportRunDto::from),
        }
    }
}

/// What happened to the items of one kind.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ImportCountsDto {
    pub created: u64,
    pub updated: u64,
    pub unchanged: u64,
    /// Edited here since the last import: kept.
    pub kept: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// An import run.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportRunDto {
    pub id: Uuid,
    /// The old site's address (the WXR's `base_blog_url`).
    pub source: String,
    /// `running`, `done`, `failed` or `abandoned`.
    pub state: String,
    /// The step it is in or ended in: `parse`, `pass1` … `pass4`, `fixup` or `report`.
    pub step: Option<String>,
    pub dry_run: bool,
    pub started_by: Uuid,
    pub started_at: DateTime<Utc>,
    /// Moves while it runs; a `running` run without one for 120 s has stopped.
    pub heartbeat_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Outcomes per kind (`posts`, `pages`, `media`, …), as far as the run got.
    pub counts: BTreeMap<String, ImportCountsDto>,
    /// Why a failed run failed.
    pub error: Option<String>,
    /// The import job that ran it; `null` for a CLI run.
    pub job_id: Option<Uuid>,
    /// `report.json` and `report.html` can be read.
    pub report: bool,
}

impl From<ImportRunView> for ImportRunDto {
    fn from(r: ImportRunView) -> Self {
        let mut counts = BTreeMap::new();
        let mut error = None;
        if let Value::Object(stats) = r.stats {
            for (key, value) in stats {
                if key == "error" {
                    error = value.as_str().map(str::to_owned);
                } else if let Ok(c) = serde_json::from_value::<ImportCountsDto>(value) {
                    counts.insert(key, c);
                }
            }
        }
        Self {
            id: r.id.as_uuid(),
            source: r.source,
            state: r.state.as_str().to_owned(),
            step: r.step,
            dry_run: r.dry_run,
            started_by: r.started_by.as_uuid(),
            started_at: r.started_at.as_datetime(),
            heartbeat_at: r.heartbeat_at.as_datetime(),
            finished_at: r.finished_at.map(|t| t.as_datetime()),
            counts,
            error,
            job_id: r.job_id.map(|j| j.as_uuid()),
            report: r.report_hash.is_some(),
        }
    }
}

/// `GET /import/runs`: one page, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportRunPageDto {
    pub items: Vec<ImportRunDto>,
    pub next_cursor: Option<String>,
}
