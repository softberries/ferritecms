//! Every body in the corpus converts to a block tree the admin editor opens: sites A and B,
//! and the WordPress theme-test files when `FERRITE_WP_CORPUS` names their directory (P15).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use ferrite_import_wp::convert::{ImageRef, Notes, Rewrite, body};
use ferrite_import_wp::model::Record;
use uuid::Uuid;

/// Every upload is media; every attachment id resolves.
struct Rw;

impl Rewrite for Rw {
    fn image(&self, src: &str) -> ImageRef {
        if src.contains("/wp-content/uploads/") {
            ImageRef::Media(Uuid::from_u128(1))
        } else {
            ImageRef::Url(src.to_owned())
        }
    }

    fn attachment(&self, wp_id: i64) -> Option<Uuid> {
        u128::try_from(wp_id).ok().map(Uuid::from_u128)
    }

    fn href(&self, href: &str) -> String {
        href.to_owned()
    }
}

/// Converts every item body in `file`; returns how many and the notes of all of them.
fn convert_all(file: &Path) -> (usize, Notes) {
    let bytes = std::fs::read(file).unwrap();
    let mut total = Notes::default();
    let mut bodies = 0;
    ferrite_import_wp::wxr::parse(&bytes[..], |r| {
        if let Record::Item(i) = r {
            let mut notes = Notes {
                item: Some(i.wp_id),
                ..Notes::default()
            };
            let doc = body::convert(&i.content, &Rw, &mut notes);
            assert!(
                ferrite_core::blocks::is_editor_safe(&doc),
                "{} item {}: {doc}",
                file.display(),
                i.wp_id
            );
            assert!(
                !doc.to_string().contains("\\u0000"),
                "{} item {}",
                file.display(),
                i.wp_id
            );
            total.merge(&notes);
            bodies += 1;
        }
        Ok(())
    })
    .unwrap();
    (bodies, total)
}

fn summary(file: &Path, bodies: usize, notes: &Notes) {
    println!("{}: {bodies} bodies", file.display());
    for (what, map) in [
        ("unknown block", &notes.unknown_blocks),
        ("unknown shortcode", &notes.unknown_shortcodes),
        ("unsupported tag", &notes.unsupported_tags),
        ("dropped mark", &notes.dropped_marks),
    ] {
        for (name, t) in map {
            println!("  {what} {name}: {} (e.g. {:?})", t.count, t.items);
        }
    }
    println!(
        "  bad block attrs: {}, h1 demoted: {}, missing media: {}, too deep: {}, page breaks: {}",
        notes.bad_block_attrs.count,
        notes.h1_demoted.count,
        notes.missing_media.count,
        notes.too_deep.count,
        notes.page_breaks.count
    );
}

#[test]
fn synthetic_sites_convert_editor_safe() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    for file in ["site-a.xml", "site-b.xml"] {
        let path = dir.join(file);
        let (bodies, notes) = convert_all(&path);
        assert!(bodies > 5);
        summary(&path, bodies, &notes);
    }
}

#[test]
fn theme_test_files_convert_editor_safe() {
    let Some(dir) = std::env::var_os("FERRITE_WP_CORPUS").map(PathBuf::from) else {
        println!("SKIP: FERRITE_WP_CORPUS is not set");
        return;
    };
    for file in ["themeunittestdata.wordpress.xml", "64-block-test-data.xml"] {
        let path = dir.join(file);
        let (bodies, notes) = convert_all(&path);
        assert!(bodies > 50, "{}: {bodies} bodies", path.display());
        summary(&path, bodies, &notes);
    }
}
