//! Safe image decoding and resized variants (bytes in, variants out).
//!
//! CPU-bound: callers run it inside `spawn_blocking`.

use std::io::Cursor;

use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError, ImageFormat,
    ImageReader, Limits,
};

use crate::error::MediaError;
use crate::sniff::{is_raster_image, sniff_mime};

/// Maximum decoded pixel count (100 MP).
pub const MAX_PIXELS: u64 = 100_000_000;
const MAX_DIMENSION: u32 = 50_000;
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
const JPEG_QUALITY: u8 = 82;
/// Source ICC profiles larger than this are dropped (real profiles are a few KiB to ~1 MiB).
const MAX_ICC_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantSpec {
    pub name: String,
    pub width: u32,
    pub height: Option<u32>,
    pub crop: bool,
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub data: Bytes,
    pub mime: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct Processed {
    /// Dimensions after EXIF orientation is applied.
    pub width: u32,
    pub height: u32,
    pub variants: Vec<Variant>,
}

pub fn default_variants() -> Vec<VariantSpec> {
    vec![
        VariantSpec {
            name: "thumbnail".to_owned(),
            width: 150,
            height: Some(150),
            crop: true,
        },
        VariantSpec {
            name: "medium".to_owned(),
            width: 768,
            height: None,
            crop: false,
        },
        VariantSpec {
            name: "large".to_owned(),
            width: 1600,
            height: None,
            crop: false,
        },
    ]
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

fn map_image_error(err: ImageError, dims: Option<(u32, u32)>) -> MediaError {
    match err {
        ImageError::Limits(_) => MediaError::ImageTooLarge {
            pixels: dims.map_or(0, |(w, h)| u64::from(w) * u64::from(h)),
        },
        other => MediaError::UnsupportedImage(other.to_string()),
    }
}

fn validate(specs: &[VariantSpec]) -> Result<(), MediaError> {
    let mut seen = std::collections::HashSet::new();
    for spec in specs {
        if spec.name.is_empty() {
            return Err(MediaError::Config("variant with an empty name".into()));
        }
        if !seen.insert(spec.name.as_str()) {
            return Err(MediaError::Config(format!(
                "duplicate variant name '{}'",
                spec.name
            )));
        }
        if spec.width == 0 || spec.height == Some(0) {
            return Err(MediaError::Config(format!(
                "variant '{}' has a zero dimension",
                spec.name
            )));
        }
        if spec.height.is_some() && !spec.crop {
            return Err(MediaError::Config(format!(
                "variant '{}': a height without crop would be a bounding box, which is not implemented",
                spec.name
            )));
        }
    }
    Ok(())
}

/// Decodes `data` (sniffed, never trusting names), applies EXIF orientation and
/// produces the requested variants. Output metadata is dropped by re-encoding.
///
/// Variants are encoded in the source format: JPEG (quality 82) or PNG; GIF
/// (first frame) and WebP sources become PNG.
///
/// The ICC profile of the source is kept (colour fidelity) unless it is larger than 1 MiB;
/// EXIF/XMP are not.
///
/// Memory (upper bounds, per call): the decoded buffer is at most 512 MiB (w × h ×
/// bytes per pixel, checked from the header before decoding, else
/// [`MediaError::ImageTooLarge`]). An EXIF rotation copies it (≤ 512 MiB more); the
/// JPEG RGB conversion copies it as RGB8 (≤ 300 MB at 100 MP). Each resize holds an
/// `f32` RGBA intermediate of source width × target height × 16 bytes (never more than
/// 16 bytes per source pixel) plus its output (never larger than the decoded buffer,
/// since nothing is upscaled). With the core sizes the intermediate is at most
/// 50 000 × 1 600 × 16 B ≈ 1.3 GB (a tall image and `large`), so one call stays under
/// about 0.5 + 1.3 + 0.5 ≈ 2.4 GB; a theme size wider than 1 600 can raise the
/// intermediate to 16 B × 100 MP = 1.6 GB (≈ 2.7 GB). Decoder-internal buffers are capped
/// separately at 512 MiB (`max_alloc`). Callers must bound concurrency (the
/// `media.variants` handler's `media.image_workers` slots).
///
/// Specs are validated first: names must be non-empty and unique, and a `height` requires
/// `crop` (a bounding box is not implemented).
///
/// A width-bounded spec keeps the aspect ratio and is skipped when the source
/// is not wider than the target. A `crop` spec is cover-resized and center-cropped
/// to `width` x `height` (height defaults to width); if the source is smaller than
/// the target in a dimension, nothing is upscaled and the result is the
/// center-cropped square of `min(w, h)` (capped at the target size).
pub fn process_image(data: &[u8], specs: &[VariantSpec]) -> Result<Processed, MediaError> {
    validate(specs)?;
    let mime = sniff_mime(data);
    if !is_raster_image(&mime) {
        return Err(MediaError::UnsupportedImage(format!(
            "not a supported raster image ({mime})"
        )));
    }

    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| MediaError::Io(e.to_string()))?;
    reader.limits(limits());
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| map_image_error(e, None))?;

    let (w, h) = decoder.dimensions();
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_PIXELS {
        return Err(MediaError::ImageTooLarge { pixels });
    }
    // The decoded buffer (w × h × bytes per pixel of the decoder's colour type) is
    // allocated by `DynamicImage::from_decoder` outside `Limits::max_alloc` (image 0.25),
    // so it is bounded here, before anything is allocated (M2a-2b R12): 100 MP of 16-bit
    // RGBA would otherwise be 800 MB.
    if decoder.total_bytes() > MAX_ALLOC {
        return Err(MediaError::ImageTooLarge { pixels });
    }
    let dims = Some((w, h));
    let orientation = decoder
        .orientation()
        .map_err(|e| map_image_error(e, dims))?;
    let icc = decoder.icc_profile().unwrap_or_else(|e| {
        tracing::debug!(error = %e, "could not read ICC profile");
        None
    });
    let icc = icc.filter(|profile| {
        let keep = profile.len() <= MAX_ICC_BYTES;
        if !keep {
            tracing::debug!(
                bytes = profile.len(),
                "dropping ICC profile larger than {MAX_ICC_BYTES} bytes"
            );
        }
        keep
    });
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| map_image_error(e, dims))?;
    img.apply_orientation(orientation);
    if mime == "image/jpeg" {
        // Convert once; every variant then resizes and encodes straight from RGB8.
        img = DynamicImage::ImageRgb8(img.into_rgb8());
    }

    let (width, height) = (img.width(), img.height());
    let (format, out_mime) = if mime == "image/jpeg" {
        (ImageFormat::Jpeg, "image/jpeg")
    } else {
        (ImageFormat::Png, "image/png")
    };

    let mut variants = Vec::new();
    for spec in specs {
        let Some(resized) = make_variant(&img, spec) else {
            continue;
        };
        variants.push(Variant {
            name: spec.name.clone(),
            width: resized.width(),
            height: resized.height(),
            data: encode(&resized, format, icc.as_deref())?,
            mime: out_mime.to_owned(),
        });
    }
    Ok(Processed {
        width,
        height,
        variants,
    })
}

