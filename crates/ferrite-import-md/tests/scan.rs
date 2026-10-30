//! The folder scan over the fixture tree and over trees made at test time (P62).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use ferrite_import_md::{
    FORMAT, MAX_FILE_BYTES, MdDoc, MdError, Miss, Problem, Target, locate, resolve, scan,
};
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
    assert!(s.docs[0].index_skipped);
    // The index's own line says why; no "no index.md" line.
    assert!(matches!(
        &s.problems[..],
        [Problem::BadFrontMatter { file, line: 2, .. }] if file == "getting-started/index.md"
    ));
}

/// A symlinked index is reported `outside` and counts as a skipped index, so the
/// folder's stand-in page is marked and gets no `synthesized` line.
#[cfg(unix)]
#[test]
fn a_symlinked_index_is_a_skipped_index() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "README.md", b"# Repo readme\n");
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "guides/one.md", b"# One\n");
    std::os::unix::fs::symlink(
        outside.path().join("README.md"),
        dir.path().join("guides/Index.md"),
    )
    .unwrap();
    write(dir.path(), "plain/one.md", b"# One\n");
    let s = scan(dir.path()).unwrap();
    assert!(doc(&s.docs, "guides").index_skipped);
    assert!(!doc(&s.docs, "plain").index_skipped);
    assert_eq!(
        s.problems
            .iter()
            .map(|p| (p.kind(), p.file()))
            .collect::<Vec<_>>(),
        [("outside", "guides/Index.md"), ("synthesized", "plain")]
    );
}

#[cfg(unix)]
#[test]
fn a_fifo_named_index_is_a_skipped_index() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "guides/one.md", b"# One\n");
    let status = std::process::Command::new("mkfifo")
        .arg(dir.path().join("guides/index.md"))
        .status()
        .unwrap();
    assert!(status.success());
    let s = scan(dir.path()).unwrap();
    assert!(doc(&s.docs, "guides").index_skipped);
    assert_eq!(
        s.problems
            .iter()
            .map(|p| (p.kind(), p.file()))
            .collect::<Vec<_>>(),
        [("unreadable", "guides/index.md")]
    );
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
        "%2e%2e/%2e%2e/outside.png",
        "..%2f..%2foutside.png",
        "../%2e%2e/outside.png",
    ] {
        assert_eq!(resolve(&dir, "guides/a.md", bad), None, "{bad}");
    }
}

/// One `..` from `guides/a.md` stays inside: with a decoy `site/outside.png` it is found,
/// encoded or not, while two climb above the folder to the real `outside.png` and are
/// refused as outside, not as missing.
#[test]
fn one_level_up_is_inside_two_are_outside() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "site/guides/a.md", b"a");
    write(root.path(), "site/outside.png", b"decoy");
    write(root.path(), "outside.png", b"o");
    let dir = root.path().join("site");
    let decoy = dir.canonicalize().unwrap().join("outside.png");
    for inside in ["../outside.png", "%2e%2e/outside.png", ".%2e/outside.png"] {
        let t = resolve(&dir, "guides/a.md", inside).unwrap_or_else(|| panic!("{inside}"));
        assert_eq!(
            (t.path, t.rel.as_str()),
            (decoy.clone(), "outside.png"),
            "{inside}"
        );
    }
    for outside in [
        "../../outside.png",
        "%2e%2e/%2e%2e/outside.png",
        "..%2f..%2foutside.png",
        "guides/../../../outside.png",
        "img\\..\\..\\outside.png",
    ] {
        assert_eq!(
            locate(&dir, "guides/a.md", outside),
            Err(Miss::Outside),
            "{outside}"
        );
    }
    assert_eq!(locate(&dir, "guides/a.md", "nope.png"), Err(Miss::Missing));
    assert_eq!(
        locate(&dir, "guides/a.md", "../.hidden.png"),
        Err(Miss::Missing)
    );
    for not_relative in [
        "/etc/passwd",
        "https://example.com/a.png",
        "file:///x",
        "#top",
    ] {
        assert_eq!(
            locate(&dir, "guides/a.md", not_relative),
            Err(Miss::NotRelative),
            "{not_relative}"
        );
    }
}

