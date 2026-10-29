//! [`ZipSource`]: attachment files read from a zip of the uploads, in place (P24).
//!
//! The central directory is read once, at [`ZipSource::open`]; each entry is matched by
//! its path below the first `uploads/` segment, or else by its full path. Both an entry's
//! name and the path looked up are compared in Unicode NFC, so an archive made on macOS
//! (NFD names) matches the paths WordPress stored. An entry is read on a blocking thread,
//! streamed out in chunks, and never extracted to disk.
//!
//! Limits: before the central directory is parsed, the archive's end records (zip64 ones
//! too) are read, and an archive declaring more than [`MAX_ENTRIES`] entries or a central
//! directory over [`MAX_DIRECTORY`] bytes is refused; one whose entries declare more than
//! [`MAX_DECLARED`] uncompressed bytes in all is refused after it. Entries whose name is
//! absolute or has a `..` segment, encrypted entries and entries packed with anything but
//! stored or deflate are left out (an attachment naming one is missing). An entry that
//! declares more than `max_upload` is refused for its size; one that inflates past its
//! declared size ends its stream with an error, so the file is missing and the run goes
//! on.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use ferrite_media::MediaError;
use unicode_normalization::UnicodeNormalization;
use zip::{CompressionMethod, ZipArchive, ZipReadOptions};

use crate::ImportError;
use crate::run::{ByteStream, MediaSource};

/// The most entries an archive may hold.
pub const MAX_ENTRIES: u64 = 500_000;
/// The most declared uncompressed bytes an archive may hold in all (64 GiB).
pub const MAX_DECLARED: u64 = 64 << 30;
/// The largest central directory an archive may declare (64 MiB).
pub const MAX_DIRECTORY: u64 = 64 << 20;
/// The classic end of central directory record: its signature and fixed size.
const EOCD: &[u8; 4] = b"PK\x05\x06";
const EOCD_LEN: usize = 22;
/// The zip64 end record locator, just before the classic record.
const LOCATOR: &[u8; 4] = b"PK\x06\x07";
const LOCATOR_LEN: usize = 20;
/// The zip64 end record, up to its directory size field.
const EOCD64: &[u8; 4] = b"PK\x06\x06";
const EOCD64_LEN: usize = 48;
/// Bytes per chunk of an entry's stream.
const CHUNK: usize = 64 * 1024;
/// Chunks in flight between the reading thread and the stream.
const QUEUE: usize = 4;

/// An entry the archive can serve.
#[derive(Debug, Clone, Copy)]
struct Entry {
    index: usize,
    /// The declared uncompressed size.
    size: u64,
}

/// A zip of `wp-content/uploads` (or of its contents), read in place.
#[derive(Debug, Clone)]
pub struct ZipSource {
    path: PathBuf,
    archive: Arc<ZipArchive<std::fs::File>>,
    /// Entries by their path below `uploads/` (or their full path).
    entries: Arc<HashMap<String, Entry>>,
}

/// The archive limits; tests use small ones.
#[derive(Debug, Clone, Copy)]
struct Limits {
    entries: u64,
    declared: u64,
    directory: u64,
}

impl ZipSource {
    /// Reads the central directory of the zip at `path` (a blocking read: call it off
    /// the async runtime for a large archive). Fails when the file is not a zip or is
    /// over the limits (see the module docs).
    pub fn open(path: &Path) -> Result<ZipSource, ImportError> {
        Self::open_with(
            path,
            Limits {
                entries: MAX_ENTRIES,
                declared: MAX_DECLARED,
                directory: MAX_DIRECTORY,
            },
        )
    }

