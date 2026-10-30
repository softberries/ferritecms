//! The folder scan over the fixture tree and over trees made at test time (P62).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use ferrite_import_md::{FORMAT, MAX_FILE_BYTES, MdDoc, MdError, Problem, Target, resolve, scan};
use sha2::{Digest, Sha256};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/site")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn doc<'a>(docs: &'a [MdDoc], rel: &str) -> &'a MdDoc {
    docs.iter()
        .find(|d| d.rel == rel)
        .unwrap_or_else(|| panic!("no doc {rel}"))
}

/// Every parent is listed before its children, and exists.
fn assert_parents_first(docs: &[MdDoc]) {
    for (i, d) in docs.iter().enumerate() {
        if let Some(p) = &d.parent {
            let at = docs
                .iter()
                .position(|x| &x.rel == p)
                .unwrap_or_else(|| panic!("parent {p} of {} missing", d.rel));
            assert!(at < i, "{p} after its child {}", d.rel);
        }
    }
}

#[test]
fn fixture_tree() {
    let s = scan(&fixture()).unwrap();
    let rels: Vec<&str> = s.docs.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(
        rels,
        [
            "index.md",
            "01-install.md",
            "02-configure.md",
            "03-reference",
            "03-reference/api.md",
            "guides/README.md",
            "guides/writing.md",
        ]
    );
    assert_parents_first(&s.docs);

    let home = doc(&s.docs, "index.md");
    assert_eq!(
        (home.parent.as_deref(), home.slug.as_str(), home.order),
        (None, "home", 0)
    );
    assert_eq!(home.front.title.as_deref(), Some("Welcome"));
    assert_eq!(home.front.description.as_deref(), Some("The docs."));
    assert_eq!(
        home.source.as_deref(),
        Some("# FerriteCMS docs\n\nStart with [installing](01-install.md).\n")
    );
    assert_eq!(home.fallback_title, "Home");

    let install = doc(&s.docs, "01-install.md");
    assert_eq!(
        (
            install.parent.as_deref(),
            install.slug.as_str(),
            install.order
        ),
        (None, "install", 1)
    );
    assert_eq!(install.fallback_title, "Install");
    let bytes = fs::read(fixture().join("01-install.md")).unwrap();
    let mut h = Sha256::new();
    h.update(&bytes);
    h.update(FORMAT.as_bytes());
    assert_eq!(install.hash, hex(&h.finalize()));

    let configure = doc(&s.docs, "02-configure.md");
    assert_eq!((configure.slug.as_str(), configure.order), ("configure", 2));

    // A folder without an index: a synthesized page, reported.
    let reference = doc(&s.docs, "03-reference");
    assert_eq!(
        (
            reference.parent.as_deref(),
            reference.slug.as_str(),
            reference.order
        ),
        (None, "reference", 3)
    );
    assert_eq!(reference.source, None);
    assert_eq!(reference.front, Default::default());
    assert_eq!(reference.fallback_title, "Reference");
    assert_eq!(
        reference.hash,
        hex(&Sha256::digest(format!("{FORMAT}03-reference").as_bytes()))
    );
    let api = doc(&s.docs, "03-reference/api.md");
    assert_eq!(api.parent.as_deref(), Some("03-reference"));
    assert_eq!((api.slug.as_str(), api.order), ("api", 0));

    // README.md is the folder's page; front matter `order` and `slug` win.
    let guides = doc(&s.docs, "guides/README.md");
    assert_eq!(
        (guides.parent.as_deref(), guides.slug.as_str(), guides.order),
        (None, "guides", 4)
    );
    assert_eq!(guides.fallback_title, "Guides");
    let writing = doc(&s.docs, "guides/writing.md");
    assert_eq!(writing.parent.as_deref(), Some("guides/README.md"));
    assert_eq!((writing.slug.as_str(), writing.order), ("write", 0));
    assert!(writing.front.draft);

    assert_eq!(
        s.problems,
        [
            Problem::SynthesizedSection {
                dir: "03-reference".into()
            },
            Problem::BadFrontMatter {
                file: "bad.md".into(),
                line: 1,
                message: s
                    .problems
                    .iter()
                    .find_map(|p| match p {
                        Problem::BadFrontMatter { message, .. } => Some(message.clone()),
                        _ => None,
                    })
                    .unwrap(),
            },
        ]
    );
    // Hidden files and folders are skipped without a report; so is a folder without
    // Markdown.
    assert!(
        !rels
            .iter()
            .any(|r| r.contains("hidden") || r.contains("drafts"))
    );
    assert!(!rels.iter().any(|r| r.starts_with("assets")));
}

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn too_large_and_not_utf8_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let limit = usize::try_from(MAX_FILE_BYTES).unwrap();
    write(dir.path(), "big.md", &vec![b'a'; limit + 1]);
    write(dir.path(), "edge.md", &vec![b'a'; limit]);
    write(dir.path(), "latin1.md", b"caf\xe9\n");
    let s = scan(dir.path()).unwrap();
    let rels: Vec<&str> = s.docs.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(rels, ["edge.md"]);
    assert_eq!(
        s.problems,
        [
            Problem::TooLarge {
                file: "big.md".into()
            },
            Problem::NotUtf8 {
                file: "latin1.md".into()
            },
        ]
    );
}

