//! Staging a media file from any byte stream (M2a-2b P4/P7, R1/F13; M3 P13): browser
//! uploads and imported files are typed and stored the same way.
//!
//! [`stage_stream`] stores the stream in the [`MediaStore`], hashed while it streams.
//! The type is decided from the content's first [`HEAD_BYTES`] before anything is
//! stored, never from a client's `Content-Type`:
//!
//! | Head | Stored as |
//! |---|---|
//! | looks like SVG ([`looks_like_svg`]) | the sanitiser's output, `image/svg+xml`, marked sanitised (at most [`MAX_SVG_BYTES`] read) |
//! | sniffs as `audio/mpeg`, named `.txt`/`.csv`, and the head passes the text rule below | `text/plain` / `text/csv` (infer's MP3 matcher reads only `ID3` or `FF Fx`; a note starting "ID3" is text) |
//! | sniffs as an [`inline_safe`] type (JPEG, PNG, GIF, WebP, MP3/Ogg/WAV, MP4/WebM, PDF) | that type |
//! | a ZIP (`PK\x03\x04`) named `.docx .xlsx .pptx .odt .ods .odp .zip` | the type of that extension |
//! | no known magic, named `.txt`/`.csv`, UTF-8 (BOM allowed) without NUL, first non-space character not `<` | `text/plain` / `text/csv` |
//! | anything else | refused ([`MediaError::Unsupported`]) |
//!
//! The file name only picks within the ZIP and text classes (F13). A stream over
//! `max_bytes` is cut while it streams; a cut, a stream error and a refused type leave
//! nothing stored (the store aborts and removes its staging object). File contents are
//! never logged.
//!
//! With `strip` on, a JPEG, PNG or WebP is read whole (at most [`STRIP_MAX_BYTES`]; a
//! larger one is refused), its EXIF and XMP removed losslessly by
//! [`strip_metadata`] and the result stored, so the artifact's hash is the stripped
//! file's. A file that cannot be parsed is refused too ([`MediaError::Metadata`]): with
//! the option on, a file is never stored with its metadata (M4-1 P17, ruling R1).

use std::io;

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _};

use crate::{
    ArtifactRef, MediaError, MediaStore, inline_safe, is_raster_image, sanitize_filename,
    sniff_mime, strip_metadata,
    svg::{MAX_SVG_BYTES, SVG_MIME, looks_like_svg, sanitize_svg},
};

/// How much of the file decides its type (P4): 8 KiB.
pub const HEAD_BYTES: usize = 8 * 1024;

/// The largest image held in memory to strip its metadata: 64 MiB (the size variants
/// are made up to).
pub const STRIP_MAX_BYTES: usize = 64 << 20;

/// The types [`strip_metadata`] works on.
fn strippable(mime: &str) -> bool {
    matches!(mime, "image/jpeg" | "image/png" | "image/webp")
}

/// The ZIP-based document classes by extension (R1/F13).
const ZIP_TYPES: [(&str, &str); 7] = [
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    ("zip", "application/zip"),
];

/// The text classes by extension (R1/F13).
const TEXT_TYPES: [(&str, &str); 2] = [("txt", "text/plain"), ("csv", "text/csv")];

const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";
/// What `sniff_mime` answers for MP3 (a weak, 2–3 byte match).
const MP3: &str = "audio/mpeg";
/// What `sniff_mime` answers for content without known magic.
const UNKNOWN: &str = "application/octet-stream";

/// A file stored by [`stage_stream`].
#[derive(Debug, Clone)]
pub struct StagedFile {
    /// The stored original.
    pub artifact: ArtifactRef,
    /// Decided from the content (see the module docs).
    pub mime: String,
    pub bytes: u64,
    /// The file name as given.
    pub original_name: String,
    /// The bytes are the SVG sanitiser's output.
    pub sanitized: bool,
    /// A raster image ([`is_raster_image`]).
    pub raster: bool,
}

