//! Site parts shared by `serve` and `seed`: the media store and the active theme.

use std::path::Path;

use anyhow::{Context, bail};
use ferrite_config::{Config, MediaSection};
use ferrite_core::PermalinkRules;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode, S3Config};
use ferrite_store::Store;
use ferrite_theme::{Theme, ThemeSource};

/// Path prefix under which the app serves media (`/_media/{hash}/{name}`).
const MEDIA_PATH: &str = "/_media";
/// The theme used when none is active, and the fallback when the active one fails.
pub(crate) const DEFAULT_THEME: &str = "blog";
/// Longest theme name accepted from the `themes` table.
const MAX_THEME_NAME: usize = 64;
/// Option holding the post permalink structure.
const POST_PERMALINK_OPTION: &str = "permalink.post";
/// Option holding the page permalink structure.
const PAGE_PERMALINK_OPTION: &str = "permalink.page";

/// The media store settings from `ferrite.toml`. Local media and S3 without
/// `public_base_url` are served by the app under `/_media`; S3 with it links there.
pub(crate) fn media_config(config: &Config) -> MediaConfig {
    let via_app = || PublicUrlMode::ServeViaApp {
        path_prefix: MEDIA_PATH.to_owned(),
    };
    match &config.media {
        MediaSection::Local { root, .. } => MediaConfig {
            backend: Backend::Local { root: root.clone() },
            prefix: String::new(),
            public: via_app(),
        },
        MediaSection::S3 {
            bucket,
            region,
            endpoint,
            access_key_id,
            secret_access_key,
            allow_http,
            virtual_hosted_style,
            prefix,
            public_base_url,
        } => MediaConfig {
            backend: Backend::S3(S3Config {
                bucket: bucket.clone(),
                region: region.clone(),
                endpoint: endpoint.clone(),
                access_key_id: access_key_id.clone(),
                secret_access_key: secret_access_key.clone(),
                allow_http: *allow_http,
                virtual_hosted_style: *virtual_hosted_style,
            }),
            prefix: prefix.clone(),
            public: public_base_url
                .as_ref()
                .map_or_else(via_app, |base| PublicUrlMode::Direct {
                    base_url: base.clone(),
                }),
        },
    }
}

/// Opens the configured media store (no probe; see [`MediaStore::check`]).
pub(crate) fn open_media(config: &Config) -> anyhow::Result<MediaStore> {
    MediaStore::open(media_config(config)).context("cannot open the media store")
}

/// Permalink rules from the options (defaults when unset), checked against the
/// registered content types and taxonomies (rulings R3, R13).
pub(crate) async fn permalink_rules(store: &dyn Store) -> anyhow::Result<PermalinkRules> {
    let repos = store.read();
    let mut structures = Vec::new();
    for key in [POST_PERMALINK_OPTION, PAGE_PERMALINK_OPTION] {
        let value = repos
            .options()
            .get(key)
            .await
            .with_context(|| format!("cannot read option {key}"))?;
        structures.push(match value {
            None => None,
            Some(entry) => match entry.value.as_str() {
                Some(s) => Some(s.to_owned()),
                None => bail!("option {key} must be a string"),
            },
        });
    }
    let rules = PermalinkRules::from_options(structures[0].as_deref(), structures[1].as_deref())
        .context("invalid permalink option")?;
    let types: Vec<String> = repos
        .types()
        .list()
        .await
        .context("cannot list content types")?
        .into_iter()
        .map(|t| t.name)
        .collect();
    let taxonomies: Vec<String> = repos
        .taxonomies()
        .list()
        .await
        .context("cannot list taxonomies")?
        .into_iter()
        .map(|t| t.name)
        .collect();
    rules
        .check_types(&types, &taxonomies)
        .context("invalid permalink option")?;
    Ok(rules)
}

/// The active theme (06 §7): its name from the `themes` table (default `blog`), loaded
/// from `<data>/themes/<name>/` if that exists, else the embedded starter theme of that
/// name. A theme that fails to load is logged and the embedded `blog` theme is used, so
/// a broken theme never takes the site down. Only a store error is an error.
pub(crate) async fn load_theme(store: &dyn Store, data_dir: &Path) -> anyhow::Result<Theme> {
    let name = store
        .read()
        .themes()
        .active()
        .await
        .context("cannot read the active theme")?
        .map_or_else(|| DEFAULT_THEME.to_owned(), |t| t.name);
    select_theme(&name, data_dir)
}

