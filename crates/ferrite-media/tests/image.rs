#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;
use std::time::Instant;

use ferrite_media::{MediaError, default_variants, process_image};
use image::codecs::gif::GifEncoder;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};

fn gradient(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 128]))
}

fn encode(img: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut out, format)
        .expect("encode fixture");
    out.into_inner()
}

/// Inserts an APP1 Exif segment (orientation tag only) right after SOI.
fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM\x00\x2a\x00\x00\x00\x08");
    tiff.extend_from_slice(&1u16.to_be_bytes()); // one IFD entry
    tiff.extend_from_slice(&0x0112u16.to_be_bytes()); // Orientation
    tiff.extend_from_slice(&3u16.to_be_bytes()); // SHORT
    tiff.extend_from_slice(&1u32.to_be_bytes()); // count
    tiff.extend_from_slice(&orientation.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    seg.extend_from_slice(&payload);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&seg);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn variant<'a>(p: &'a ferrite_media::Processed, name: &str) -> &'a ferrite_media::Variant {
    p.variants
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("variant {name} missing"))
}

#[test]
fn variants_sizes() {
    let data = encode(&gradient(2000, 1000), ImageFormat::Jpeg);
    let p = process_image(&data, &default_variants()).unwrap();
    assert_eq!((p.width, p.height), (2000, 1000));
    let t = variant(&p, "thumbnail");
    assert_eq!((t.width, t.height), (150, 150));
    let m = variant(&p, "medium");
    assert_eq!((m.width, m.height), (768, 384));
    let l = variant(&p, "large");
    assert_eq!((l.width, l.height), (1600, 800));
    assert_eq!(m.mime, "image/jpeg");
    let decoded = image::load_from_memory(&m.data).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (768, 384));
}

#[test]
fn no_upscale() {
    let data = encode(&gradient(400, 300), ImageFormat::Jpeg);
    let p = process_image(&data, &default_variants()).unwrap();
    assert_eq!(p.variants.len(), 1);
    let t = variant(&p, "thumbnail");
    assert_eq!((t.width, t.height), (150, 150));
}

#[test]
fn small_source_thumbnail_keeps_square_of_min_side() {
    let data = encode(&gradient(100, 60), ImageFormat::Png);
    let p = process_image(&data, &default_variants()).unwrap();
    let t = variant(&p, "thumbnail");
    assert_eq!((t.width, t.height), (60, 60));
}

#[test]
fn orientation_applied() {
    let jpeg = encode(&gradient(200, 100), ImageFormat::Jpeg);
    let data = with_exif_orientation(&jpeg, 6);
    let p = process_image(&data, &default_variants()).unwrap();
    assert_eq!((p.width, p.height), (100, 200));
}

#[test]
fn metadata_stripped() {
    let jpeg = encode(&gradient(200, 100), ImageFormat::Jpeg);
    let data = with_exif_orientation(&jpeg, 1);
    assert!(data.windows(6).any(|w| w == b"Exif\0\0"));
    let p = process_image(&data, &default_variants()).unwrap();
    for v in &p.variants {
        assert!(!v.data.windows(4).any(|w| w == b"Exif"), "{}", v.name);
    }
}

#[test]
fn decompression_bomb_rejected() {
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(b"IHDR");
    ihdr.extend_from_slice(&20_000u32.to_be_bytes());
    ihdr.extend_from_slice(&20_000u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    data.extend_from_slice(&13u32.to_be_bytes());
    data.extend_from_slice(&ihdr);
    data.extend_from_slice(&crc32(&ihdr).to_be_bytes());
    let idat = [b'I', b'D', b'A', b'T', 0x78, 0x9c, 0, 0];
    data.extend_from_slice(&4u32.to_be_bytes());
    data.extend_from_slice(&idat);
    data.extend_from_slice(&crc32(&idat).to_be_bytes());
    let start = Instant::now();
    let err = process_image(&data, &default_variants()).unwrap_err();
    assert!(start.elapsed().as_secs_f32() < 1.0);
    assert!(
        matches!(err, MediaError::ImageTooLarge { pixels } if pixels == 400_000_000),
        "{err:?}"
    );
}

#[test]
fn non_image_rejected() {
    let html = b"<!doctype html><html><body>photo.png</body></html>";
    let err = process_image(html, &default_variants()).unwrap_err();
    assert!(matches!(err, MediaError::UnsupportedImage(_)), "{err:?}");
}

#[test]
fn gif_first_frame_to_png() {
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        let f1 = RgbaImage::from_pixel(200, 200, Rgba([255, 0, 0, 255]));
        let f2 = RgbaImage::from_pixel(200, 200, Rgba([0, 0, 255, 255]));
        enc.encode_frames([f1, f2].into_iter().map(image::Frame::new))
            .unwrap();
    }
    let p = process_image(&out, &default_variants()).unwrap();
    let t = variant(&p, "thumbnail");
    assert_eq!(t.mime, "image/png");
    let px = image::load_from_memory(&t.data).unwrap().to_rgba8();
    assert_eq!(px.get_pixel(10, 10).0[0], 255);
    assert_eq!(px.get_pixel(10, 10).0[2], 0);
}

fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut ihdr = b"IHDR".to_vec();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    data.extend_from_slice(&13u32.to_be_bytes());
    data.extend_from_slice(&ihdr);
    data.extend_from_slice(&crc32(&ihdr).to_be_bytes());
    let idat = [b'I', b'D', b'A', b'T', 0x78, 0x9c, 0, 0];
    data.extend_from_slice(&4u32.to_be_bytes());
    data.extend_from_slice(&idat);
    data.extend_from_slice(&crc32(&idat).to_be_bytes());
    data
}

#[test]
fn side_limit_maps_to_image_too_large() {
    let err = process_image(&png_header(60_000, 2_000), &default_variants()).unwrap_err();
    assert!(matches!(err, MediaError::ImageTooLarge { .. }), "{err:?}");
}

#[test]
fn gif_bomb_rejected() {
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&65_535u16.to_le_bytes());
    gif.extend_from_slice(&65_535u16.to_le_bytes());
    gif.extend_from_slice(&[0, 0, 0]);
    gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 0x01, 0, 0x3B]);
    let start = Instant::now();
    let err = process_image(&gif, &default_variants()).unwrap_err();
    assert!(start.elapsed().as_secs_f32() < 1.0);
    assert!(matches!(err, MediaError::ImageTooLarge { .. }), "{err:?}");
}

#[test]
fn icc_profile_kept() {
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageDecoder, ImageEncoder};
    let profile = b"fake-icc-profile-bytes".to_vec();
    let img = gradient(300, 300);
    let mut data = Vec::new();
    let mut enc = PngEncoder::new(&mut data);
    enc.set_icc_profile(profile.clone()).unwrap();
    enc.write_image(img.as_raw(), 300, 300, ExtendedColorType::Rgb8)
        .unwrap();
    let p = process_image(&data, &default_variants()).unwrap();
    let t = variant(&p, "thumbnail");
    let mut dec = image::ImageReader::new(Cursor::new(&t.data[..]))
        .with_guessed_format()
        .unwrap()
        .into_decoder()
        .unwrap();
    assert_eq!(dec.icc_profile().unwrap(), Some(profile));
}