/// Stages `stream`, named `file_name`, into `store` (see the module docs): reads the
/// head, decides the type and stores the file, streamed and hashed as it arrives (an SVG
/// is read whole, at most [`MAX_SVG_BYTES`], and its sanitised form stored).
///
/// With `strip`, EXIF and XMP are removed from JPEG, PNG and WebP files first (see the
/// module docs).
///
/// Fails with [`MediaError::TooLarge`] past `max_bytes` (or the SVG limit),
/// [`MediaError::Unsupported`] or [`MediaError::Svg`] for a refused type, and
/// [`MediaError::Io`] when the stream yields an error, and, with `strip`, with
/// [`MediaError::Metadata`] for a JPEG/PNG/WebP over [`STRIP_MAX_BYTES`] or one that
/// cannot be parsed (its metadata cannot be removed, so it is not stored); nothing is
/// stored then.
pub async fn stage_stream<S>(
    store: &MediaStore,
    file_name: &str,
    stream: S,
    max_bytes: u64,
    strip: bool,
) -> Result<StagedFile, MediaError>
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + Unpin,
{
    stage_stream_capped(store, file_name, stream, max_bytes, strip, STRIP_MAX_BYTES).await
}

/// [`stage_stream`] with the strip buffer limit (`strip_cap`) given, for tests.
async fn stage_stream_capped<S>(
    store: &MediaStore,
    file_name: &str,
    stream: S,
    max_bytes: u64,
    strip: bool,
    strip_cap: usize,
) -> Result<StagedFile, MediaError>
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + Unpin,
{
    // The head loop may see the end; the rest is chained after it and polled again.
    let mut stream = stream.fuse();
    let original_name = file_name.to_owned();
    let svg_limit = max_bytes.min(MAX_SVG_BYTES as u64);

    let mut head = BytesMut::new();
    let mut ended = false;
    while head.len() < HEAD_BYTES {
        match stream.next().await.transpose().map_err(io_error)? {
            Some(chunk) => {
                if (head.len() + chunk.len()) as u64 > max_bytes {
                    return Err(MediaError::TooLarge { limit: max_bytes });
                }
                head.extend_from_slice(&chunk);
            }
            None => {
                ended = true;
                break;
            }
        }
    }
    let sniff = &head[..head.len().min(HEAD_BYTES)];

    if looks_like_svg(sniff) {
        let mut all = head;
        while !ended {
            match stream.next().await.transpose().map_err(io_error)? {
                Some(chunk) => {
                    if (all.len() + chunk.len()) as u64 > svg_limit {
                        return Err(MediaError::TooLarge { limit: svg_limit });
                    }
                    all.extend_from_slice(&chunk);
                }
                None => ended = true,
            }
        }
        if all.len() as u64 > svg_limit {
            return Err(MediaError::TooLarge { limit: svg_limit });
        }
        let input = all.freeze();
        let clean = tokio::task::spawn_blocking(move || sanitize_svg(&input))
            .await
            .map_err(|e| MediaError::Task(e.to_string()))?
            .map_err(MediaError::Svg)?;
        let artifact = store.put_artifact(Bytes::from(clean)).await?;
        return Ok(staged(artifact, SVG_MIME.to_owned(), original_name, true));
    }

    // The head decides; it is cut when the file goes on past it.
    let cut = !ended || head.len() > HEAD_BYTES;
    let mime = classify(sniff, cut, &original_name)?;
    if strip && strippable(&mime) {
        return stage_stripped(
            store,
            mime,
            original_name,
            head,
            ended,
            stream,
            (max_bytes, strip_cap),
        )
        .await;
    }
    let stream = futures::stream::once(async move { Ok(head.freeze()) }).chain(stream);
    let artifact = store.put_stream(Box::pin(stream), max_bytes).await?;
    Ok(staged(artifact, mime, original_name, false))
}