/// [`load_theme`] once the name is known.
fn select_theme(name: &str, data_dir: &Path) -> anyhow::Result<Theme> {
    match try_theme(name, data_dir) {
        Ok(theme) => Ok(theme),
        Err(e) => {
            tracing::error!(
                theme = %name,
                error = %format!("{e:#}"),
                "active theme failed to load; using the embedded `{DEFAULT_THEME}` theme"
            );
            Theme::load_embedded(DEFAULT_THEME).context("the embedded blog theme failed to load")
        }
    }
}

fn try_theme(name: &str, data_dir: &Path) -> anyhow::Result<Theme> {
    let valid = !name.is_empty()
        && name.len() <= MAX_THEME_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !valid {
        bail!("invalid theme name");
    }
    let dir = data_dir.join("themes").join(name);
    // A symlink counts as present: the loader refuses it rather than us skipping it.
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Theme::load(&ThemeSource::Dir(dir.clone()))
            .with_context(|| format!("theme folder {}", dir.display()));
    }
    Theme::load_embedded(name).context("no theme folder and no embedded theme of that name")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_theme(dir: &Path, name: &str, index: &str) {
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::write(
            dir.join("theme.toml"),
            format!("[theme]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("templates/index.html"), index).unwrap();
    }

    #[test]
    fn folder_wins_over_embedded() {
        let data = tempfile::tempdir().unwrap();
        // A folder named like the embedded theme replaces it.
        write_theme(&data.path().join("themes/blog"), "blog", "custom");
        let t = select_theme("blog", data.path()).unwrap();
        assert!(t.has_template("index.html"));
        assert!(
            !t.has_template("single.html"),
            "the folder theme, not the embedded one"
        );
        write_theme(&data.path().join("themes/mine"), "mine", "mine");
        assert_eq!(select_theme("mine", data.path()).unwrap().name(), "mine");
    }

    #[test]
    fn embedded_when_no_folder() {
        let data = tempfile::tempdir().unwrap();
        let t = select_theme("blog", data.path()).unwrap();
        assert_eq!(t.name(), "blog");
        assert!(t.has_template("single.html"));
    }

    #[test]
    fn broken_unknown_or_invalid_falls_back_to_blog() {
        let data = tempfile::tempdir().unwrap();
        write_theme(&data.path().join("themes/broken"), "broken", "{% if %}");
        for name in ["broken", "missing", "../etc", "", "UPPER", "a/b"] {
            let t = select_theme(name, data.path()).unwrap();
            assert_eq!(t.name(), "blog", "{name:?}");
            assert!(t.has_template("single.html"), "{name:?}");
        }
    }

    #[test]
    fn media_config_maps_backends() {
        let data = tempfile::tempdir().unwrap();
        std::fs::write(
            data.path().join("ferrite.toml"),
            "[site]\nurl = \"https://example.com\"\n",
        )
        .unwrap();
        let config = ferrite_config::load(data.path(), Vec::new()).unwrap();
        let m = media_config(&config);
        assert!(
            matches!(m.backend, Backend::Local { ref root } if root == &data.path().join("media"))
        );
        assert!(
            matches!(m.public, PublicUrlMode::ServeViaApp { ref path_prefix } if path_prefix == "/_media")
        );
        std::fs::write(
            data.path().join("ferrite.toml"),
            "[site]\nurl = \"https://example.com\"\n[media]\nbackend = \"s3\"\n\
             [media.s3]\nbucket = \"b\"\nregion = \"r\"\nprefix = \"p/\"\n\
             public_base_url = \"https://cdn.example.com\"\n",
        )
        .unwrap();
        let config = ferrite_config::load(data.path(), Vec::new()).unwrap();
        let m = media_config(&config);
        assert!(matches!(m.backend, Backend::S3(ref s) if s.bucket == "b" && s.region == "r"));
        assert_eq!(m.prefix, "p/");
        assert!(
            matches!(m.public, PublicUrlMode::Direct { ref base_url } if base_url.as_str() == "https://cdn.example.com/")
        );
    }
}
