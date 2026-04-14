// Author: Darshankumar Joshi
//
// Image transform pipeline for the `/api/storage/*path?transform=...` endpoint.
//
// Parses a comma-separated transform string (e.g. `resize:800x600,crop:0,0,400,400,
// format:webp,quality:80`) into an ordered list of operations, applies them over
// the decoded image using the `image` crate, encodes the result in the requested
// format, and exposes the matching output MIME type.
//
// A small process-local LRU-ish byte cache is provided for callers that do not
// yet have access to a shared bytes-oriented cache in `AppState`. Keys are
// expected to be caller-computed (e.g. `img:<path>:<sha256(transform)>`) so that
// path + transform combinations do not collide across requests.

use std::io::Cursor;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use image::{DynamicImage, ImageEncoder, ImageError, imageops::FilterType};

use super::MAX_IMAGE_DIMENSION;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced while parsing, applying, or encoding an image transform.
#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    /// The transform string was malformed (unknown op, bad token, bad value).
    #[error("invalid transform: {0}")]
    Parse(String),
    /// The `image` crate failed to decode or encode the image.
    #[error("image processing failed: {0}")]
    Image(#[from] ImageError),
    /// A crop region fell outside the source image bounds.
    #[error("crop region out of bounds")]
    CropOutOfBounds,
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

/// A single image transform operation, in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Resize to an exact width/height (aspect ratio not preserved).
    Resize { w: u32, h: u32 },
    /// Crop a rectangle starting at `(x, y)` with dimensions `w`×`h`.
    Crop { x: u32, y: u32, w: u32, h: u32 },
    /// Force the output format (`webp`, `avif`, `jpeg`/`jpg`, `png`).
    Format(String),
    /// Quality hint for lossy encoders (1-100).
    Quality(u8),
}

// ---------------------------------------------------------------------------
// TransformOps
// ---------------------------------------------------------------------------

/// Ordered list of image operations parsed from a transform query string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransformOps {
    pub ops: Vec<Op>,
}