    fn open_with(path: &Path, limits: Limits) -> Result<ZipSource, ImportError> {
        let refused = |msg: String| ImportError::Archive {
            path: path.to_owned(),
            msg,
        };
        let file_err = |source| ImportError::File {
            path: path.to_owned(),
            source,
        };
        let mut file = std::fs::File::open(path).map_err(file_err)?;
        let end = end_records(&mut file)
            .map_err(file_err)?
            .ok_or_else(|| refused("it has no end of central directory record".into()))?;
        if end.entries > limits.entries {
            return Err(refused(format!(
                "it declares {} entries, more than the {} accepted",
                end.entries, limits.entries
            )));
        }
        if end.directory > limits.directory {
            return Err(refused(format!(
                "its central directory declares {} bytes, more than the {} accepted",
                end.directory, limits.directory
            )));
        }
        let archive = ZipArchive::new(file).map_err(|e| refused(e.to_string()))?;
        let mut entries = HashMap::new();
        let mut declared: u64 = 0;
        for index in 0..archive.len() {
            let data = archive
                .by_index_data(index)
                .map_err(|e| refused(e.to_string()))?;
            declared = declared.saturating_add(data.size());
            if declared > limits.declared {
                return Err(refused(format!(
                    "its entries declare more than {} bytes in all",
                    limits.declared
                )));
            }
            if data.is_dir() {
                continue;
            }
            let Some(key) = data.name().ok().as_deref().and_then(entry_key) else {
                tracing::warn!(
                    index,
                    "an uploads archive entry with an unsafe name is left out"
                );
                continue;
            };
            let readable = !data.encrypted()
                && matches!(
                    data.compression(),
                    CompressionMethod::Stored | CompressionMethod::Deflated
                );
            if !readable {
                tracing::warn!(entry = %key, "an encrypted or unsupported uploads archive entry is left out");
                continue;
            }
            entries.entry(key).or_insert(Entry {
                index,
                size: data.size(),
            });
        }
        Ok(ZipSource {
            path: path.to_owned(),
            archive: Arc::new(archive),
            entries: Arc::new(entries),
        })
    }
}

/// What an archive's end records declare.
#[derive(Debug, PartialEq, Eq)]
struct EndRecords {
    entries: u64,
    /// The central directory's size in bytes.
    directory: u64,
}

/// Reads the end records from the tail of `file`: the classic one (searched back over a
/// comment of up to 64 KiB), and the zip64 one its locator points to when there is one.
/// `None` when there is no classic record.
fn end_records(file: &mut std::fs::File) -> std::io::Result<Option<EndRecords>> {
    let len = file.seek(SeekFrom::End(0))?;
    let tail_len = len.min((LOCATOR_LEN + EOCD_LEN + usize::from(u16::MAX)) as u64);
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; usize::try_from(tail_len).unwrap_or(0)];
    file.read_exact(&mut tail)?;
    let u16_at = |b: &[u8], at: usize| u64::from(u16::from_le_bytes([b[at], b[at + 1]]));
    let u32_at = |b: &[u8], at: usize| {
        u64::from(u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))
    };
    let u64_at = |b: &[u8], at: usize| {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&b[at..at + 8]);
        u64::from_le_bytes(bytes)
    };
    // The last record whose comment ends the file.
    let Some(at) = (0..=tail.len().saturating_sub(EOCD_LEN)).rev().find(|&i| {
        tail.len() >= EOCD_LEN
            && &tail[i..i + 4] == EOCD
            && i as u64 + EOCD_LEN as u64 + u16_at(&tail, i + 20) == tail.len() as u64
    }) else {
        return Ok(None);
    };
    let mut end = EndRecords {
        entries: u16_at(&tail, at + 10),
        directory: u32_at(&tail, at + 12),
    };
    if at >= LOCATOR_LEN && &tail[at - LOCATOR_LEN..at - LOCATOR_LEN + 4] == LOCATOR {
        let offset = u64_at(&tail, at - LOCATOR_LEN + 8);
        if offset.saturating_add(EOCD64_LEN as u64) <= len {
            let mut record = [0u8; EOCD64_LEN];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut record)?;
            if &record[..4] == EOCD64 {
                end = EndRecords {
                    entries: u64_at(&record, 32),
                    directory: u64_at(&record, 40),
                };
            }
        }
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(Some(end))
}

/// The NFC segments of `name` (`\\` read as `/`); `None` for a name that is absolute or
/// has a `..`, `.` or empty segment.
fn segments(name: &str) -> Option<Vec<String>> {
    let name: String = name.replace('\\', "/").nfc().collect();
    if name.starts_with('/') || name.contains(':') {
        return None;
    }
    let segments: Vec<String> = name.split('/').map(str::to_owned).collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return None;
    }
    Some(segments)
}

/// The key an entry is found by: its name below the first `uploads` segment, or the
/// whole name (see [`segments`]).
fn entry_key(name: &str) -> Option<String> {
    let segments = segments(name)?;
    let below = match segments.iter().position(|s| s == "uploads") {
        Some(i) if i + 1 < segments.len() => &segments[i + 1..],
        _ => &segments[..],
    };
    Some(below.join("/"))
}

/// The key a looked-up path (relative to the uploads) is found by: the path itself,
/// normalised as [`segments`] does.
fn lookup_key(rel_path: &str) -> Option<String> {
    segments(rel_path).map(|s| s.join("/"))
}

