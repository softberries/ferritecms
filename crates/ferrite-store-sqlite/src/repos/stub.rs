//! TEMPORARY scaffolding (M1a Tasks 4–10): repositories not implemented yet return
//! `StoreError::Unsupported`. Each task deletes the `stub!` block of the traits it
//! implements; Task 10 deletes this file.

use async_trait::async_trait;
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, Event, InstalledPlugin, InstalledTheme, Media, NewEvent,
    Timestamp,
};
use ferrite_store::{
    ArtifactRepo, EventRepo, MediaRepo, PluginKvRepo, PluginRepo, SearchRepo, StoreError, ThemeRepo,
};

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

stub!(ArtifactRepo {
    fn insert_if_absent(&Artifact) -> bool;
    fn get(&ArtifactHash) -> Option<Artifact>;
    fn delete(&ArtifactHash) -> ();
});

stub!(MediaRepo {
    fn insert(&Media) -> ();
    fn get(ContentId) -> Option<Media>;
    fn update(&Media) -> ();
});

stub!(PluginRepo {
    fn upsert(&InstalledPlugin) -> ();
    fn get(&str) -> Option<InstalledPlugin>;
    fn list() -> Vec<InstalledPlugin>;
    fn set_enabled(&str, bool, Option<&str>) -> ();
    fn delete(&str) -> ();
});

stub!(PluginKvRepo {
    fn get(&str, &str) -> Option<Vec<u8>>;
    fn set(&str, &str, &[u8]) -> ();
    fn delete(&str, &str) -> ();
    fn list_keys(&str, &str, u32) -> Vec<String>;
    fn usage(&str) -> (u64, u64);
});

stub!(ThemeRepo {
    fn upsert(&InstalledTheme) -> ();
    fn get(&str) -> Option<InstalledTheme>;
    fn list() -> Vec<InstalledTheme>;
    fn active() -> Option<InstalledTheme>;
    fn activate(&str) -> ();
});

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