/// An unreadable `.md` file or subfolder is reported and skipped; the rest is scanned.
#[cfg(unix)]
#[test]
fn unreadable_entries_are_skipped() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "ok.md", b"# Ok\n");
    write(dir.path(), "locked.md", b"# Locked\n");
    write(dir.path(), "closed/inner.md", b"# Inner\n");
    let mode = |p: &Path, m: u32| fs::set_permissions(p, fs::Permissions::from_mode(m)).unwrap();
    mode(&dir.path().join("locked.md"), 0o000);
    mode(&dir.path().join("closed"), 0o000);
    // Root reads anything: nothing to check then.
    let readable = fs::File::open(dir.path().join("locked.md")).is_ok();
    let s = scan(dir.path());
    mode(&dir.path().join("closed"), 0o755);
    mode(&dir.path().join("locked.md"), 0o644);
    if readable {
        return;
    }
    let s = s.unwrap();
    let rels: Vec<&str> = s.docs.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(rels, ["ok.md"]);
    let files: Vec<&str> = s
        .problems
        .iter()
        .map(|p| match p {
            Problem::Unreadable { file, message } => {
                assert!(!message.is_empty());
                file.as_str()
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(files, ["closed", "locked.md"]);
}

/// Folders nest at most 64 deep (core's parent chain limit); deeper ones are reported
/// and skipped.
#[test]
fn folders_nest_at_most_64_deep() {
    let dir = tempfile::tempdir().unwrap();
    let deep = |n: usize| vec!["d"; n].join("/");
    write(dir.path(), &format!("{}/ok.md", deep(64)), b"ok");
    write(dir.path(), &format!("{}/far.md", deep(65)), b"far");
    let s = scan(dir.path()).unwrap();
    assert!(
        s.docs
            .iter()
            .any(|d| d.rel == format!("{}/ok.md", deep(64)))
    );
    assert!(!s.docs.iter().any(|d| d.rel.ends_with("far.md")));
    assert!(s.problems.contains(&Problem::TooDeep { dir: deep(65) }));
    let ok = doc(&s.docs, &format!("{}/ok.md", deep(64)));
    // 64 synthesized folder pages above it.
    let mut depth = 0;
    let mut at = ok.parent.clone();
    while let Some(p) = at {
        depth += 1;
        at = doc(&s.docs, &p).parent.clone();
    }
    assert_eq!(depth, 64);
}

/// `Index.md` and `readme.md` are a folder's page too; `index.md` wins over `README.md`
/// whatever the case.
#[test]
fn index_names_ignore_case() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a/Index.md", b"# A\n");
    write(dir.path(), "a/README.md", b"# Readme\n");
    write(dir.path(), "a/child.md", b"# Child\n");
    write(dir.path(), "b/readme.md", b"# B\n");
    write(dir.path(), "b/child.md", b"# Child\n");
    let s = scan(dir.path()).unwrap();
    let a = doc(&s.docs, "a/Index.md");
    assert_eq!((a.slug.as_str(), a.folder.as_deref()), ("a", Some("a")));
    assert_eq!(
        doc(&s.docs, "a/child.md").parent.as_deref(),
        Some("a/Index.md")
    );
    // The other index-like file is an ordinary child.
    assert_eq!(
        doc(&s.docs, "a/README.md").parent.as_deref(),
        Some("a/Index.md")
    );
    let b = doc(&s.docs, "b/readme.md");
    assert_eq!((b.slug.as_str(), b.fallback_title.as_str()), ("b", "B"));
    assert_eq!(
        doc(&s.docs, "b/child.md").parent.as_deref(),
        Some("b/readme.md")
    );
    assert!(s.problems.is_empty(), "{:?}", s.problems);
}

/// Every page knows the folder it stands for: the root index `""`, a folder's index or
/// synthesized page that folder; other files none.
#[test]
fn folder_pages_name_their_folder() {
    let s = scan(&fixture()).unwrap();
    let folder = |rel: &str| doc(&s.docs, rel).folder.clone();
    assert_eq!(folder("index.md").as_deref(), Some(""));
    assert_eq!(folder("03-reference").as_deref(), Some("03-reference"));
    assert_eq!(folder("guides/README.md").as_deref(), Some("guides"));
    assert_eq!(folder("01-install.md"), None);
    assert_eq!(folder("guides/writing.md"), None);
}