#[async_trait]
impl MediaSource for ZipSource {
    /// The entry for `rel_path`; one that declares more than `max` is refused
    /// ([`MediaError::TooLarge`]), and its stream fails once it yields more than it
    /// declared or more than `max`.
    async fn open(
        &self,
        rel_path: &str,
        _url: &str,
        max: u64,
    ) -> Result<Option<ByteStream>, ImportError> {
        let Some(entry) = lookup_key(rel_path).and_then(|key| self.entries.get(&key).copied())
        else {
            return Ok(None);
        };
        if entry.size > max {
            return Err(ImportError::Media(MediaError::TooLarge { limit: max }));
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(QUEUE);
        let archive = self.archive.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = read_entry(&archive, &path, entry, max, &tx) {
                // The receiver may be gone already; then nobody wants the error either.
                let _ = tx.blocking_send(Err(e));
            }
        });
        Ok(Some(Box::pin(futures::stream::poll_fn(move |cx| {
            rx.poll_recv(cx)
        }))))
    }
}

/// Streams `entry` into `tx`, at most `min(declared, max)` bytes; more is an error.
fn read_entry(
    archive: &ZipArchive<std::fs::File>,
    path: &Path,
    entry: Entry,
    max: u64,
    tx: &tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
) -> Result<(), std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let data = archive
        .by_index_data(entry.index)
        .map_err(std::io::Error::other)?;
    let mut reader = data
        .with_reader(&mut file, ZipReadOptions::new())
        .map_err(std::io::Error::other)?;
    let limit = entry.size.min(max);
    let mut read: u64 = 0;
    loop {
        let mut buf = vec![0u8; CHUNK];
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        read += n as u64;
        if read > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("the archive entry is larger than the {limit} bytes it declares"),
            ));
        }
        buf.truncate(n);
        if tx.blocking_send(Ok(Bytes::from(buf))).is_err() {
            // The stream was dropped: stop reading.
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Write;

    use futures::TryStreamExt;
    use zip::write::SimpleFileOptions;

    use super::*;

    /// Writes a zip of `(name, bytes)` entries, deflated, to `path`.
    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, bytes) in files {
            zip.start_file(*name, deflate).unwrap();
            zip.write_all(bytes).unwrap();
        }
        std::fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
    }

    /// Rewrites, in the central and local headers of entry `name`, the field at
    /// `central`/`local` (offsets into each header) with `value`.
    fn patch(path: &Path, name: &str, central: usize, local: usize, value: &[u8]) {
        let mut zip = std::fs::read(path).unwrap();
        let u16_at = |b: &[u8], at: usize| usize::from(u16::from_le_bytes([b[at], b[at + 1]]));
        let u32_at =
            |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        let eocd = zip.len() - 22;
        assert_eq!(&zip[eocd..eocd + 4], b"PK\x05\x06");
        let mut at = u32_at(&zip, eocd + 16);
        for _ in 0..u16_at(&zip, eocd + 10) {
            let name_len = u16_at(&zip, at + 28);
            if &zip[at + 46..at + 46 + name_len] == name.as_bytes() {
                let header = u32_at(&zip, at + 42);
                zip[at + central..at + central + value.len()].copy_from_slice(value);
                zip[header + local..header + local + value.len()].copy_from_slice(value);
                std::fs::write(path, zip).unwrap();
                return;
            }
            at += 46 + name_len + u16_at(&zip, at + 30) + u16_at(&zip, at + 32);
        }
        panic!("no entry {name}");
    }

    async fn read(src: &ZipSource, rel: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let Some(stream) = src.open(rel, "", max).await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let chunks: Vec<Bytes> = stream.try_collect().await.map_err(|e| e.to_string())?;
        Ok(Some(chunks.concat()))
    }

    #[test]
    fn keys_are_below_uploads_or_the_whole_name() {
        assert_eq!(
            entry_key("wp-content/uploads/2024/03/a.png").as_deref(),
            Some("2024/03/a.png")
        );
        assert_eq!(
            entry_key("site/uploads/x/uploads/a.png").as_deref(),
            Some("x/uploads/a.png")
        );
        assert_eq!(entry_key("2024/03/a.png").as_deref(), Some("2024/03/a.png"));
        assert_eq!(entry_key("uploads").as_deref(), Some("uploads"));
        assert_eq!(
            entry_key("uploads\\2024\\a.png").as_deref(),
            Some("2024/a.png")
        );
        for bad in [
            "/etc/passwd",
            "../a.png",
            "uploads/../../a.png",
            "a/./b.png",
            "a//b.png",
            "C:/a.png",
            "",
        ] {
            assert_eq!(entry_key(bad), None, "{bad}");
        }
    }

    #[tokio::test]
    async fn zip_source_reads_entries_and_misses_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("u.zip");
        write_zip(
            &archive,
            &[
                ("wp-content/uploads/2024/03/a.txt", b"hello"),
                ("wp-content/uploads/2024/03/big.txt", &[7u8; 200_000]),
            ],
        );
        let src = ZipSource::open(&archive).unwrap();
        assert_eq!(
            read(&src, "2024/03/a.txt", 10).await.unwrap().unwrap(),
            b"hello"
        );
        let big = read(&src, "2024/03/big.txt", 1 << 20)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(big, vec![7u8; 200_000]);
        for missing in ["2024/03/b.txt", "2024/03", "../2024/03/a.txt", ""] {
            assert_eq!(read(&src, missing, 10).await.unwrap(), None, "{missing}");
        }
        // Declared over `max`: refused for its size before any read.
        let err = src.open("2024/03/big.txt", "", 1000).await.err().unwrap();
        assert!(
            matches!(
                err,
                ImportError::Media(MediaError::TooLarge { limit: 1000 })
            ),
            "{err:?}"
        );
        // Not a zip at all.
        std::fs::write(dir.path().join("no.zip"), b"plain").unwrap();
        let err = ZipSource::open(&dir.path().join("no.zip")).unwrap_err();
        assert!(matches!(err, ImportError::Archive { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn zip_source_refuses_traversal_names() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("u.zip");
        write_zip(
            &archive,
            &[
                ("../evil.png", b"evil"),
                ("/abs.png", b"abs"),
                ("wp-content/uploads/../../up.png", b"up"),
                ("wp-content/uploads/ok.png", b"ok"),
            ],
        );
        let src = ZipSource::open(&archive).unwrap();
        assert_eq!(src.entries.len(), 1, "{:?}", src.entries);
        assert_eq!(read(&src, "ok.png", 10).await.unwrap().unwrap(), b"ok");
        for name in [
            "evil.png",
            "../evil.png",
            "abs.png",
            "/abs.png",
            "up.png",
            "../../up.png",
        ] {
            assert_eq!(read(&src, name, 10).await.unwrap(), None, "{name}");
        }
    }

    /// An entry that inflates past its declared size fails its stream (the import
    /// counts that file missing); the other entries still read.
    #[tokio::test]
    async fn zip_source_entry_bomb_is_missing_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("u.zip");
        write_zip(
            &archive,
            &[
                ("uploads/bomb.png", &vec![0u8; 10 << 20]),
                ("uploads/ok.png", b"ok"),
            ],
        );
        // Central header offset 24 and local header offset 22: the uncompressed size.
        patch(&archive, "uploads/bomb.png", 24, 22, &1024u32.to_le_bytes());
        let src = ZipSource::open(&archive).unwrap();
        let err = read(&src, "bomb.png", 1 << 30).await.unwrap_err();
        assert!(err.contains("declare") || err.contains("checksum"), "{err}");
        assert_eq!(read(&src, "ok.png", 10).await.unwrap().unwrap(), b"ok");
    }

    #[test]
    fn zip_source_refuses_too_many_entries() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("u.zip");
        write_zip(&archive, &[("a", b"1"), ("b", b"2"), ("c", b"3")]);
        let limits = |entries, declared| Limits {
            entries,
            declared,
            directory: MAX_DIRECTORY,
        };
        assert!(ZipSource::open_with(&archive, limits(3, 3)).is_ok());
        let err = ZipSource::open_with(&archive, limits(2, 3)).unwrap_err();
        assert!(err.to_string().contains("3 entries"), "{err}");
        let err = ZipSource::open_with(&archive, limits(3, 2)).unwrap_err();
        assert!(
            err.to_string().contains("declare more than 2 bytes"),
            "{err}"
        );
        assert_eq!(
            (MAX_ENTRIES, MAX_DECLARED, MAX_DIRECTORY),
            (500_000, 64 << 30, 64 << 20)
        );
    }

    /// An end record that declares too many entries, or too large a central directory,
    /// is refused before the directory is read (here the archive has no directory at
    /// all, so a parse would fail differently).
    #[test]
    fn zip_source_refuses_too_many_entries_before_parsing() {
        let dir = tempfile::tempdir().unwrap();
        // A zip64 end record declaring 600 000 entries, its locator, then the classic
        // end record with every field saturated.
        let mut zip64 = Vec::new();
        zip64.extend_from_slice(b"PK\x06\x06");
        zip64.extend_from_slice(&44u64.to_le_bytes()); // size of the rest
        zip64.extend_from_slice(&[45, 0, 45, 0]); // versions
        zip64.extend_from_slice(&[0; 8]); // disk numbers
        zip64.extend_from_slice(&600_000u64.to_le_bytes()); // entries on this disk
        zip64.extend_from_slice(&600_000u64.to_le_bytes()); // entries in all
        zip64.extend_from_slice(&1000u64.to_le_bytes()); // directory size
        zip64.extend_from_slice(&0u64.to_le_bytes()); // directory offset
        zip64.extend_from_slice(b"PK\x06\x07");
        zip64.extend_from_slice(&0u32.to_le_bytes());
        zip64.extend_from_slice(&0u64.to_le_bytes()); // the zip64 end record's offset
        zip64.extend_from_slice(&1u32.to_le_bytes());
        zip64.extend_from_slice(b"PK\x05\x06");
        zip64.extend_from_slice(&[0; 4]);
        zip64.extend_from_slice(&[0xff; 4]); // entries (see zip64)
        zip64.extend_from_slice(&[0xff; 8]); // size, offset (see zip64)
        zip64.extend_from_slice(&[0; 2]); // no comment
        let many = dir.path().join("many.zip");
        std::fs::write(&many, &zip64).unwrap();
        let err = ZipSource::open(&many).unwrap_err();
        assert!(matches!(err, ImportError::Archive { .. }), "{err:?}");
        assert!(err.to_string().contains("600000 entries"), "{err}");

        // A classic end record declaring a 70 MiB directory (with a comment).
        let mut end = Vec::new();
        end.extend_from_slice(b"PK\x05\x06");
        end.extend_from_slice(&[0; 4]);
        end.extend_from_slice(&3u16.to_le_bytes());
        end.extend_from_slice(&3u16.to_le_bytes());
        end.extend_from_slice(&(70u32 << 20).to_le_bytes());
        end.extend_from_slice(&0u32.to_le_bytes());
        end.extend_from_slice(&5u16.to_le_bytes());
        end.extend_from_slice(b"hello");
        let big = dir.path().join("big.zip");
        std::fs::write(&big, &end).unwrap();
        let err = ZipSource::open(&big).unwrap_err();
        assert!(err.to_string().contains("central directory"), "{err}");
        // No end record at all: not a zip.
        let none = dir.path().join("none.zip");
        std::fs::write(&none, vec![0u8; 100]).unwrap();
        assert!(matches!(
            ZipSource::open(&none).unwrap_err(),
            ImportError::Archive { .. }
        ));
    }

    /// The looked-up path is only normalised: an `uploads` segment below the uploads
    /// root is part of it. Names match whatever their Unicode normal form.
    #[tokio::test]
    async fn lookups_keep_their_segments_and_match_any_normal_form() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("u.zip");
        write_zip(
            &archive,
            &[
                ("wp-content/uploads/2024/uploads/a.png", b"nested"),
                ("uploads/cafe\u{301}.png", b"nfd"),
                ("uploads/na\u{ef}ve.png", b"nfc"),
            ],
        );
        let src = ZipSource::open(&archive).unwrap();
        assert_eq!(
            read(&src, "2024/uploads/a.png", 10).await.unwrap().unwrap(),
            b"nested"
        );
        assert_eq!(read(&src, "a.png", 10).await.unwrap(), None);
        assert_eq!(
            read(&src, "caf\u{e9}.png", 10).await.unwrap().unwrap(),
            b"nfd"
        );
        assert_eq!(
            read(&src, "nai\u{308}ve.png", 10).await.unwrap().unwrap(),
            b"nfc"
        );
    }

    /// Only stored and deflate entries are read; another method leaves the entry out.
    #[tokio::test]
    async fn zip_source_reads_stored_and_deflate_only() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("u.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for name in ["stored.txt", "other.txt"] {
            zip.start_file(name, stored).unwrap();
            zip.write_all(b"plain").unwrap();
        }
        zip.finish().unwrap();
        // Method 12 (bzip2) in both headers (central offset 10, local offset 8).
        patch(&archive, "other.txt", 10, 8, &12u16.to_le_bytes());
        let src = ZipSource::open(&archive).unwrap();
        assert_eq!(
            read(&src, "stored.txt", 10).await.unwrap().unwrap(),
            b"plain"
        );
        assert_eq!(read(&src, "other.txt", 10).await.unwrap(), None);
    }
}