fn make_variant(img: &DynamicImage, spec: &VariantSpec) -> Option<DynamicImage> {
    let (w, h) = (img.width(), img.height());
    if spec.crop {
        let tw = spec.width;
        let th = spec.height.unwrap_or(spec.width);
        if w < tw || h < th {
            // Never upscale: crop the largest centered square instead.
            let side = w.min(h).min(tw.min(th));
            return Some(center_crop(img, side, side));
        }
        return Some(img.resize_to_fill(tw, th, FilterType::Lanczos3));
    }
    if w <= spec.width {
        return None;
    }
    let nh = ((u64::from(h) * u64::from(spec.width) + u64::from(w) / 2) / u64::from(w)).max(1);
    let nh = u32::try_from(nh).ok()?;
    Some(img.resize_exact(spec.width, nh, FilterType::Lanczos3))
}

fn unsupported(err: impl std::fmt::Display) -> MediaError {
    MediaError::UnsupportedImage(err.to_string())
}

fn center_crop(img: &DynamicImage, cw: u32, ch: u32) -> DynamicImage {
    let x = (img.width() - cw) / 2;
    let y = (img.height() - ch) / 2;
    img.crop_imm(x, y, cw, ch)
}

/// True when the ICC header colour-space signature (bytes 16..20) matches the
/// output pixels: `GRAY` for luma outputs, `RGB ` otherwise.
fn icc_matches(profile: &[u8], gray: bool) -> bool {
    let want: &[u8; 4] = if gray { b"GRAY" } else { b"RGB " };
    profile.get(16..20) == Some(want.as_slice())
}

