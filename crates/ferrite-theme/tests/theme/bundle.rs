//! Theme bundles (P17): deterministic packing, same hash as the folder, and a hostile
//! tar never gets past the unpacker.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use ferrite_theme::{
    MAX_BUNDLE_BYTES, MAX_FILES, Theme, ThemeError, ThemeSource, pack, pack_embedded,
};

use crate::common::{MANIFEST, write};

fn blog_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes/blog")
}

/// Copies `src` to `dst`, giving every file the modification time `mtime`.
fn copy_tree(src: &Path, dst: &Path, mtime: SystemTime) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to, mtime);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
            let f = std::fs::File::options().write(true).open(&to).unwrap();
            f.set_modified(mtime).unwrap();
        }
    }
}

#[test]
fn pack_is_deterministic() {
    let a = pack(&ThemeSource::Dir(blog_dir())).unwrap();
    let b = pack(&ThemeSource::Dir(blog_dir())).unwrap();
    assert_eq!(a, b);
    let copy = tempfile::tempdir().unwrap();
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    copy_tree(&blog_dir(), copy.path(), then);
    assert_eq!(
        pack(&ThemeSource::Dir(copy.path().to_path_buf())).unwrap(),
        a
    );
    assert_eq!(pack(&ThemeSource::Embedded("blog")).unwrap(), a);
    // Repacking a bundle gives the same bytes.
    assert_eq!(
        pack(&ThemeSource::Bundle(Bytes::from(a.clone()))).unwrap(),
        a
    );
}

#[test]
fn pack_embedded_matches_pack() {
    let name = String::from("blog");
    assert_eq!(
        pack_embedded(&name).unwrap(),
        pack(&ThemeSource::Embedded("blog")).unwrap()
    );
    for unknown in ["missing", "../blog", "blog/templates", ""] {
        assert!(
            matches!(pack_embedded(unknown), Err(ThemeError::NotFound(_))),
            "{unknown:?}"
        );
    }
}

#[test]
fn pack_writes_plain_ustar_with_zeroed_metadata() {
    let bundle = pack(&ThemeSource::Dir(blog_dir())).unwrap();
    let mut archive = tar::Archive::new(&bundle[..]);
    let mut paths = Vec::new();
    for entry in archive.entries().unwrap().raw(true) {
        let entry = entry.unwrap();
        let h = entry.header();
        assert!(h.as_ustar().is_some());
        assert_eq!(h.entry_type(), tar::EntryType::Regular);
        assert_eq!(h.mode().unwrap(), 0o644);
        assert_eq!(h.mtime().unwrap(), 0);
        assert_eq!(h.uid().unwrap(), 0);
        assert_eq!(h.gid().unwrap(), 0);
        assert_eq!(h.username_bytes(), Some(&b""[..]));
        assert_eq!(h.groupname_bytes(), Some(&b""[..]));
        paths.push(String::from_utf8(entry.path_bytes().into_owned()).unwrap());
    }
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    assert!(paths.contains(&"theme.toml".to_owned()));
    assert!(paths.contains(&"templates/index.html".to_owned()));
}

#[test]
fn bundle_and_folder_have_the_same_hash() {
    let folder = Theme::load(&ThemeSource::Dir(blog_dir())).unwrap();
    let embedded = Theme::load(&ThemeSource::Embedded("blog")).unwrap();
    let bundle = pack(&ThemeSource::Dir(blog_dir())).unwrap();
    let unpacked = Theme::load(&ThemeSource::Bundle(Bytes::from(bundle))).unwrap();
    assert_eq!(unpacked.hash(), folder.hash());
    assert_eq!(unpacked.hash(), embedded.hash());
    assert_eq!(unpacked.name(), "blog");
    assert_eq!(
        unpacked.templates().collect::<Vec<_>>(),
        folder.templates().collect::<Vec<_>>()
    );
    let (css, mime) = unpacked.asset("style.css").unwrap();
    assert_eq!(css, folder.asset("style.css").unwrap().0);
    assert_eq!(mime, "text/css; charset=utf-8");
}

#[test]
fn hidden_files_are_left_out_of_bundles() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "theme.toml", MANIFEST.as_bytes());
    write(dir.path(), "templates/index.html", b"index");
    let plain = pack(&ThemeSource::Dir(dir.path().to_path_buf())).unwrap();
    write(dir.path(), ".git/config", b"secret");
    write(dir.path(), "assets/.DS_Store", b"junk");
    assert_eq!(
        pack(&ThemeSource::Dir(dir.path().to_path_buf())).unwrap(),
        plain
    );
    // A hidden entry inside a bundle is skipped like in a folder: same hash.
    let mut tar = base();
    entry(&mut tar, b".hidden/x", b'0', b"", b"x");
    let with_hidden = load_tar(tar).unwrap();
    assert_eq!(with_hidden.hash(), load_tar(base()).unwrap().hash());
}

#[test]
fn pack_refuses_paths_that_do_not_fit_ustar() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "theme.toml", MANIFEST.as_bytes());
    write(dir.path(), "templates/index.html", b"index");
    let long = format!("assets/{}.css", "x".repeat(120));
    write(dir.path(), &long, b"x");
    let err = pack(&ThemeSource::Dir(dir.path().to_path_buf())).unwrap_err();
    assert!(err.to_string().contains(&long), "{err}");
}

