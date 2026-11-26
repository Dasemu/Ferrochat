//! Photos in chat bubbles: display-sized previews decoded off the UI thread,
//! their disk cache, and the blurred placeholder made from the inline
//! `jpegThumbnail` while the picture loads.
//!
//! iced decodes a `Handle::from_bytes` picture while drawing, on the UI
//! thread, at full resolution (a 12 Mpx photo is 48 MB of pixels), and again
//! whenever it was off screen for a frame (tiny-skia's raster cache keeps
//! only what the last frame drew). A bubble is at most 330 px wide, so the
//! app decodes once in a worker, scales to [`PREVIEW_SIDE`] and hands iced
//! ready RGBA.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use image::{DynamicImage, ImageDecoder};

use crate::error::{Error, Result};

/// Longest side of a bubble preview: the 330 px bubble at scale factor 2.
pub const PREVIEW_SIDE: u32 = 660;
/// Longest side of the blurred placeholder (it is drawn scaled up; being
/// blurred it stays smooth, and it is small enough to keep many in memory).
pub const PLACEHOLDER_SIDE: u32 = 64;
/// Previews kept on disk.
pub const DISK_FILES: usize = 3000;
/// Bytes of previews kept on disk.
pub const DISK_BYTES: u64 = 200 * 1024 * 1024;
/// Largest preview file read back (a 660 px JPEG is ~100 kB).
const MAX_PREVIEW_FILE: u64 = 4 * 1024 * 1024;
/// Inline thumbnails are ~100 px; anything bigger is not one.
const MAX_THUMB_PIXELS: u64 = 1_000_000;

fn media_err(e: impl std::fmt::Display) -> Error {
    Error::Media(e.to_string())
}

/// Decoded RGBA pixels, ready for `image::Handle::from_rgba`.
#[derive(Clone, PartialEq, Eq)]
pub struct Preview {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Preview({}x{})", self.width, self.height)
    }
}

impl Preview {
    fn from_image(img: DynamicImage) -> Self {
        let rgba = img.into_rgba8();
        Self {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
        }
    }

