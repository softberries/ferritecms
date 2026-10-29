//! Lossless EXIF/XMP stripping of JPEG, PNG and WebP originals (M4-1 P17).
//!
//! [`strip_metadata`] walks the container (JPEG segments, PNG chunks, WebP RIFF chunks)
//! and copies everything but the metadata blocks byte for byte: no pixel is decoded or
//! re-encoded, ICC data stays. An EXIF Orientation other than 1 is written back as a
//! minimal EXIF block that holds only that tag, so the picture still displays upright.

use crate::MediaError;

const EXIF_TAG_ORIENTATION: u16 = 0x0112;
const JPEG_EXIF: &[u8] = b"Exif\0\0";
const JPEG_XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const JPEG_XMP_EXT: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
/// ImageMagick's legacy PNG carriers of EXIF/APP1/XMP profiles (ICC's is not listed).
const PNG_RAW_PROFILES: [&[u8]; 3] = [
    b"Raw profile type exif",
    b"Raw profile type APP1",
    b"Raw profile type xmp",
];
const VP8X_EXIF_FLAG: u8 = 0x08;
const VP8X_XMP_FLAG: u8 = 0x04;

fn bad(what: &str) -> MediaError {
    MediaError::UnsupportedImage(format!("metadata strip: {what}"))
}

/// Removes EXIF and XMP from `bytes` of type `mime` (`image/jpeg`, `image/png` or
/// `image/webp`; any other type gives `None`). `Ok(None)` means nothing to remove, so
/// the file stays as it is; `Err` means the container could not be parsed (the caller
/// stores the original unchanged).
pub fn strip_metadata(bytes: &[u8], mime: &str) -> Result<Option<Vec<u8>>, MediaError> {
    match mime {
        "image/jpeg" => strip_jpeg(bytes),
        "image/png" => strip_png(bytes),
        "image/webp" => strip_webp(bytes),
        _ => Ok(None),
    }
}

/// The entries of the first IFD of a TIFF structure (`II*\0` / `MM\0*` header):
/// `(tag, type, first two value bytes as a number)`.
#[cfg(test)]
fn ifd0_entries(tiff: &[u8]) -> Option<Vec<(u16, u16, u16)>> {
    let (le, ifd, count) = ifd0(tiff)?;
    (0..count)
        .map(|i| {
            let e = ifd + 2 + i * 12;
            Some((
                rd16(tiff, e, le)?,
                rd16(tiff, e + 2, le)?,
                rd16(tiff, e + 8, le)?,
            ))
        })
        .collect()
}

fn rd16(t: &[u8], at: usize, le: bool) -> Option<u16> {
    let b: [u8; 2] = t.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(b)
    } else {
        u16::from_be_bytes(b)
    })
}

fn rd32(t: &[u8], at: usize, le: bool) -> Option<u32> {
    let b: [u8; 4] = t.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(b)
    } else {
        u32::from_be_bytes(b)
    })
}

/// `(little endian, offset of the first IFD, its entry count)`.
fn ifd0(tiff: &[u8]) -> Option<(bool, usize, usize)> {
    let le = match tiff.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let ifd = usize::try_from(rd32(tiff, 4, le)?).ok()?;
    Some((le, ifd, usize::from(rd16(tiff, ifd, le)?)))
}

/// The Orientation (1..=8) of a TIFF structure, if present.
fn tiff_orientation(tiff: &[u8]) -> Option<u16> {
    let (le, ifd, count) = ifd0(tiff)?;
    for i in 0..count {
        let entry = ifd.checked_add(2 + i * 12)?;
        if rd16(tiff, entry, le)? == EXIF_TAG_ORIENTATION {
            let value = rd16(tiff, entry + 8, le)?;
            return (1..=8).contains(&value).then_some(value);
        }
    }
    None
}