impl TransformOps {
    /// Parse a transform string like `resize:800x600,crop:0,0,400,400,format:webp,quality:80`.
    ///
    /// Tokens are comma-separated. Each token starts with `<op>:` followed by
    /// op-specific arguments. Crop is the only op that itself contains commas
    /// (four `u32`s), so the parser performs a small two-pass tokenization: it
    /// walks the raw string, peeling off crop tokens greedily and splitting the
    /// remainder on commas.
    pub fn parse(s: &str) -> Result<Self, TransformError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(TransformError::Parse("empty transform string".into()));
        }

        let mut ops: Vec<Op> = Vec::new();
        let mut i = 0usize;
        let bytes = trimmed.as_bytes();

        while i < bytes.len() {
            // Skip leading commas / whitespace.
            while i < bytes.len() && (bytes[i] == b',' || bytes[i] == b' ') {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }

            // Find the next colon: that terminates the op key.
            let key_end = match trimmed[i..].find(':') {
                Some(rel) => i + rel,
                None => {
                    return Err(TransformError::Parse(format!(
                        "missing ':' in token starting at {}",
                        &trimmed[i..]
                    )));
                }
            };
            let key = trimmed[i..key_end].trim().to_ascii_lowercase();
            let val_start = key_end + 1;

            // For crop, consume the next four comma-separated numbers; for all
            // other ops, consume up to the next comma.
            let (val, next) = if key == "crop" {
                let mut end = val_start;
                let mut commas = 0usize;
                while end < bytes.len() {
                    if bytes[end] == b',' {
                        if commas == 3 {
                            break;
                        }
                        commas += 1;
                    }
                    end += 1;
                }
                (&trimmed[val_start..end], end)
            } else {
                let end = trimmed[val_start..]
                    .find(',')
                    .map(|rel| val_start + rel)
                    .unwrap_or(bytes.len());
                (&trimmed[val_start..end], end)
            };

            ops.push(parse_op(&key, val.trim())?);
            i = next;
        }

        if ops.is_empty() {
            return Err(TransformError::Parse("no operations parsed".into()));
        }

        Ok(Self { ops })
    }

    /// Apply every op in order over the decoded `DynamicImage`.
    pub fn apply(&self, mut img: DynamicImage) -> Result<DynamicImage, TransformError> {
        for op in &self.ops {
            match op {
                Op::Resize { w, h } => {
                    img = img.resize_exact(*w, *h, FilterType::Lanczos3);
                }
                Op::Crop { x, y, w, h } => {
                    let (iw, ih) = (img.width(), img.height());
                    if x.saturating_add(*w) > iw || y.saturating_add(*h) > ih {
                        return Err(TransformError::CropOutOfBounds);
                    }
                    img = img.crop_imm(*x, *y, *w, *h);
                }
                // Format + quality are encoder-side; no-op here.
                Op::Format(_) | Op::Quality(_) => {}
            }
        }
        Ok(img)
    }

    /// Encode the image using the last `Format` op (defaulting to PNG) and the
    /// last `Quality` op (where the encoder honors it).
    pub fn encode(&self, img: DynamicImage) -> Result<Vec<u8>, TransformError> {
        let fmt = self.last_format().unwrap_or(OutputFormat::Png);
        let quality = self.last_quality().unwrap_or(80);

        let mut buf: Vec<u8> = Vec::new();
        match fmt {
            OutputFormat::Jpeg => {
                let rgb = img.to_rgb8();
                let mut cursor = Cursor::new(&mut buf);
                let encoder =
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
                encoder.write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )?;
            }
            OutputFormat::Png => {
                let mut cursor = Cursor::new(&mut buf);
                img.write_to(&mut cursor, image::ImageFormat::Png)?;
            }
            OutputFormat::Webp => {
                // `image` 0.25 exposes a lossless WebP encoder; `write_to` picks it
                // based on the output format, so quality is ignored (lossless).
                let mut cursor = Cursor::new(&mut buf);
                img.write_to(&mut cursor, image::ImageFormat::WebP)?;
            }
            OutputFormat::Avif => {
                // AVIF encoding is feature-gated in `image`; expose via write_to.
                let mut cursor = Cursor::new(&mut buf);
                img.write_to(&mut cursor, image::ImageFormat::Avif)?;
            }
        }
        Ok(buf)
    }

    /// MIME type matching the encoder selected by [`Self::encode`].
    pub fn output_mime(&self) -> &'static str {
        match self.last_format().unwrap_or(OutputFormat::Png) {
            OutputFormat::Jpeg => "image/jpeg",
            OutputFormat::Png => "image/png",
            OutputFormat::Webp => "image/webp",
            OutputFormat::Avif => "image/avif",
        }
    }

    fn last_format(&self) -> Option<OutputFormat> {
        self.ops.iter().rev().find_map(|op| match op {
            Op::Format(f) => OutputFormat::parse(f),
            _ => None,
        })
    }

    fn last_quality(&self) -> Option<u8> {
        self.ops.iter().rev().find_map(|op| match op {
            Op::Quality(q) => Some(*q),
            _ => None,
        })
    }
}

// ---------------------------------------------------------------------------
// Output format enum (internal)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Jpeg,
    Png,
    Webp,
    Avif,
}