#[test]
fn pack_refuses_a_broken_folder_source() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "theme.toml", MANIFEST.as_bytes());
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", dir.path().join("link")).unwrap();
    #[cfg(unix)]
    assert!(matches!(
        pack(&ThemeSource::Dir(dir.path().to_path_buf())),
        Err(ThemeError::Invalid(_))
    ));
}

// ---------------------------------------------------------------------------------
// Hand-built tar streams. Paths are written straight into the header bytes so the
// `tar` crate's own path checks do not stop the hostile ones before the unpacker sees
// them.

/// Appends one ustar entry: `kind` is the type flag, `link` the link name.
fn entry(out: &mut Vec<u8>, path: &[u8], kind: u8, link: &[u8], data: &[u8]) {
    entry_sized(out, path, kind, link, data, data.len() as u64);
}

/// [`entry`] whose header claims `size` bytes whatever `data` holds.
fn entry_sized(out: &mut Vec<u8>, path: &[u8], kind: u8, link: &[u8], data: &[u8], size: u64) {
    let mut h = tar::Header::new_ustar();
    {
        let old = h.as_old_mut();
        old.name[..path.len()].copy_from_slice(path);
        old.linkname[..link.len()].copy_from_slice(link);
    }
    h.set_size(size);
    h.set_mode(0o644);
    h.set_entry_type(tar::EntryType::new(kind));
    h.set_cksum();
    out.extend_from_slice(h.as_bytes());
    out.extend_from_slice(data);
    let pad = (512 - data.len() % 512) % 512;
    out.extend(std::iter::repeat_n(0u8, pad));
}

/// A valid minimal theme, not yet terminated.
fn base() -> Vec<u8> {
    let mut out = Vec::new();
    entry(&mut out, b"templates/index.html", b'0', b"", b"index");
    entry(&mut out, b"theme.toml", b'0', b"", MANIFEST.as_bytes());
    out
}

fn load_tar(mut tar: Vec<u8>) -> Result<Theme, ThemeError> {
    tar.extend_from_slice(&[0u8; 1024]);
    Theme::load(&ThemeSource::Bundle(Bytes::from(tar)))
}

/// Loading `tar` fails with `Invalid`, and the message names `needle`.
fn assert_refused(tar: Vec<u8>, needle: &str) {
    match load_tar(tar) {
        Err(ThemeError::Invalid(problems)) => {
            let msg = problems.join("; ");
            assert!(msg.contains(needle), "{needle:?} not in {msg:?}");
        }
        Err(other) => panic!("expected Invalid naming {needle:?}, got {other}"),
        Ok(_) => panic!("hostile bundle loaded ({needle:?})"),
    }
}

#[test]
fn hand_built_base_bundle_loads() {
    let t = load_tar(base()).unwrap();
    assert_eq!(t.name(), "t");
}

mod bundle_rejects_hostile_entries {
    use super::*;

    #[test]
    fn symlink() {
        let mut tar = base();
        entry(&mut tar, b"assets/link", b'2', b"../../../etc/passwd", b"");
        assert_refused(tar, "assets/link");
    }

    #[test]
    fn hardlink() {
        let mut tar = base();
        entry(&mut tar, b"assets/hl", b'1', b"theme.toml", b"");
        assert_refused(tar, "assets/hl");
    }

    #[test]
    fn parent_segment() {
        let mut tar = base();
        entry(&mut tar, b"../x", b'0', b"", b"x");
        assert_refused(tar, "../x");
        let mut tar = base();
        entry(&mut tar, b"assets/../../x", b'0', b"", b"x");
        assert_refused(tar, "assets/../../x");
    }

    #[test]
    fn absolute_path() {
        let mut tar = base();
        entry(&mut tar, b"/etc/cron.d/x", b'0', b"", b"x");
        assert_refused(tar, "/etc/cron.d/x");
    }

    #[test]
    fn empty_and_dot_segments() {
        for path in [&b"assets//x"[..], b"./theme2.toml", b"assets/./x", b"a\\b"] {
            let mut tar = base();
            entry(&mut tar, path, b'0', b"", b"x");
            assert_refused(tar, &String::from_utf8_lossy(path).replace('\\', "\\\\"));
        }
    }

    #[test]
    fn duplicate_path() {
        let mut tar = base();
        entry(&mut tar, b"templates/index.html", b'0', b"", b"evil");
        assert_refused(tar, "templates/index.html");
    }

    #[test]
    fn devices_fifos_and_directories() {
        for kind in *b"34657" {
            let mut tar = base();
            entry(&mut tar, b"assets/dev", kind, b"", b"");
            assert_refused(tar, "assets/dev");
        }
    }

