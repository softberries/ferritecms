//! TEMPORARY scaffolding (M1a Task 10): repositories not implemented yet return
//! `StoreError::Unsupported`. Task 10 deletes the `SearchRepo` block and this file.

use async_trait::async_trait;
use ferrite_domain::ContentId;
use ferrite_store::{SearchRepo, StoreError};

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

stub!(SearchRepo {
    fn index(ContentId, &str, &str) -> ();
    fn remove(ContentId) -> ();
    fn query(&str, u32) -> Vec<ContentId>;
});
