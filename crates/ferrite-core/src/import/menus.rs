//! Imported menus (P19).

use ferrite_domain::Menu;

use super::ImportBatch;
use crate::{CoreError, menu::put_in};

impl ImportBatch<'_> {
    /// Creates the menu or replaces its whole item tree, with the validation of
    /// [`MenuService::put`](crate::MenuService::put) (name, labels, at most 500 items,
    /// every item's target must exist). The menu's key joins the batch's event.
    ///
    /// **Errors.** As for [`content`](Self::content): any `Err` leaves the batch
    /// unusable, and the caller must [`rollback`](Self::rollback) it.
    pub async fn menu(&mut self, m: &Menu) -> Result<(), CoreError> {
        self.keys.extend(put_in(self.tx.as_ref(), m).await?);
        self.items += 1;
        Ok(())
    }
}