/// Reads the rest of an image after `head`, strips it and stores the result. A file
/// over `cap` or one the walker rejects is refused ([`MediaError::Metadata`]): with
/// stripping on, a file keeping its metadata is never stored.
async fn stage_stripped<S>(
    store: &MediaStore,
    mime: String,
    original_name: String,
    head: BytesMut,
    mut ended: bool,
    mut stream: futures::stream::Fuse<S>,
    (max_bytes, cap): (u64, usize),
) -> Result<StagedFile, MediaError>
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + Unpin,
{
    let refuse = |reason: &str| {
        tracing::warn!(
            file = %sanitize_filename(&original_name),
            reason,
            "image refused: its metadata cannot be removed"
        );
        MediaError::Metadata(reason.to_owned())
    };
    let mut all = head;
    if all.len() > cap {
        return Err(refuse("the image is too large to strip"));
    }
    while !ended {
        match stream.next().await.transpose().map_err(io_error)? {
            Some(chunk) => {
                if (all.len() + chunk.len()) as u64 > max_bytes {
                    return Err(MediaError::TooLarge { limit: max_bytes });
                }
                all.extend_from_slice(&chunk);
                if all.len() > cap {
                    return Err(refuse("the image is too large to strip"));
                }
            }
            None => ended = true,
        }
    }
    let original = all.freeze();
    let kept = original.clone();
    let kind = mime.clone();
    let stripped = tokio::task::spawn_blocking(move || strip_metadata(&kept, &kind))
        .await
        .map_err(|e| MediaError::Task(e.to_string()))?;
    let bytes = match stripped {
        Ok(Some(clean)) => Bytes::from(clean),
        Ok(None) => original,
        Err(_) => return Err(refuse("the image could not be parsed")),
    };
    let artifact = store.put_artifact(bytes).await?;
    Ok(staged(artifact, mime, original_name, false))
}

fn io_error(e: io::Error) -> MediaError {
    MediaError::Io(e.to_string())
}

fn staged(
    artifact: ArtifactRef,
    mime: String,
    original_name: String,
    sanitized: bool,
) -> StagedFile {
    StagedFile {
        bytes: artifact.bytes,
        artifact,
        raster: is_raster_image(&mime),
        mime,
        original_name,
        sanitized,
    }
}

/// The type of a non-SVG file from its `head` (the first [`HEAD_BYTES`]; `cut` when
/// the file goes on) and, within the ZIP and text classes, its file name's extension.
fn classify(head: &[u8], cut: bool, file_name: &str) -> Result<String, MediaError> {
    let sniffed = sniff_mime(head);
    let ext = extension(file_name);
    let by_ext = |table: &[(&str, &str)]| {
        ext.as_deref()
            .and_then(|e| table.iter().find(|(x, _)| *x == e))
            .map(|(_, mime)| (*mime).to_owned())
    };
    if inline_safe(&sniffed) {
        // infer's MP3 matcher is 2–3 bytes (`ID3`, `FF FB/F3/F2`): a `.txt`/`.csv` whose
        // head is plain text is text. `FF` is never UTF-8, so only an `ID3`-led note
        // qualifies; a real MP3 head has NULs or invalid UTF-8 and stays audio.
        if sniffed == MP3
            && is_plain_text(head, cut)
            && let Some(mime) = by_ext(&TEXT_TYPES)
        {
            return Ok(mime);
        }
        return Ok(sniffed);
    }
    if head.starts_with(ZIP_MAGIC)
        && let Some(mime) = by_ext(&ZIP_TYPES)
    {
        return Ok(mime);
    }
    if sniffed == UNKNOWN
        && is_plain_text(head, cut)
        && let Some(mime) = by_ext(&TEXT_TYPES)
    {
        return Ok(mime);
    }
    Err(MediaError::Unsupported { mime: sniffed })
}