    /// Memory the pixels take.
    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

/// Decodes with a pixel cap and allocation limits, applying the EXIF
/// orientation like iced's own loader does.
pub fn decode_oriented(bytes: &[u8], max_pixels: u64) -> Result<DynamicImage> {
    super::image::check_image_bounds_max(bytes, max_pixels)?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(media_err)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(max_pixels * 4 + 16 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(media_err)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(media_err)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// The bubble preview of a full picture: decoded, oriented and scaled to
/// fit [`PREVIEW_SIDE`] (never scaled up). Blocking; call it off the UI
/// thread.
pub fn preview_of(bytes: &[u8], max_pixels: u64) -> Result<Preview> {
    let img = decode_oriented(bytes, max_pixels)?;
    let img = if img.width().max(img.height()) > PREVIEW_SIDE {
        // Area averaging: fast and alias-free for big reductions.
        img.thumbnail(PREVIEW_SIDE, PREVIEW_SIDE)
    } else {
        img
    };
    Ok(Preview::from_image(img))
}

/// The placeholder shown while the picture loads: the inline thumbnail,
/// reduced to [`PLACEHOLDER_SIDE`] and blurred, so the bubble shows a soft
/// image (like WhatsApp) instead of upscaled JPEG blocks.
pub fn blurred_placeholder(jpeg: &[u8]) -> Option<Preview> {
    let img = decode_oriented(jpeg, MAX_THUMB_PIXELS).ok()?;
    let img = if img.width().max(img.height()) > PLACEHOLDER_SIDE {
        img.thumbnail(PLACEHOLDER_SIDE, PLACEHOLDER_SIDE)
    } else {
        img
    };
    let blurred = image::imageops::blur(&img.into_rgba8(), 1.4);
    Some(Preview::from_image(DynamicImage::ImageRgba8(blurred)))
}

/// Previews on disk (`$XDG_CACHE_HOME/ferrochat/previews/`), keyed by the
/// media cache key (hash of the plaintext), so a photo is downloaded once
/// and shows at once after a restart. Every method blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewCache {
    dir: PathBuf,
}

/// Writes since the last prune (pruning reads the whole directory).
static WRITES: AtomicU32 = AtomicU32::new(0);

impl PreviewCache {
    /// A cache in `dir` (created on the first write).
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir
            .join(format!("{}.img", super::avatar::name_hash(key)))
    }

    /// The preview stored for `key`, if any (and marks it recently used).
    pub fn load(&self, key: &str) -> Option<Preview> {
        let path = self.path(key);
        let file = std::fs::File::open(&path).ok()?;
        if file.metadata().ok()?.len() > MAX_PREVIEW_FILE {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let side = u64::from(PREVIEW_SIDE);
        match decode_oriented(&bytes, side * side) {
            Ok(img) => {
                // Pruning drops the oldest files first.
                let _ = file.set_modified(std::time::SystemTime::now());
                Some(Preview::from_image(img))
            }
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    /// Stores `p` for `key` (JPEG, or PNG when it has transparency).
    pub fn store(&self, key: &str, p: &Preview) -> Result<()> {
        let path = self.path(key);
        if path.is_file() {
            return Ok(());
        }
        let encoded = encode(p)?;
        crate::config::secure_dir(&self.dir).map_err(media_err)?;
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = self.dir.join(format!(".tmp-{}-{n}", std::process::id()));
        std::fs::write(&tmp, &encoded).map_err(media_err)?;
        std::fs::rename(&tmp, &path).map_err(media_err)?;
        if WRITES.fetch_add(1, Ordering::Relaxed).is_multiple_of(32) {
            super::avatar::prune(&self.dir, DISK_FILES, DISK_BYTES);
        }
        Ok(())
    }

    /// The directory (removed on logout).
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn encode(p: &Preview) -> Result<Vec<u8>> {
    let img = image::RgbaImage::from_raw(p.width, p.height, p.rgba.clone())
        .ok_or_else(|| Error::Media("preview size does not match its pixels".into()))?;
    let opaque = p.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 255);
    let mut out = Vec::new();
    if opaque {
        let rgb = DynamicImage::ImageRgba8(img).into_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 88)
            .encode_image(&rgb)
            .map_err(media_err)?;
    } else {
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .map_err(media_err)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 90])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 85)
            .encode_image(&img)
            .unwrap();
        out
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ferrochat-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn previews_fit_the_bubble_and_keep_the_aspect() {
        let p = preview_of(&jpeg(2000, 1000), 40_000_000).unwrap();
        assert_eq!((p.width, p.height), (PREVIEW_SIDE, PREVIEW_SIDE / 2));
        assert_eq!(p.bytes(), (PREVIEW_SIDE * PREVIEW_SIDE / 2 * 4) as usize);
        // Small pictures are not scaled up.
        let p = preview_of(&jpeg(300, 200), 40_000_000).unwrap();
        assert_eq!((p.width, p.height), (300, 200));
        // Over the pixel cap: refused before decoding.
        assert!(preview_of(&jpeg(2000, 1000), 1_000_000).is_err());
        assert!(preview_of(b"not an image", 40_000_000).is_err());
    }

    #[test]
    fn the_placeholder_is_small_and_blurred() {
        // A sharp checkerboard thumbnail: blurring must soften the edges.
        let img = image::RgbImage::from_fn(96, 64, |x, y| {
            if (x / 8 + y / 8) % 2 == 0 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        let mut thumb = Vec::new();
        image::codecs::png::PngEncoder::new(&mut thumb)
            .write_image(img.as_raw(), 96, 64, image::ExtendedColorType::Rgb8)
            .unwrap();
        let p = blurred_placeholder(&thumb).unwrap();
        assert_eq!(p.width.max(p.height), PLACEHOLDER_SIDE);
        let extremes = p
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] < 10 || px[0] > 245)
            .count();
        assert!(
            extremes < p.rgba.len() / 4 / 4,
            "{extremes} hard pixels left"
        );
        assert!(blurred_placeholder(b"junk").is_none());
    }

    use image::ImageEncoder;

    #[test]
    fn the_disk_cache_round_trips_and_survives_junk() {
        let dir = temp_dir("previews");
        let cache = PreviewCache::new(dir.clone());
        assert!(cache.load("k").is_none());
        let p = preview_of(&jpeg(800, 600), 40_000_000).unwrap();
        cache.store("k", &p).unwrap();
        let back = cache.load("k").expect("stored");
        assert_eq!((back.width, back.height), (p.width, p.height));
        // A transparent preview keeps its alpha (PNG).
        let clear = Preview {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 0, 4, 5, 6, 255],
        };
        cache.store("alpha", &clear).unwrap();
        assert_eq!(cache.load("alpha"), Some(clear));
        // A corrupt file is a miss and is removed.
        std::fs::write(cache.path("bad"), b"junk").unwrap();
        assert!(cache.load("bad").is_none());
        assert!(!cache.path("bad").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What a full-size photo cost when iced decoded it while drawing,
    /// versus the preview handed to it now. `cargo test --release
    /// decode_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing; prints numbers"]
    fn decode_cost() {
        let big = jpeg(4000, 3000);
        let t = std::time::Instant::now();
        let full = image::load_from_memory(&big).unwrap().into_rgba8();
        let draw = t.elapsed();
        let t = std::time::Instant::now();
        let p = preview_of(&big, 40_000_000).unwrap();
        let worker = t.elapsed();
        println!(
            "12 Mpx JPEG: iced-style full decode {draw:?} ({} MB RGBA, on the UI thread, again after every scroll-out); preview {worker:?} in a worker ({} kB RGBA)",
            full.len() >> 20,
            p.bytes() >> 10
        );
    }
}
