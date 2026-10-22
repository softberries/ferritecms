//! TEMPORARY scaffolding (M1a Tasks 4–10): repositories not implemented yet return
//! `StoreError::Unsupported`. Each task deletes the `stub!` block of the traits it
//! implements; Task 10 deletes this file.

use async_trait::async_trait;
use ferrite_domain::{
    Artifact, ArtifactHash, ContentId, ContentItem, ContentQuery, ContentType, Event,
    InstalledPlugin, InstalledTheme, Media, Menu, NewContent, NewEvent, NewUser, OptionEntry, Page,
    Redirect, RedirectId, Revision, RevisionId, Taxonomy, Term, TermId, Timestamp, User, UserId,
};
use ferrite_store::{
    ArtifactRepo, ContentRepo, ContentTypeRepo, EventRepo, MediaRepo, MenuRepo, MetaRepo,
    OptionRepo, PluginKvRepo, PluginRepo, RedirectRepo, RevisionRepo, SearchRepo, StoreError,
    TaxonomyRepo, TermRepo, ThemeRepo, UserRepo,
};
use serde_json::Value;

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

stub!(ContentTypeRepo {
    fn upsert(&ContentType) -> ();
    fn get(&str) -> Option<ContentType>;
    fn list() -> Vec<ContentType>;
    fn delete(&str) -> ();
});

stub!(ContentRepo {
    fn insert(&NewContent) -> ContentItem;
    fn get(ContentId) -> Option<ContentItem>;
    fn get_by_slug(&str, Option<ContentId>, &str) -> Option<ContentItem>;
    fn update(&ContentItem) -> ContentItem;
    fn list(&ContentQuery) -> Page<ContentItem>;
    fn delete(ContentId) -> ();
    fn due_scheduled(Timestamp, u32) -> Vec<ContentItem>;
});

stub!(MetaRepo {
    fn get(ContentId, &str) -> Option<Value>;
    fn set(ContentId, &str, &Value) -> ();
    fn delete(ContentId, &str) -> ();
    fn list(ContentId) -> Vec<(String, Value)>;
});

stub!(RevisionRepo {
    fn insert(&Revision) -> ();
    fn list(ContentId, u32) -> Vec<Revision>;
    fn get(RevisionId) -> Option<Revision>;
    fn prune(ContentId, u32) -> u64;
});

stub!(TaxonomyRepo {
    fn upsert(&Taxonomy) -> ();
    fn get(&str) -> Option<Taxonomy>;
    fn list() -> Vec<Taxonomy>;
});

stub!(TermRepo {
    fn insert(&Term) -> ();
    fn update(&Term) -> ();
    fn get(TermId) -> Option<Term>;
    fn get_by_slug(&str, &str) -> Option<Term>;
    fn list(&str) -> Vec<Term>;
    fn delete(TermId) -> ();
    fn set_for_item(ContentId, &[TermId]) -> ();
    fn for_item(ContentId) -> Vec<Term>;
});

stub!(MenuRepo {
    fn get(&str) -> Option<Menu>;
    fn put(&Menu) -> ();
    fn list() -> Vec<(String, String)>;
    fn delete(&str) -> ();
});

stub!(OptionRepo {
    fn get(&str) -> Option<OptionEntry>;
    fn set(&str, &Value, bool) -> ();
    fn delete(&str) -> ();
    fn autoloaded() -> Vec<OptionEntry>;
});

stub!(RedirectRepo {
    fn get_by_source(&str) -> Option<Redirect>;
    fn upsert(&Redirect) -> ();
    fn delete(RedirectId) -> ();
    fn list(u32, Option<&str>) -> Page<Redirect>;
    fn hit(RedirectId) -> ();
});

stub!(UserRepo {
    fn insert(&NewUser) -> User;
    fn get(UserId) -> Option<User>;
    fn get_by_login(&str) -> Option<User>;
    fn get_by_email(&str) -> Option<User>;
    fn update(&User) -> ();
    fn list(u32, Option<&str>) -> Page<User>;
});

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