/// The lowercase extension of the last `/` or `\` segment of `name`.
fn extension(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = last.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// `head` is UTF-8 (a UTF-8 BOM allowed; when `cut`, a character split at the end is
/// allowed) without NUL, and its first non-whitespace character is not `<`.
fn is_plain_text(head: &[u8], cut: bool) -> bool {
    let body = head.strip_prefix(UTF8_BOM).unwrap_or(head);
    if body.contains(&0) {
        return false;
    }
    let text = match std::str::from_utf8(body) {
        Ok(text) => text,
        // Only an incomplete last character of a head that was cut.
        Err(e) if cut && e.error_len().is_none() => {
            match std::str::from_utf8(&body[..e.valid_up_to()]) {
                Ok(text) => text,
                Err(_) => return false,
            }
        }
        Err(_) => return false,
    };
    !text.trim_start().starts_with('<')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Backend, MediaConfig, PublicUrlMode};

    fn local_store() -> (tempfile::TempDir, MediaStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: dir.path().join("media"),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .expect("open store");
        (dir, store)
    }

    /// `data` as a stream of `chunk`-byte pieces.
    fn chunks(
        data: &[u8],
        chunk: usize,
    ) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + Unpin + use<> {
        let parts: Vec<_> = data
            .chunks(chunk)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        futures::stream::iter(parts)
    }

    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::new_rgb8(4, 3)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode png");
        out
    }

    #[tokio::test]
    async fn ingest_a_png_is_a_raster_image() {
        let (_dir, store) = local_store();
        let data = png();
        let staged = stage_stream(&store, "dir/photo.png", chunks(&data, 7), 1 << 20, false)
            .await
            .expect("staged");
        assert_eq!(staged.mime, "image/png");
        assert!(staged.raster);
        assert!(!staged.sanitized);
        assert_eq!(staged.bytes, data.len() as u64);
        assert_eq!(staged.original_name, "dir/photo.png");
        assert_eq!(store.get(&staged.artifact.hash).await.unwrap(), data);
    }

    #[tokio::test]
    async fn ingest_an_svg_is_sanitised() {
        let (_dir, store) = local_store();
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><rect width="1" height="1"/></svg>"#;
        let staged = stage_stream(&store, "logo.svg", chunks(svg, 5), 1 << 20, false)
            .await
            .expect("staged");
        assert_eq!(staged.mime, "image/svg+xml");
        assert!(staged.sanitized);
        assert!(!staged.raster);
        let stored = store.get(&staged.artifact.hash).await.unwrap();
        let text = std::str::from_utf8(&stored).unwrap();
        assert!(!text.contains("script"), "{text}");
        assert!(text.contains("<rect"), "{text}");
        assert_eq!(staged.bytes, stored.len() as u64);
    }

    #[tokio::test]
    async fn ingest_a_csv_is_text_by_its_extension() {
        let (_dir, store) = local_store();
        let staged = stage_stream(&store, "data.CSV", chunks(b"a,b\n1,2\n", 3), 1 << 20, false)
            .await
            .expect("staged");
        assert_eq!(staged.mime, "text/csv");
        assert!(!staged.raster);
        assert!(!staged.sanitized);
    }

    #[tokio::test]
    async fn ingest_never_polls_an_ended_stream() {
        // `unfold` panics when polled after it ended, as a multipart field stream does.
        let (_dir, store) = local_store();
        let one = |data: Vec<u8>| {
            Box::pin(futures::stream::unfold(Some(data), |left| async move {
                left.map(|d| (Ok(Bytes::from(d)), None))
            }))
        };
        let staged = stage_stream(&store, "a.csv", one(b"a,b\n".to_vec()), 1 << 20, false)
            .await
            .expect("short file");
        assert_eq!(staged.mime, "text/csv");
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#.to_vec();
        let staged = stage_stream(&store, "a.svg", one(svg), 1 << 20, false)
            .await
            .expect("short svg");
        assert!(staged.sanitized);
    }

    #[tokio::test]
    async fn ingest_over_max_bytes_is_too_large() {
        let (_dir, store) = local_store();
        let data = png();
        let max = data.len() as u64 - 1;
        let err = stage_stream(&store, "photo.png", chunks(&data, 16), max, false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MediaError::TooLarge { limit } if limit == max),
            "{err:?}"
        );
        // Past the head, while it streams.
        let big = [png(), vec![0; 3 * HEAD_BYTES]].concat();
        let max = 2 * HEAD_BYTES as u64;
        let err = stage_stream(&store, "photo.png", chunks(&big, 1000), max, false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, MediaError::TooLarge { limit } if limit == max),
            "{err:?}"
        );
    }

    #[test]
    fn classes_follow_content_then_extension() {
        let ok = |head: &[u8], name: &str| classify(head, false, name).ok();
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(ok(png, "x.svg").as_deref(), Some("image/png"));
        assert_eq!(ok(png, "x.txt").as_deref(), Some("image/png"));
        assert_eq!(
            ok(b"%PDF-1.7\n", "x.docx").as_deref(),
            Some("application/pdf")
        );
        let zip = b"PK\x03\x04\x14\0\0\0\x08\0";
        assert_eq!(
            ok(zip, "A.DOCX").as_deref(),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
        assert_eq!(ok(zip, "x.zip").as_deref(), Some("application/zip"));
        assert_eq!(ok(zip, "x.txt"), None);
        assert_eq!(ok(zip, "x.exe"), None);
        assert_eq!(ok(zip, "docx"), None);
        assert_eq!(ok(b"hello", "notes.txt").as_deref(), Some("text/plain"));
        assert_eq!(ok(b"a,b\n", "d.CSV").as_deref(), Some("text/csv"));
        assert_eq!(ok(b"", "e.txt").as_deref(), Some("text/plain"));
        assert_eq!(
            ok(b"\xEF\xBB\xBFhi", "b.txt").as_deref(),
            Some("text/plain")
        );
        assert_eq!(ok(b"hello", "notes.md"), None);
        assert_eq!(ok(b"hello", ".txt"), None);
        assert_eq!(ok(b" \n<html>", "x.txt"), None);
        assert_eq!(ok(b"\xEF\xBB\xBF<p>", "x.txt"), None);
        assert_eq!(ok(b"\xFF\xFEh\0i\0", "x.txt"), None);
        assert_eq!(ok(b"bad \xC3\x28 utf8", "x.txt"), None);
        // The weak MP3 match yields to text only for a text name and a text head.
        assert_eq!(
            ok(b"ID3 tags, explained", "n.txt").as_deref(),
            Some("text/plain")
        );
        assert_eq!(
            ok(b"ID3 tags, explained", "n.mp3").as_deref(),
            Some("audio/mpeg")
        );
        assert_eq!(
            ok(b"ID3\x04\0\0\0\0", "n.txt").as_deref(),
            Some("audio/mpeg")
        );
        assert_eq!(ok(b"\xFF\xFBrest", "n.txt").as_deref(), Some("audio/mpeg"));
        assert!(matches!(
            classify(b"\x7fELF\x02\x01\x01\0", false, "x.txt"),
            Err(MediaError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_character_split_by_the_head_cut_is_text() {
        // "é" is C3 A9; the head ends after C3.
        let head = b"caf\xC3";
        assert!(is_plain_text(head, true));
        assert!(!is_plain_text(head, false));
    }

    #[test]
    fn extensions_come_from_the_last_segment() {
        assert_eq!(extension("a/b.c/report.PDF").as_deref(), Some("pdf"));
        assert_eq!(extension("a\\b\\x.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension("dir.txt/noext"), None);
        assert_eq!(extension(".bashrc"), None);
        assert_eq!(extension("trailing."), None);
    }

    /// A JPEG with an XMP APP1 segment and an Exif one (Orientation 6) after SOI.
    fn jpeg_with_metadata() -> Vec<u8> {
        let mut plain = Vec::new();
        image::DynamicImage::new_rgb8(8, 8)
            .write_to(
                &mut std::io::Cursor::new(&mut plain),
                image::ImageFormat::Jpeg,
            )
            .expect("encode jpeg");
        let segment = |payload: &[u8]| {
            let mut s = vec![0xFF, 0xE1];
            s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
            s.extend_from_slice(payload);
            s
        };
        let mut xmp = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        xmp.extend_from_slice(b"<x:xmpmeta>where I live</x:xmpmeta>");
        // Big-endian TIFF with Orientation 6, then junk standing in for a GPS block.
        let mut exif =
            b"Exif\0\0MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0\0\0\0\0".to_vec();
        exif.extend_from_slice(&[0x47; 40]);
        let mut out = plain[..2].to_vec();
        out.extend(segment(&xmp));
        out.extend(segment(&exif));
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[tokio::test]
    async fn ingest_hashes_stripped_bytes_when_on() {
        let (_dir, store) = local_store();
        let data = jpeg_with_metadata();
        let expected = strip_metadata(&data, "image/jpeg").unwrap().unwrap();
        assert!(expected.len() < data.len());
        let staged = stage_stream(&store, "me.jpg", chunks(&data, 13), 1 << 20, true)
            .await
            .expect("staged");
        assert_eq!(staged.mime, "image/jpeg");
        assert!(staged.raster);
        assert_eq!(staged.bytes, expected.len() as u64);
        assert_eq!(store.get(&staged.artifact.hash).await.unwrap(), expected);
        // The hash is the stripped bytes': staging the stripped file again is a duplicate.
        let again = stage_stream(&store, "me.jpg", chunks(&expected, 64), 1 << 20, true)
            .await
            .expect("staged again");
        assert_eq!(again.artifact.hash, staged.artifact.hash);
        assert!(!again.artifact.created);
    }

    #[tokio::test]
    async fn off_by_default_keeps_bytes() {
        let (_dir, store) = local_store();
        let data = jpeg_with_metadata();
        let staged = stage_stream(&store, "me.jpg", chunks(&data, 13), 1 << 20, false)
            .await
            .expect("staged");
        assert_eq!(staged.bytes, data.len() as u64);
        assert_eq!(store.get(&staged.artifact.hash).await.unwrap(), data);
    }

    #[tokio::test]
    async fn corrupt_image_is_refused_when_stripping() {
        let (_dir, store) = local_store();
        // A JPEG head that sniffs as JPEG, then a segment that claims more than is there.
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xF0];
        data.extend_from_slice(b"Exif\0\0 truncated");
        let err = stage_stream(&store, "bad.jpg", chunks(&data, 5), 1 << 20, true)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::Metadata(_)), "{err}");
        // With the option off the same bytes are stored unchanged.
        let staged = stage_stream(&store, "bad.jpg", chunks(&data, 5), 1 << 20, false)
            .await
            .expect("stored");
        assert_eq!(store.get(&staged.artifact.hash).await.unwrap(), data);
    }

    #[tokio::test]
    async fn truncated_png_and_oversize_riff_webp_are_refused() {
        let (_dir, store) = local_store();
        let png = png();
        let truncated = &png[..png.len() - 12]; // no IEND
        let mut webp = Vec::new();
        image::DynamicImage::new_rgb8(4, 4)
            .write_to(
                &mut std::io::Cursor::new(&mut webp),
                image::ImageFormat::WebP,
            )
            .expect("encode webp");
        let oversize = webp.len() as u32 + 100;
        webp[4..8].copy_from_slice(&oversize.to_le_bytes());
        for (name, data) in [("a.png", truncated.to_vec()), ("a.webp", webp)] {
            let err = stage_stream(&store, name, chunks(&data, 9), 1 << 20, true)
                .await
                .unwrap_err();
            assert!(matches!(err, MediaError::Metadata(_)), "{name}: {err}");
            let kept = stage_stream(&store, name, chunks(&data, 9), 1 << 20, false)
                .await
                .expect("strip off stores it unchanged");
            assert_eq!(
                store.get(&kept.artifact.hash).await.unwrap(),
                data,
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn over_the_strip_cap_is_refused() {
        let (_dir, store) = local_store();
        let data = jpeg_with_metadata();
        let cap = data.len() - 1;
        let err = stage_stream_capped(&store, "me.jpg", chunks(&data, 16), 1 << 20, true, cap)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::Metadata(_)), "{err}");
        // A cap that fits stores the stripped file; off, the cap is not applied.
        stage_stream_capped(
            &store,
            "me.jpg",
            chunks(&data, 16),
            1 << 20,
            true,
            data.len(),
        )
        .await
        .expect("fits");
        let kept = stage_stream_capped(&store, "me.jpg", chunks(&data, 16), 1 << 20, false, cap)
            .await
            .expect("strip off");
        assert_eq!(store.get(&kept.artifact.hash).await.unwrap(), data);
    }

    #[tokio::test]
    async fn strip_respects_the_size_limit() {
        let (_dir, store) = local_store();
        let data = jpeg_with_metadata();
        let err = stage_stream(&store, "me.jpg", chunks(&data, 16), 100, true)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::TooLarge { limit: 100 }));
    }
}