#[test]
fn bad_index_gets_a_synthesized_page() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "getting-started/index.md",
        b"+++\nnope = 1\n+++\n",
    );
    write(dir.path(), "getting-started/first.md", b"# First\n");
    let s = scan(dir.path()).unwrap();
    let rels: Vec<&str> = s.docs.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(rels, ["getting-started", "getting-started/first.md"]);
    assert_eq!(s.docs[0].fallback_title, "Getting started");
    assert_eq!(s.docs[1].parent.as_deref(), Some("getting-started"));
    assert!(matches!(
        &s.problems[..],
        [
            Problem::BadFrontMatter { file, line: 2, .. },
            Problem::SynthesizedSection { dir },
        ] if file == "getting-started/index.md" && dir == "getting-started"
    ));
}

#[test]
fn deep_tree_lists_parents_first() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "z.md", b"z");
    write(dir.path(), "a/b/c/d.md", b"d");
    write(dir.path(), "a/b/index.md", b"b");
    write(dir.path(), "a/0.md", b"0");
    let s = scan(dir.path()).unwrap();
    assert_parents_first(&s.docs);
    let rels: Vec<&str> = s.docs.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(
        rels,
        ["a", "a/0.md", "a/b/index.md", "a/b/c", "a/b/c/d.md", "z.md"]
    );
    assert_eq!(
        doc(&s.docs, "a/b/c").parent.as_deref(),
        Some("a/b/index.md")
    );
}

#[test]
fn not_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "x.md", b"x");
    assert!(matches!(
        scan(&dir.path().join("x.md")),
        Err(MdError::NotADirectory { .. })
    ));
    assert!(matches!(
        scan(&dir.path().join("missing")),
        Err(MdError::Io { .. })
    ));
}

#[test]
fn too_many_files() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..=ferrite_import_md::MAX_FILES {
        write(dir.path(), &format!("{i}.md"), b"");
    }
    assert!(matches!(
        scan(dir.path()),
        Err(MdError::TooManyFiles { limit: 5000 })
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    use std::os::unix::fs::symlink;

    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "secret.md", b"# Secret\n");
    write(outside.path(), "folder/inner.md", b"# Inner\n");
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "page.md", b"# Page\n");
    symlink(
        outside.path().join("secret.md"),
        dir.path().join("linked.md"),
    )
    .unwrap();
    symlink(
        outside.path().join("folder"),
        dir.path().join("linked-folder"),
    )
    .unwrap();
    symlink(dir.path().join("page.md"), dir.path().join("inside.md")).unwrap();
    // A symlink that is not Markdown nor a folder is ignored like any other file.
    symlink(
        outside.path().join("secret.md"),
        dir.path().join("image.png"),
    )
    .unwrap();

    let s = scan(dir.path()).unwrap();
    let rels: Vec<&str> = s.docs.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(rels, ["page.md"]);
    let files: Vec<&str> = s
        .problems
        .iter()
        .map(|p| match p {
            Problem::OutsideDir { file, target } => {
                assert!(!target.is_empty());
                file.as_str()
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(files, ["inside.md", "linked-folder", "linked.md"]);

    // `resolve` never passes a symlink either.
    assert_eq!(resolve(dir.path(), "page.md", "linked.md"), None);
    assert_eq!(
        resolve(dir.path(), "page.md", "linked-folder/inner.md"),
        None
    );
}

#[test]
fn resolve_stays_inside_the_folder() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "site/guides/a.md", b"a");
    write(root.path(), "site/img/my pic.png", b"png");
    write(root.path(), "site/.hidden/x.png", b"x");
    write(root.path(), "outside.png", b"o");
    let dir = root.path().join("site");
    let canon = dir.canonicalize().unwrap();

    assert_eq!(
        resolve(&dir, "guides/a.md", "../img/my%20pic.png"),
        Some(Target {
            path: canon.join("img/my pic.png"),
            rel: "img/my pic.png".into(),
            fragment: None,
        })
    );
    assert_eq!(
        resolve(&dir, "index.md", "guides/a.md#part-2"),
        Some(Target {
            path: canon.join("guides/a.md"),
            rel: "guides/a.md".into(),
            fragment: Some("part-2".into()),
        })
    );
    assert_eq!(
        resolve(&dir, "index.md", "./guides/"),
        Some(Target {
            path: canon.join("guides"),
            rel: "guides".into(),
            fragment: None,
        })
    );
    for bad in [
        "../outside.png",
        "guides/../../outside.png",
        "../../../../../../etc/passwd",
        "/etc/passwd",
        "file:///etc/passwd",
        "https://example.com/a.png",
        "//example.com/a.png",
        "#top",
        ".hidden/x.png",
        "img/missing.png",
        "img\\..\\..\\outside.png",
        "%2e%2e/outside.png",
    ] {
        assert_eq!(resolve(&dir, "guides/a.md", bad), None, "{bad}");
    }
}