fn attach_icc(encoder: &mut impl ImageEncoder, icc: Option<&[u8]>, gray: bool) {
    let Some(profile) = icc else { return };
    if !icc_matches(profile, gray) {
        tracing::debug!("dropping ICC profile whose colour space does not match the output");
        return;
    }
    if let Err(e) = encoder.set_icc_profile(profile.to_vec()) {
        tracing::debug!(error = %e, "encoder rejected ICC profile; continuing without it");
    }
}

fn encode(
    img: &DynamicImage,
    format: ImageFormat,
    icc: Option<&[u8]>,
) -> Result<Bytes, MediaError> {
    let mut out = Vec::new();
    let (w, h) = (img.width(), img.height());
    match format {
        ImageFormat::Jpeg => {
            let mut enc = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
            attach_icc(&mut enc, icc, false);
            match img.as_rgb8() {
                Some(rgb) => enc.write_image(rgb, w, h, ExtendedColorType::Rgb8),
                None => enc.write_image(&img.to_rgb8(), w, h, ExtendedColorType::Rgb8),
            }
            .map_err(unsupported)?;
        }
        _ => {
            let mut enc = PngEncoder::new(&mut out);
            let gray = matches!(
                img.color(),
                image::ColorType::L8
                    | image::ColorType::La8
                    | image::ColorType::L16
                    | image::ColorType::La16
            );
            attach_icc(&mut enc, icc, gray);
            enc.write_image(img.as_bytes(), w, h, img.color().into())
                .map_err(unsupported)?;
        }
    }
    Ok(Bytes::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(width: u32, height: Option<u32>) -> VariantSpec {
        VariantSpec {
            name: "x".to_owned(),
            width,
            height,
            crop: false,
        }
    }

    #[test]
    fn icc_guard_matches_colour_space() {
        let header = |sig: &[u8; 4]| {
            let mut h = vec![0u8; 128];
            h[16..20].copy_from_slice(sig);
            h
        };
        assert!(icc_matches(&header(b"RGB "), false));
        assert!(!icc_matches(&header(b"RGB "), true));
        assert!(icc_matches(&header(b"GRAY"), true));
        assert!(!icc_matches(&header(b"GRAY"), false));
        assert!(!icc_matches(&header(b"CMYK"), false));
        assert!(!icc_matches(&header(b"CMYK"), true));
        assert!(!icc_matches(b"short", false));
    }

    fn named(name: &str, width: u32, height: Option<u32>, crop: bool) -> VariantSpec {
        VariantSpec {
            name: name.to_owned(),
            width,
            height,
            crop,
        }
    }

    #[test]
    fn invalid_variant_sets_rejected() {
        let config_err = |specs: &[VariantSpec]| match validate(specs) {
            Err(MediaError::Config(msg)) => msg,
            other => panic!("expected Config error, got {other:?}"),
        };
        assert!(config_err(&[named("", 10, None, false)]).contains("name"));
        assert!(
            config_err(&[named("a", 10, None, false), named("a", 20, None, false)])
                .contains("duplicate")
        );
        // A height without crop would be a bounding box, which is not implemented.
        assert!(config_err(&[named("box", 10, Some(10), false)]).contains("bounding box"));
        // Valid shapes.
        assert!(validate(&[named("a", 10, None, false), named("b", 10, None, true)]).is_ok());
        assert!(validate(&[named("c", 10, Some(20), true)]).is_ok());
        assert!(validate(&[]).is_ok());
    }

    #[test]
    fn zero_dimension_specs_rejected() {
        assert!(matches!(
            validate(&[spec(0, None)]),
            Err(MediaError::Config(_))
        ));
        assert!(matches!(
            validate(&[spec(10, Some(0))]),
            Err(MediaError::Config(_))
        ));
        assert!(validate(&default_variants()).is_ok());
    }
}