impl OutputFormat {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "webp" => Some(Self::Webp),
            "avif" => Some(Self::Avif),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Token parsing
// ---------------------------------------------------------------------------

fn parse_op(key: &str, val: &str) -> Result<Op, TransformError> {
    match key {
        "resize" => {
            let mut parts = val.split(['x', 'X']);
            let w = parts
                .next()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .ok_or_else(|| TransformError::Parse(format!("resize: bad width in '{val}'")))?;
            let h = parts
                .next()
                .and_then(|v| v.trim().parse::<u32>().ok())
                .ok_or_else(|| TransformError::Parse(format!("resize: bad height in '{val}'")))?;
            if w == 0 || h == 0 {
                return Err(TransformError::Parse(
                    "resize: width/height must be > 0".into(),
                ));
            }
            if w > MAX_IMAGE_DIMENSION || h > MAX_IMAGE_DIMENSION {
                return Err(TransformError::Parse(format!(
                    "resize: dimension exceeds {MAX_IMAGE_DIMENSION}"
                )));
            }
            Ok(Op::Resize { w, h })
        }
        "crop" => {
            let nums: Vec<u32> = val
                .split(',')
                .map(str::trim)
                .map(|p| p.parse::<u32>())
                .collect::<Result<_, _>>()
                .map_err(|e| TransformError::Parse(format!("crop: {e}")))?;
            if nums.len() != 4 {
                return Err(TransformError::Parse(format!(
                    "crop: expected 4 numbers x,y,w,h (got {})",
                    nums.len()
                )));
            }
            let (x, y, w, h) = (nums[0], nums[1], nums[2], nums[3]);
            if w == 0 || h == 0 {
                return Err(TransformError::Parse(
                    "crop: width/height must be > 0".into(),
                ));
            }
            if w > MAX_IMAGE_DIMENSION || h > MAX_IMAGE_DIMENSION {
                return Err(TransformError::Parse(format!(
                    "crop: dimension exceeds {MAX_IMAGE_DIMENSION}"
                )));
            }
            Ok(Op::Crop { x, y, w, h })
        }
        "format" | "f" => {
            if OutputFormat::parse(val).is_none() {
                return Err(TransformError::Parse(format!(
                    "format: unknown output format '{val}'"
                )));
            }
            Ok(Op::Format(val.trim().to_ascii_lowercase()))
        }
        "quality" | "q" => {
            let q = val
                .parse::<u8>()
                .map_err(|_| TransformError::Parse(format!("quality: bad value '{val}'")))?;
            if !(1..=100).contains(&q) {
                return Err(TransformError::Parse(
                    "quality: must be in 1..=100".into(),
                ));
            }
            Ok(Op::Quality(q))
        }
        other => Err(TransformError::Parse(format!("unknown op: '{other}'"))),
    }
}

// ---------------------------------------------------------------------------
// Process-local byte cache
// ---------------------------------------------------------------------------

struct BytesEntry {
    bytes: Vec<u8>,
    mime: &'static str,
    inserted_at: Instant,
    ttl: Duration,
}

/// Minimal, process-local, TTL-aware byte cache used as an L1 for transformed
/// images while the shared bytes cache is still being merged. Uses `DashMap`
/// for lock-free concurrent reads and trims expired entries lazily on access.
static TRANSFORM_BYTE_CACHE: OnceLock<DashMap<String, BytesEntry>> = OnceLock::new();

fn cache() -> &'static DashMap<String, BytesEntry> {
    TRANSFORM_BYTE_CACHE.get_or_init(DashMap::new)
}

/// Look up a cached transform result.
pub fn cache_get(key: &str) -> Option<(Vec<u8>, &'static str)> {
    let c = cache();
    if let Some(entry) = c.get(key) {
        if entry.inserted_at.elapsed() < entry.ttl {
            return Some((entry.bytes.clone(), entry.mime));
        }
    }
    // Lazy expiry cleanup.
    c.remove(key);
    None
}

/// Insert a transformed image into the cache.
pub fn cache_set(key: String, bytes: Vec<u8>, mime: &'static str, ttl: Duration) {
    cache().insert(
        key,
        BytesEntry {
            bytes,
            mime,
            inserted_at: Instant::now(),
            ttl,
        },
    );
}

