//! Safe image decoding and resized variants (bytes in, variants out).
//!
//! CPU-bound: callers run it inside `spawn_blocking`.

use std::io::Cursor;

use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use crate::error::MediaError;
use crate::sniff::{is_raster_image, sniff_mime};

/// Maximum decoded pixel count (100 MP).
pub const MAX_PIXELS: u64 = 100_000_000;
const MAX_DIMENSION: u32 = 50_000;
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
const JPEG_QUALITY: u8 = 82;

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

fn unsupported(err: impl std::fmt::Display) -> MediaError {
    MediaError::UnsupportedImage(err.to_string())
}

/// Decodes `data` (sniffed, never trusting names), applies EXIF orientation and
/// produces the requested variants. Output metadata is dropped by re-encoding.
///
/// Variants are encoded in the source format: JPEG (quality 82) or PNG; GIF
/// (first frame) and WebP sources become PNG.
///
/// A width-bounded spec keeps the aspect ratio and is skipped when the source
/// is not wider than the target. A `crop` spec is cover-resized and center-cropped
/// to `width` x `height` (height defaults to width); if the source is smaller than
/// the target in a dimension, nothing is upscaled and the result is the
/// center-cropped square of `min(w, h)` (capped at the target size).
pub fn process_image(data: &[u8], specs: &[VariantSpec]) -> Result<Processed, MediaError> {
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
    let mut decoder = reader.into_decoder().map_err(unsupported)?;

    let (w, h) = decoder.dimensions();
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_PIXELS {
        return Err(MediaError::ImageTooLarge { pixels });
    }
    let orientation = decoder.orientation().map_err(unsupported)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(unsupported)?;
    img.apply_orientation(orientation);

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
            data: encode(&resized, format)?,
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

fn center_crop(img: &DynamicImage, cw: u32, ch: u32) -> DynamicImage {
    let x = (img.width() - cw) / 2;
    let y = (img.height() - ch) / 2;
    img.crop_imm(x, y, cw, ch)
}

fn encode(img: &DynamicImage, format: ImageFormat) -> Result<Bytes, MediaError> {
    let mut out = Vec::new();
    match format {
        ImageFormat::Jpeg => {
            let rgb = img.to_rgb8();
            JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
                .encode_image(&rgb)
                .map_err(unsupported)?;
        }
        other => {
            img.write_to(&mut Cursor::new(&mut out), other)
                .map_err(unsupported)?;
        }
    }
    Ok(Bytes::from(out))
}