/// A TIFF structure (big endian) whose only entry is Orientation `value`.
fn minimal_tiff(value: u16) -> Vec<u8> {
    let mut t = Vec::with_capacity(26);
    t.extend_from_slice(b"MM\0*");
    t.extend_from_slice(&8u32.to_be_bytes());
    t.extend_from_slice(&1u16.to_be_bytes());
    t.extend_from_slice(&EXIF_TAG_ORIENTATION.to_be_bytes());
    t.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    t.extend_from_slice(&1u32.to_be_bytes());
    t.extend_from_slice(&value.to_be_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
    t
}

/// An Orientation worth keeping: anything but 1.
fn keep_orientation(found: Option<u16>) -> Option<u16> {
    found.filter(|o| *o != 1)
}

// JPEG ------------------------------------------------------------------------------

fn strip_jpeg(b: &[u8]) -> Result<Option<Vec<u8>>, MediaError> {
    if b.get(..2) != Some(&[0xFF, 0xD8]) {
        return Err(bad("not a JPEG"));
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[..2]);
    let mut pos = 2;
    let mut removed = false;
    let mut orientation = None;
    // Where a kept Orientation goes: right after a leading JFIF APP0, else after SOI.
    let mut insert_at = 2;
    let mut first_segment = true;
    loop {
        let start = pos;
        // Fill bytes (extra 0xFF) may precede a marker.
        while b.get(pos) == Some(&0xFF) && b.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        if b.get(pos) != Some(&0xFF) {
            return Err(bad("expected a JPEG marker"));
        }
        let marker = *b.get(pos + 1).ok_or_else(|| bad("truncated JPEG"))?;
        match marker {
            // Standalone markers: no length.
            0x01 | 0xD0..=0xD8 => {
                out.extend_from_slice(&b[start..pos + 2]);
                pos += 2;
                first_segment = false;
                continue;
            }
            0xD9 => {
                out.extend_from_slice(&b[start..]);
                break;
            }
            _ => {}
        }
        let len =
            usize::from(rd16(b, pos + 2, false).ok_or_else(|| bad("truncated JPEG segment"))?);
        if len < 2 {
            return Err(bad("bad JPEG segment length"));
        }
        let end = pos + 2 + len;
        let payload = b
            .get(pos + 4..end)
            .ok_or_else(|| bad("truncated JPEG segment"))?;
        if marker == 0xDA {
            // Start of scan: entropy-coded data to the end, verbatim.
            out.extend_from_slice(&b[start..]);
            break;
        }
        let drop_it = marker == 0xE1
            && (payload.starts_with(JPEG_EXIF)
                || payload.starts_with(JPEG_XMP)
                || payload.starts_with(JPEG_XMP_EXT));
        if drop_it {
            removed = true;
            if let Some(tiff) = payload.strip_prefix(JPEG_EXIF) {
                orientation = orientation.or(tiff_orientation(tiff));
            }
        } else {
            out.extend_from_slice(&b[start..end]);
            if first_segment && marker == 0xE0 && payload.starts_with(b"JFIF\0") {
                insert_at = out.len();
            }
        }
        first_segment = false;
        pos = end;
    }
    if !removed {
        return Ok(None);
    }
    if let Some(o) = keep_orientation(orientation) {
        let tiff = minimal_tiff(o);
        let mut seg = vec![0xFF, 0xE1];
        let len = u16::try_from(2 + JPEG_EXIF.len() + tiff.len()).map_err(|_| bad("exif"))?;
        seg.extend_from_slice(&len.to_be_bytes());
        seg.extend_from_slice(JPEG_EXIF);
        seg.extend_from_slice(&tiff);
        out.splice(insert_at..insert_at, seg);
    }
    Ok(Some(out))
}

// PNG -------------------------------------------------------------------------------

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &byte in *part {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
            }
        }
    }
    !crc
}

/// The keyword of a `tEXt`/`zTXt`/`iTXt` chunk: its text up to the first NUL.
fn png_keyword(data: &[u8]) -> &[u8] {
    data.split(|b| *b == 0).next().unwrap_or(&[])
}

