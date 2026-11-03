//! AI change sets (design 17): every write an assistant makes over MCP is recorded with
//! before- and after-images, grouped per connection and time window, and can be
//! reviewed and reverted by a person.

mod capture;
mod object;
mod restore;
mod service;
mod snapshot;

pub(crate) use capture::Capture;
pub use capture::{IDLE_WINDOW_MICROS, MAX_SET_CHANGES};
pub(crate) use object::{ObjectRef, Op};
pub use service::{
    ChangeDetail, ChangeSetDetail, ChangeSetFilter, ChangeSetService, ConflictReason, Images,
    MAX_REVERT_CHANGES, RevertConflict, RevertError, RevertReport, RevertRequest, is_open,
};
pub use snapshot::{MAX_IMAGE, canonical, open_image};