/// Drain the cache (used in tests).
#[cfg(test)]
pub fn cache_clear() {
    cache().clear();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn sample_image(w: u32, h: u32) -> DynamicImage {
        let buf: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
            Rgba([(x % 255) as u8, (y % 255) as u8, 128, 255])
        });
        DynamicImage::ImageRgba8(buf)
    }

    #[test]
    fn parses_resize() {
        let ops = TransformOps::parse("resize:800x600").expect("parse");
        assert_eq!(ops.ops, vec![Op::Resize { w: 800, h: 600 }]);
    }

    #[test]
    fn parses_full_pipeline() {
        let ops = TransformOps::parse("resize:400x400,crop:0,0,200,200,format:webp,quality:80")
            .expect("parse");
        assert_eq!(ops.ops.len(), 4);
        assert_eq!(ops.ops[0], Op::Resize { w: 400, h: 400 });
        assert_eq!(
            ops.ops[1],
            Op::Crop {
                x: 0,
                y: 0,
                w: 200,
                h: 200
            }
        );
        assert_eq!(ops.ops[2], Op::Format("webp".into()));
        assert_eq!(ops.ops[3], Op::Quality(80));
        assert_eq!(ops.output_mime(), "image/webp");
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(TransformOps::parse("").is_err());
        assert!(TransformOps::parse("   ").is_err());
        assert!(TransformOps::parse("resize:notxnum").is_err());
        assert!(TransformOps::parse("resize:800").is_err());
        assert!(TransformOps::parse("crop:1,2,3").is_err());
        assert!(TransformOps::parse("format:tiff").is_err());
        assert!(TransformOps::parse("quality:200").is_err());
        assert!(TransformOps::parse("unknown:foo").is_err());
        assert!(TransformOps::parse("resize").is_err());
    }

    #[test]
    fn applies_resize_100x100() {
        let img = sample_image(400, 300);
        let ops = TransformOps::parse("resize:100x100").unwrap();
        let out = ops.apply(img).unwrap();
        assert_eq!(out.width(), 100);
        assert_eq!(out.height(), 100);
    }

    #[test]
    fn crop_out_of_bounds_errors() {
        let img = sample_image(100, 100);
        let ops = TransformOps::parse("crop:50,50,200,200").unwrap();
        assert!(matches!(
            ops.apply(img),
            Err(TransformError::CropOutOfBounds)
        ));
    }

    #[test]
    fn format_conversion_jpeg_to_webp() {
        // Encode a JPEG source first so we know the pipeline really converts.
        let img = sample_image(64, 64);
        let mut jpeg_bytes: Vec<u8> = Vec::new();
        {
            let mut cursor = Cursor::new(&mut jpeg_bytes);
            let rgb = img.to_rgb8();
            let encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 85);
            encoder
                .write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .unwrap();
        }
        let decoded = image::load_from_memory(&jpeg_bytes).unwrap();
        let ops = TransformOps::parse("format:webp").unwrap();
        let transformed = ops.apply(decoded).unwrap();
        let out = ops.encode(transformed).unwrap();
        assert!(!out.is_empty(), "webp output should have bytes");
        // WebP magic: "RIFF" .... "WEBP"
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(&out[8..12], b"WEBP");
        assert_eq!(ops.output_mime(), "image/webp");
    }

    #[test]
    fn quality_50_jpeg_smaller_than_quality_95() {
        let img = sample_image(256, 256);
        let low = TransformOps::parse("format:jpeg,quality:50").unwrap();
        let high = TransformOps::parse("format:jpeg,quality:95").unwrap();
        let low_bytes = low.encode(low.apply(img.clone()).unwrap()).unwrap();
        let high_bytes = high.encode(high.apply(img).unwrap()).unwrap();
        assert!(
            low_bytes.len() < high_bytes.len(),
            "q=50 ({}) should be smaller than q=95 ({})",
            low_bytes.len(),
            high_bytes.len()
        );
        assert_eq!(low.output_mime(), "image/jpeg");
    }

    #[test]
    fn png_encode_roundtrip() {
        let img = sample_image(32, 32);
        let ops = TransformOps::parse("format:png").unwrap();
        let out = ops.encode(ops.apply(img).unwrap()).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!(decoded.width(), 32);
        assert_eq!(decoded.height(), 32);
    }

    #[test]
    fn byte_cache_set_and_get() {
        cache_clear();
        let key = "img:foo:bar".to_string();
        cache_set(key.clone(), vec![1, 2, 3], "image/png", Duration::from_secs(60));
        let hit = cache_get(&key).expect("hit");
        assert_eq!(hit.0, vec![1, 2, 3]);
        assert_eq!(hit.1, "image/png");
        assert!(cache_get("img:missing:x").is_none());
    }

    #[test]
    fn byte_cache_expires() {
        cache_clear();
        let key = "img:ttl:x".to_string();
        cache_set(key.clone(), vec![9], "image/png", Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache_get(&key).is_none());
    }
}
