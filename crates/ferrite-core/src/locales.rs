//! The site's locales (design 13 §2.2): read from the `site.locales` option.

use ferrite_domain::locale::{LOCALES_OPTION, SiteLocales, bcp47_loose};
use ferrite_store::Repos;

use crate::CoreError;

/// The option holding the one-locale site's language (`<html lang>`, feeds).
pub const SITE_LOCALE_OPTION: &str = "site.locale";

/// The site's locales: the `site.locales` option, else (a database `settle` has not
/// seen yet, or an unreadable value) one locale from the `site.locale` option, else
/// `en`.
pub async fn read(repos: &dyn Repos) -> Result<SiteLocales, CoreError> {
    let options = repos.options();
    if let Some(entry) = options.get(LOCALES_OPTION).await? {
        match SiteLocales::from_option(&entry.value) {
            Ok(locales) => return Ok(locales),
            Err(e) => tracing::warn!(error = %e, "site.locales is unreadable; using site.locale"),
        }
    }
    let locale = options
        .get(SITE_LOCALE_OPTION)
        .await?
        .and_then(|e| e.value.as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok(SiteLocales::single(bcp47_loose(&locale)))
}
