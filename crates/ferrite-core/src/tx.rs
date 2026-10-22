//! Shared transaction helper for services.

use crate::CoreError;

/// Commits on success; on error rolls back explicitly, so no connection is left with an
/// open transaction.
pub(crate) async fn finish<T>(
    tx: Box<dyn ferrite_store::Tx>,
    result: Result<T, CoreError>,
) -> Result<T, CoreError> {
    match result {
        Ok(v) => {
            tx.commit().await?;
            Ok(v)
        }
        Err(e) => {
            if let Err(rollback) = tx.rollback().await {
                tracing::warn!(error = %rollback, "rollback after failed transaction failed");
            }
            Err(e)
        }
    }
}
