//! TEMPORARY scaffolding (M1a Tasks 9–10): repositories not implemented yet return
//! `StoreError::Unsupported`. Each task deletes the `stub!` block of the traits it
//! implements; Task 10 deletes this file.

use async_trait::async_trait;
use ferrite_domain::{ContentId, Event, NewEvent, Timestamp};
use ferrite_store::{EventRepo, SearchRepo, StoreError};

pub(crate) struct Unsupported;

macro_rules! stub {
    ($repo:ident { $(fn $method:ident($($arg:ty),*) -> $ret:ty;)* }) => {
        #[async_trait]
        impl $repo for Unsupported {
            $(async fn $method(&self, $(_: $arg),*) -> Result<$ret, StoreError> {
                Err(StoreError::Unsupported(concat!(
                    stringify!($repo), "::", stringify!($method)
                )))
            })*
        }
    };
}

stub!(EventRepo {
    fn append(&NewEvent) -> i64;
    fn since(i64, u32) -> Vec<Event>;
    fn max_seq() -> i64;
    fn prune(Timestamp) -> u64;
});

stub!(SearchRepo {
    fn index(ContentId, &str, &str) -> ();
    fn remove(ContentId) -> ();
    fn query(&str, u32) -> Vec<ContentId>;
});