    #[test]
    fn pax_and_gnu_extension_headers() {
        // A PAX header renaming the next entry, and a GNU long name.
        let pax = b"30 path=assets/renamed-by-pax\n";
        for (kind, data) in [(b'x', &pax[..]), (b'g', &pax[..]), (b'L', b"assets/long\0")] {
            let mut tar = base();
            entry(&mut tar, b"././@Ext", kind, b"", data);
            entry(&mut tar, b"assets/a", b'0', b"", b"a");
            assert_refused(tar, "././@Ext");
        }
    }

    #[test]
    fn too_many_files() {
        // The base has 2 files; 1999 more make 2001.
        let mut tar = base();
        for i in 0..(MAX_FILES - 1) {
            entry(&mut tar, format!("assets/f{i}").as_bytes(), b'0', b"", b"");
        }
        assert_refused(tar, &format!("more than {MAX_FILES} files"));
    }

    #[test]
    fn more_than_twenty_mib() {
        let chunk = vec![b'a'; 7 * 1024 * 1024];
        let mut tar = base();
        for i in 0..3 {
            entry(
                &mut tar,
                format!("assets/big{i}").as_bytes(),
                b'0',
                b"",
                &chunk,
            );
        }
        assert!(
            (tar.len() as u64) < MAX_BUNDLE_BYTES,
            "hits the streaming limit"
        );
        assert_refused(tar, "assets/big2");
    }

    #[test]
    fn size_header_beyond_the_limit_is_refused_before_reading() {
        // A tar bomb's header: claims 21 MiB, carries nothing.
        let mut tar = base();
        entry_sized(&mut tar, b"assets/bomb", b'0', b"", b"", 21 * 1024 * 1024);
        assert_refused(tar, "assets/bomb");
    }

    #[test]
    fn truncated_entry() {
        let mut tar = base();
        entry_sized(&mut tar, b"assets/short", b'0', b"", b"abc", 4096);
        // No end-of-archive blocks: the data simply runs out.
        let err = Theme::load(&ThemeSource::Bundle(Bytes::from(tar))).unwrap_err();
        assert!(matches!(err, ThemeError::Invalid(_)), "{err}");
    }

    #[test]
    fn depth_seventeen() {
        let deep = format!("{}f.css", "d/".repeat(17));
        let mut tar = base();
        entry(&mut tar, deep.as_bytes(), b'0', b"", b"x");
        assert_refused(tar, &deep);
        // Sixteen levels are fine.
        let ok = format!("{}f.css", "d/".repeat(16));
        let mut tar = base();
        entry(&mut tar, ok.as_bytes(), b'0', b"", b"x");
        load_tar(tar).unwrap();
    }

    #[test]
    fn bundle_larger_than_the_cap() {
        let mut tar = base();
        tar.resize(MAX_BUNDLE_BYTES as usize + 1, 0);
        let err = Theme::load(&ThemeSource::Bundle(Bytes::from(tar))).unwrap_err();
        assert!(matches!(err, ThemeError::Invalid(_)), "{err}");
    }

    #[test]
    fn garbage_and_bad_checksums() {
        let err = Theme::load(&ThemeSource::Bundle(Bytes::from_static(&[7u8; 2048]))).unwrap_err();
        assert!(matches!(err, ThemeError::Invalid(_)), "{err}");
        let mut tar = base();
        tar[148] ^= 1; // first header's checksum field
        let err = load_tar(tar).unwrap_err();
        assert!(matches!(err, ThemeError::Invalid(_)), "{err}");
        let err = Theme::load(&ThemeSource::Bundle(Bytes::new())).unwrap_err();
        assert!(matches!(err, ThemeError::Invalid(_)), "{err}");
    }

    #[test]
    fn non_canonical_ending() {
        let refused = |tar: Vec<u8>| {
            let err = Theme::load(&ThemeSource::Bundle(Bytes::from(tar))).unwrap_err();
            assert!(err.to_string().contains("two zero blocks"), "{err}");
        };
        // No end blocks, only one, a third one, and a byte after the end.
        refused(base());
        let mut one = base();
        one.extend_from_slice(&[0u8; 512]);
        refused(one);
        let mut three = base();
        three.extend_from_slice(&[0u8; 1536]);
        refused(three);
        let mut trailing = base();
        trailing.extend_from_slice(&[0u8; 1024]);
        trailing.push(b'x');
        refused(trailing);
        // Data hidden after the first zero block (a second archive appended).
        let mut smuggled = base();
        smuggled.extend_from_slice(&[0u8; 512]);
        entry(&mut smuggled, b"assets/smuggled", b'0', b"", b"x");
        smuggled.extend_from_slice(&[0u8; 1024]);
        refused(smuggled);
        // A packed bundle is canonical.
        let packed = pack(&ThemeSource::Dir(blog_dir())).unwrap();
        assert_eq!(&packed[packed.len() - 1024..], &[0u8; 1024][..]);
        Theme::load(&ThemeSource::Bundle(Bytes::from(packed))).unwrap();
    }

    #[test]
    fn non_ustar_header() {
        let mut tar = base();
        let mut h = tar::Header::new_old();
        h.as_old_mut().name[..8].copy_from_slice(b"assets/o");
        h.set_size(0);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        tar.extend_from_slice(h.as_bytes());
        assert_refused(tar, "assets/o");
    }
}
