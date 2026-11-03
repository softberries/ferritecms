//! Releases (design 15 §4–§5): named sets of staged changes applied in one transaction,
//! now or on a schedule, and the sandbox the whole-site preview renders through.

mod sandbox;
mod tx;

pub use sandbox::{SandboxReport, SandboxStore};
pub use tx::{ApplyTx, MAX_RELEASE_KEYS, Mode};
