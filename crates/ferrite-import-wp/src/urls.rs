//! Old attachment URLs → the new media (P14).
//!
//! [`MediaIndex`] maps every URL form of an imported attachment to its media item: the
//! `<wp:attachment_url>`, each size file of `_wp_attachment_metadata` (`sizes.*.file`, in
//! the same folder) and, for a big image WordPress scaled down, the `original_image`
//! beside the `-scaled` file. Keys are host-insensitive paths: the part from
//! `/wp-content/uploads/` on when the path has it (so a CDN host or a site moved to a
//! sub-folder still matches), else the whole path; percent-decoded, without query or
//! fragment. Pass 2 builds it and keeps it in the IR, so a resumed run has it.

use std::collections::HashMap;

use uuid::Uuid;

use crate::ImportError;
use crate::ir::Ir;
use crate::php::PhpValue;

/// Where WordPress keeps uploads; keys start here when a path has it.
const UPLOADS: &str = "/wp-content/uploads/";

/// The media item an old URL now names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRef {
    pub id: Uuid,
    /// The media's public URL (`MediaStore::public_url`).
    pub url: String,
}

/// See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaIndex {
    by_path: HashMap<String, MediaRef>,
}

impl MediaIndex {
    /// The index pass 2 stored in `ir` (empty before pass 2).
    pub async fn load(ir: &Ir) -> Result<MediaIndex, ImportError> {
        let by_path = ir
            .media_urls()
            .await?
            .into_iter()
            .map(|(path, id, url)| (path, MediaRef { id, url }))
            .collect();
        Ok(MediaIndex { by_path })
    }

    /// Stores the index in `ir`, replacing what was there.
    pub(crate) async fn save(&self, ir: &Ir) -> Result<(), ImportError> {
        let rows: Vec<(String, Uuid, String)> = self
            .by_path
            .iter()
            .map(|(path, r)| (path.clone(), r.id, r.url.clone()))
            .collect();
        ir.set_media_urls(&rows).await
    }

    /// Adds every URL form of one attachment (see [`forms`]).
    pub fn insert(&mut self, attachment_url: &str, metadata: Option<&PhpValue>, media: MediaRef) {
        for key in forms(attachment_url, metadata) {
            self.by_path.insert(key, media.clone());
        }
    }

    /// The media an old URL (absolute, protocol-relative or a root-relative path) names.
    pub fn resolve(&self, url: &str) -> Option<&MediaRef> {
        self.by_path.get(&key(url)?)
    }

    /// How many URL forms are known.
    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

/// The keys of an attachment's URL forms: `attachment_url`, then every size file and the
/// `original_image` of `metadata`, in the folder of `attachment_url`. Empty when the URL
/// has no path.
pub fn forms(attachment_url: &str, metadata: Option<&PhpValue>) -> Vec<String> {
    let Some(main) = key(attachment_url) else {
        return Vec::new();
    };
    let folder = match main.rfind('/') {
        Some(i) => main[..=i].to_owned(),
        None => String::new(),
    };
    let mut keys = vec![main];
    if let Some(meta) = metadata {
        let sizes = meta
            .get("sizes")
            .map(PhpValue::entries)
            .unwrap_or_default()
            .iter()
            .filter_map(|(_, size)| size.get("file"));
        for file in sizes.chain(meta.get("original_image")) {
            let Some(name) = file.as_text().filter(|n| !n.is_empty() && !n.contains('/')) else {
                continue;
            };
            let k = format!("{folder}{}", percent_decode(&name));
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    keys
}

/// The index key of `url` (see the module docs); `None` without a path.
pub fn key(url: &str) -> Option<String> {
    let url = url.trim();
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let path = match url.find("//") {
        // `scheme://host/path` or `//host/path`: the path starts after the host.
        Some(i)
            if url[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.:".contains(c)) =>
        {
            let rest = &url[i + 2..];
            &rest[rest.find('/')?..]
        }
        _ if url.starts_with('/') => url,
        _ => return None,
    };
    let path = percent_decode(path);
    let key = match path.find(UPLOADS) {
        Some(i) => path[i..].to_owned(),
        None => path,
    };
    (key.len() > 1).then_some(key)
}

/// Decodes `%XX` escapes; a string that does not decode to UTF-8 is kept as it is.
pub fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::php::unserialize;

    fn media(n: u128) -> MediaRef {
        MediaRef {
            id: Uuid::from_u128(n),
            url: format!("/_media/{n}"),
        }
    }

    #[test]
    fn keys_ignore_host_query_and_escapes() {
        let want = Some("/wp-content/uploads/2026/10/a b.png".to_owned());
        for url in [
            "http://wp.test/wp-content/uploads/2026/10/a%20b.png",
            "HTTPS://CDN.test/blog/wp-content/uploads/2026/10/a b.png?ver=1#top",
            "//wp.test/wp-content/uploads/2026/10/a%20b.png",
            " /wp-content/uploads/2026/10/a%20b.png ",
        ] {
            assert_eq!(key(url), want, "{url}");
        }
        assert_eq!(
            key("http://wp.test/files/x.png").as_deref(),
            Some("/files/x.png")
        );
        for none in [
            "",
            "x.png",
            "mailto:a@b.test",
            "http://wp.test",
            "http://wp.test/",
        ] {
            assert_eq!(key(none), None, "{none}");
        }
    }

    #[test]
    fn percent_decoding_keeps_what_is_not_utf8() {
        assert_eq!(percent_decode("%d0%b0%d0%b1"), "аб");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%ff"), "%ff");
    }

    /// A big image: WordPress stores `-scaled` as the file and the upload as
    /// `original_image`; sizes are named after the original.
    #[test]
    fn scaled_images_resolve_by_every_name() {
        let meta = unserialize(
            r#"a:3:{s:4:"file";s:24:"2024/03/photo-scaled.jpg";s:5:"sizes";a:2:{s:6:"medium";a:1:{s:4:"file";s:17:"photo-300x200.jpg";}s:5:"large";a:1:{s:4:"file";s:18:"photo-1024x683.jpg";}}s:14:"original_image";s:9:"photo.jpg";}"#,
        )
        .unwrap();
        let mut index = MediaIndex::default();
        index.insert(
            "http://wp.test/wp-content/uploads/2024/03/photo-scaled.jpg",
            Some(&meta),
            media(1),
        );
        index.insert(
            "http://wp.test/wp-content/uploads/2024/03/doc.pdf",
            None,
            media(2),
        );
        for name in [
            "photo-scaled.jpg",
            "photo.jpg",
            "photo-300x200.jpg",
            "photo-1024x683.jpg",
        ] {
            let url = format!("https://wp.test/wp-content/uploads/2024/03/{name}");
            assert_eq!(index.resolve(&url), Some(&media(1)), "{name}");
        }
        assert_eq!(
            index.resolve("/wp-content/uploads/2024/03/doc.pdf"),
            Some(&media(2))
        );
        assert_eq!(
            index.resolve("/wp-content/uploads/2024/03/photo-1x1.jpg"),
            None
        );
        assert_eq!(index.len(), 5);
    }

    #[test]
    fn size_files_with_a_folder_are_ignored() {
        let meta =
            unserialize(r#"a:1:{s:5:"sizes";a:1:{s:1:"x";a:1:{s:4:"file";s:8:"../a.png";}}}"#)
                .unwrap();
        assert_eq!(
            forms("http://wp.test/wp-content/uploads/b.png", Some(&meta)),
            ["/wp-content/uploads/b.png"]
        );
    }
}