fn strip_png(b: &[u8]) -> Result<Option<Vec<u8>>, MediaError> {
    if b.get(..8) != Some(PNG_SIG) {
        return Err(bad("not a PNG"));
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(PNG_SIG);
    let mut pos = 8;
    let mut removed = false;
    let mut orientation = None;
    let mut insert_at = None;
    let mut ended = false;
    while pos < b.len() {
        let len = rd32(b, pos, false).ok_or_else(|| bad("truncated PNG chunk"))?;
        let len = usize::try_from(len).map_err(|_| bad("PNG chunk length"))?;
        let kind = b
            .get(pos + 4..pos + 8)
            .ok_or_else(|| bad("truncated PNG chunk"))?;
        let end = pos
            .checked_add(12 + len)
            .filter(|e| *e <= b.len())
            .ok_or_else(|| bad("truncated PNG chunk"))?;
        let data = &b[pos + 8..pos + 8 + len];
        let drop_it = match kind {
            b"eXIf" => {
                orientation = orientation.or(tiff_orientation(data));
                true
            }
            b"iTXt" | b"tEXt" | b"zTXt" => {
                let key = png_keyword(data);
                key == PNG_XMP_KEYWORD || PNG_RAW_PROFILES.contains(&key)
            }
            _ => false,
        };
        if drop_it {
            removed = true;
        } else {
            if kind == b"IDAT" && insert_at.is_none() {
                insert_at = Some(out.len());
            }
            out.extend_from_slice(&b[pos..end]);
        }
        pos = end;
        if kind == b"IEND" {
            ended = true;
            // Anything after IEND is copied as it is.
            out.extend_from_slice(&b[pos..]);
            break;
        }
    }
    if !ended {
        return Err(bad("PNG without IEND"));
    }
    if !removed {
        return Ok(None);
    }
    if let (Some(o), Some(at)) = (keep_orientation(orientation), insert_at) {
        let tiff = minimal_tiff(o);
        let mut chunk = Vec::with_capacity(12 + tiff.len());
        chunk.extend_from_slice(
            &u32::try_from(tiff.len())
                .map_err(|_| bad("exif"))?
                .to_be_bytes(),
        );
        chunk.extend_from_slice(b"eXIf");
        chunk.extend_from_slice(&tiff);
        chunk.extend_from_slice(&crc32(&[b"eXIf", &tiff]).to_be_bytes());
        out.splice(at..at, chunk);
    }
    Ok(Some(out))
}

// WebP ------------------------------------------------------------------------------

fn strip_webp(b: &[u8]) -> Result<Option<Vec<u8>>, MediaError> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return Err(bad("not a WebP"));
    }
    let riff_len = usize::try_from(rd32(b, 4, true).ok_or_else(|| bad("RIFF length"))?)
        .map_err(|_| bad("RIFF length"))?;
    let end = riff_len
        .checked_add(8)
        .filter(|e| *e <= b.len() && *e >= 12)
        .ok_or_else(|| bad("truncated WebP"))?;
    let mut body = Vec::with_capacity(b.len());
    let mut pos = 12;
    let mut removed = false;
    let mut orientation = None;
    let mut vp8x_at = None;
    while pos < end {
        if pos + 8 > end {
            return Err(bad("truncated WebP chunk"));
        }
        let fourcc = &b[pos..pos + 4];
        let len = usize::try_from(rd32(b, pos + 4, true).ok_or_else(|| bad("WebP chunk"))?)
            .map_err(|_| bad("WebP chunk length"))?;
        let chunk_end = pos + 8 + len;
        if chunk_end > end {
            return Err(bad("truncated WebP chunk"));
        }
        // Chunks are padded to an even size.
        let next = (chunk_end + (len & 1)).min(end);
        match fourcc {
            b"EXIF" | b"XMP " => {
                removed = true;
                if fourcc == b"EXIF" {
                    let data = &b[pos + 8..chunk_end];
                    let tiff = data.strip_prefix(JPEG_EXIF).unwrap_or(data);
                    orientation = orientation.or(tiff_orientation(tiff));
                }
            }
            _ => {
                if fourcc == b"VP8X" {
                    if len < 10 {
                        return Err(bad("short VP8X"));
                    }
                    vp8x_at = Some(body.len());
                }
                body.extend_from_slice(&b[pos..chunk_end]);
                if len & 1 == 1 {
                    body.push(0);
                }
            }
        }
        pos = next;
    }
    if !removed {
        return Ok(None);
    }
    // Only an extended file (VP8X) can carry the EXIF chunk.
    let keep = vp8x_at.and(keep_orientation(orientation));
    if let Some(at) = vp8x_at {
        let flags = &mut body[at + 8];
        *flags &= !(VP8X_EXIF_FLAG | VP8X_XMP_FLAG);
        if keep.is_some() {
            *flags |= VP8X_EXIF_FLAG;
        }
    }
    if let Some(o) = keep {
        let tiff = minimal_tiff(o);
        body.extend_from_slice(b"EXIF");
        body.extend_from_slice(
            &u32::try_from(tiff.len())
                .map_err(|_| bad("exif"))?
                .to_le_bytes(),
        );
        body.extend_from_slice(&tiff);
        if tiff.len() & 1 == 1 {
            body.push(0);
        }
    }
    let mut out = Vec::with_capacity(12 + body.len());
    out.extend_from_slice(b"RIFF");
    let riff = u32::try_from(4 + body.len()).map_err(|_| bad("WebP size"))?;
    out.extend_from_slice(&riff.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbImage};
    use std::io::Cursor;

    fn picture() -> DynamicImage {
        let mut img = RgbImage::new(16, 8);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 15) as u8, (y * 30) as u8, 77]);
        }
        DynamicImage::ImageRgb8(img)
    }

    fn encode(format: ImageFormat) -> Vec<u8> {
        let mut out = Vec::new();
        picture()
            .write_to(&mut Cursor::new(&mut out), format)
            .expect("encode");
        out
    }

    fn pixels(bytes: &[u8]) -> Vec<u8> {
        image::load_from_memory(bytes)
            .expect("decodes")
            .to_rgb8()
            .into_raw()
    }

    /// A big-endian TIFF: IFD0 holds Orientation and a GPS IFD pointer; the GPS IFD one
    /// entry (GPSLatitudeRef).
    fn tiff_with_gps(orientation: u16) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"MM\0*");
        t.extend_from_slice(&8u32.to_be_bytes());
        t.extend_from_slice(&2u16.to_be_bytes());
        for (tag, ty, value) in [(0x0112u16, 3u16, orientation as u32), (0x8825, 4, 38)] {
            t.extend_from_slice(&tag.to_be_bytes());
            t.extend_from_slice(&ty.to_be_bytes());
            t.extend_from_slice(&1u32.to_be_bytes());
            if ty == 3 {
                t.extend_from_slice(&(value as u16).to_be_bytes());
                t.extend_from_slice(&[0, 0]);
            } else {
                t.extend_from_slice(&value.to_be_bytes());
            }
        }
        t.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(t.len(), 38);
        t.extend_from_slice(&1u16.to_be_bytes());
        t.extend_from_slice(&1u16.to_be_bytes());
        t.extend_from_slice(&2u16.to_be_bytes());
        t.extend_from_slice(&2u32.to_be_bytes());
        t.extend_from_slice(b"N\0\0\0");
        t.extend_from_slice(&0u32.to_be_bytes());
        t
    }

    const XMP: &[u8] = b"<x:xmpmeta>secret location</x:xmpmeta>";

    fn jpeg_segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut s = vec![0xFF, marker];
        s.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        s.extend_from_slice(payload);
        s
    }

    /// The segments `(marker, payload)` of a JPEG up to the scan.
    fn jpeg_segments(b: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = 2;
        while pos + 4 <= b.len() && b[pos + 1] != 0xDA {
            let len = u16::from_be_bytes([b[pos + 2], b[pos + 3]]) as usize;
            out.push((b[pos + 1], b[pos + 4..pos + 2 + len].to_vec()));
            pos += 2 + len;
        }
        out
    }

    fn jpeg_with_metadata(orientation: u16) -> Vec<u8> {
        let plain = encode(ImageFormat::Jpeg);
        let mut exif = JPEG_EXIF.to_vec();
        exif.extend_from_slice(&tiff_with_gps(orientation));
        let mut xmp = JPEG_XMP.to_vec();
        xmp.extend_from_slice(XMP);
        let mut icc = b"ICC_PROFILE\0\x01\x01".to_vec();
        icc.extend_from_slice(&[7; 40]);
        let mut out = plain[..2].to_vec();
        out.extend(jpeg_segment(0xE1, &exif));
        out.extend(jpeg_segment(0xE1, &xmp));
        out.extend(jpeg_segment(0xE2, &icc));
        out.extend_from_slice(&plain[2..]);
        out
    }

    #[test]
    fn jpeg_gps_removed_orientation_kept_icc_kept() {
        let src = jpeg_with_metadata(6);
        let out = strip_metadata(&src, "image/jpeg")
            .unwrap()
            .expect("changed");
        assert!(!out.windows(XMP.len()).any(|w| w == XMP));
        assert_eq!(pixels(&out), pixels(&src), "pixels are identical");
        let segs = jpeg_segments(&out);
        let exifs: Vec<_> = segs
            .iter()
            .filter(|(m, p)| *m == 0xE1 && p.starts_with(JPEG_EXIF))
            .collect();
        assert_eq!(exifs.len(), 1);
        let tiff = &exifs[0].1[JPEG_EXIF.len()..];
        assert_eq!(ifd0_entries(tiff).unwrap(), vec![(0x0112, 3, 6)]);
        assert!(
            segs.iter()
                .any(|(m, p)| *m == 0xE2 && p.starts_with(b"ICC_PROFILE\0")),
            "ICC kept"
        );
        assert!(!segs.iter().any(|(_, p)| p.starts_with(JPEG_XMP)));
    }

    #[test]
    fn jpeg_orientation_one_leaves_no_exif() {
        let out = strip_metadata(&jpeg_with_metadata(1), "image/jpeg")
            .unwrap()
            .unwrap();
        assert!(!jpeg_segments(&out).iter().any(|(m, _)| *m == 0xE1));
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
        c
    }

    fn png_chunks(b: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = 8;
        while pos < b.len() {
            let len = u32::from_be_bytes(b[pos..pos + 4].try_into().unwrap()) as usize;
            out.push((
                b[pos + 4..pos + 8].try_into().unwrap(),
                b[pos + 8..pos + 8 + len].to_vec(),
            ));
            pos += 12 + len;
        }
        out
    }

    fn png_with_metadata() -> Vec<u8> {
        let plain = encode(ImageFormat::Png);
        // After IHDR (8 signature + 25).
        let mut out = plain[..33].to_vec();
        out.extend(png_chunk(b"eXIf", &tiff_with_gps(8)));
        let mut xmp = PNG_XMP_KEYWORD.to_vec();
        xmp.extend_from_slice(&[0, 0, 0, 0, 0]);
        xmp.extend_from_slice(XMP);
        out.extend(png_chunk(b"iTXt", &xmp));
        out.extend(png_chunk(b"tEXt", b"Comment\0kept"));
        out.extend_from_slice(&plain[33..]);
        out
    }

    #[test]
    fn png_exif_and_xmp_removed() {
        let src = png_with_metadata();
        let out = strip_metadata(&src, "image/png").unwrap().expect("changed");
        assert_eq!(pixels(&out), pixels(&src));
        assert!(!out.windows(XMP.len()).any(|w| w == XMP));
        let chunks = png_chunks(&out);
        let exif: Vec<_> = chunks.iter().filter(|(k, _)| k == b"eXIf").collect();
        assert_eq!(exif.len(), 1, "orientation 8 is written back");
        assert_eq!(ifd0_entries(&exif[0].1).unwrap(), vec![(0x0112, 3, 8)]);
        assert!(!chunks.iter().any(|(k, _)| k == b"iTXt"));
        assert!(
            chunks
                .iter()
                .any(|(k, d)| k == b"tEXt" && d.starts_with(b"Comment"))
        );
        // The written chunk's CRC is valid: the decoder (which checks it) passed above.
        let first_idat = chunks.iter().position(|(k, _)| k == b"IDAT").unwrap();
        let exif_at = chunks.iter().position(|(k, _)| k == b"eXIf").unwrap();
        assert!(exif_at < first_idat);
    }

    fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = kind.to_vec();
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    fn webp_chunks(b: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = 12;
        while pos + 8 <= b.len() {
            let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
            out.push((
                b[pos..pos + 4].try_into().unwrap(),
                b[pos + 8..pos + 8 + len].to_vec(),
            ));
            pos += 8 + len + (len & 1);
        }
        out
    }

    fn webp_with_metadata(orientation: u16) -> Vec<u8> {
        let plain = encode(ImageFormat::WebP);
        let image: Vec<u8> = {
            let chunks = webp_chunks(&plain);
            assert_eq!(chunks.len(), 1, "a simple lossless WebP");
            riff_chunk(&chunks[0].0, &chunks[0].1)
        };
        let mut vp8x = vec![VP8X_EXIF_FLAG | VP8X_XMP_FLAG | 0x20, 0, 0, 0];
        vp8x.extend_from_slice(&[15, 0, 0, 7, 0, 0]);
        let mut body = b"WEBP".to_vec();
        body.extend(riff_chunk(b"VP8X", &vp8x));
        body.extend(&image);
        body.extend(riff_chunk(b"EXIF", &tiff_with_gps(orientation)));
        body.extend(riff_chunk(b"XMP ", XMP));
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn webp_exif_xmp_removed_flags_fixed() {
        let src = webp_with_metadata(1);
        assert_eq!(pixels(&src).len(), 16 * 8 * 3, "the fixture decodes");
        let out = strip_metadata(&src, "image/webp")
            .unwrap()
            .expect("changed");
        assert_eq!(pixels(&out), pixels(&src), "it still decodes, same pixels");
        let chunks = webp_chunks(&out);
        assert!(!chunks.iter().any(|(k, _)| k == b"EXIF" || k == b"XMP "));
        let vp8x = &chunks.iter().find(|(k, _)| k == b"VP8X").unwrap().1;
        assert_eq!(vp8x[0] & (VP8X_EXIF_FLAG | VP8X_XMP_FLAG), 0);
        assert_eq!(vp8x[0] & 0x20, 0x20, "the ICC flag stays");
        let riff = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff + 8, out.len(), "the RIFF size is fixed");
    }

    #[test]
    fn webp_orientation_written_back() {
        let out = strip_metadata(&webp_with_metadata(3), "image/webp")
            .unwrap()
            .unwrap();
        let chunks = webp_chunks(&out);
        let exif = &chunks.iter().find(|(k, _)| k == b"EXIF").unwrap().1;
        assert_eq!(ifd0_entries(exif).unwrap(), vec![(0x0112, 3, 3)]);
        let vp8x = &chunks.iter().find(|(k, _)| k == b"VP8X").unwrap().1;
        assert_eq!(vp8x[0] & (VP8X_EXIF_FLAG | VP8X_XMP_FLAG), VP8X_EXIF_FLAG);
        assert_eq!(pixels(&out), pixels(&webp_with_metadata(3)));
    }

    #[test]
    fn no_metadata_returns_none() {
        for (format, mime) in [
            (ImageFormat::Jpeg, "image/jpeg"),
            (ImageFormat::Png, "image/png"),
            (ImageFormat::WebP, "image/webp"),
        ] {
            assert!(
                strip_metadata(&encode(format), mime).unwrap().is_none(),
                "{mime}"
            );
        }
        assert!(strip_metadata(b"GIF89a", "image/gif").unwrap().is_none());
    }

    #[test]
    fn corrupt_image_is_an_error_not_a_panic() {
        let jpeg = jpeg_with_metadata(6);
        let png = png_with_metadata();
        let webp = webp_with_metadata(6);
        for (bytes, mime) in [
            (&jpeg, "image/jpeg"),
            (&png, "image/png"),
            (&webp, "image/webp"),
        ] {
            for cut in [0, 1, 5, 20, bytes.len() / 2, bytes.len() - 3] {
                let _ = strip_metadata(&bytes[..cut], mime);
            }
            assert!(strip_metadata(b"not an image at all", mime).is_err());
        }
    }
}
