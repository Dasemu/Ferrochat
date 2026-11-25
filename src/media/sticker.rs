//! Stickers and GIFs kept for the picker: the on-disk shelf (bounded),
//! the recent / favourite lists (pure, tested) and "make a sticker from an
//! image" (512x512 WebP).
//!
//! The shelf holds the plaintext bytes of stickers (`<key>.webp`) and GIFs
//! (`<key>.mp4` plus a `<key>.jpg` poster) under
//! `$XDG_CACHE_HOME/ferrochat/{stickers,gifs}`. `key` is the hex SHA-256 of
//! the file, which is also [`crate::net::MediaRef::cache_key`] of a received
//! copy, so a sticker seen in a chat and the same one in the picker share
//! one entry. Files not listed in the recent / favourite lists are removed
//! by [`Shelf::prune`]. Nothing here touches the network or the phone's
//! app state.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::error::{Error, Result};
use crate::net::GifSource;

/// Recent stickers kept.
pub const MAX_RECENT_STICKERS: usize = 48;
/// Favourite stickers kept.
pub const MAX_FAVOURITES: usize = 200;
/// Recent GIFs kept.
pub const MAX_RECENT_GIFS: usize = 40;
/// Disk budget of the sticker shelf.
pub const STICKER_SHELF_BYTES: u64 = 64 * 1024 * 1024;
/// Disk budget of the GIF shelf.
pub const GIF_SHELF_BYTES: u64 = 200 * 1024 * 1024;
/// Side of a WhatsApp sticker.
pub const STICKER_SIDE: u32 = 512;
/// A made sticker above this is re-encoded with fewer colours.
pub const STICKER_TARGET_BYTES: usize = 300 * 1024;
/// A made sticker is refused above this.
pub const STICKER_MAX_BYTES: usize = 1024 * 1024;
/// Largest GIF (MP4) sent.
pub const MAX_GIF_BYTES: u64 = 16 * 1024 * 1024;

/// Cache keys of the picker's lists.
pub const RECENT_STICKERS_KEY: &str = "recent_stickers";
/// See [`RECENT_STICKERS_KEY`].
pub const FAVOURITE_STICKERS_KEY: &str = "favourite_stickers";
/// See [`RECENT_STICKERS_KEY`].
pub const RECENT_GIFS_KEY: &str = "recent_gifs";

fn err(e: impl std::fmt::Display) -> Error {
    Error::Media(e.to_string())
}

/// Hex SHA-256 of `bytes` (the shelf key).
pub fn key_of(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

/// One sticker or GIF in a picker list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShelfItem {
    /// Hex SHA-256 of the file.
    pub key: String,
    /// Pixel size.
    pub width: u32,
    /// Pixel size.
    pub height: u32,
    /// Animated sticker.
    #[serde(default)]
    pub animated: bool,
    /// GIF duration in seconds.
    #[serde(default)]
    pub seconds: u32,
    /// Where a GIF came from.
    #[serde(default)]
    pub source: GifSource,
}

impl ShelfItem {
    /// A sticker entry.
    pub fn sticker(key: String, width: u32, height: u32, animated: bool) -> Self {
        Self {
            key,
            width,
            height,
            animated,
            seconds: 0,
            source: GifSource::None,
        }
    }

    /// Whether the key is a plain hex digest (never a path).
    pub fn valid(&self) -> bool {
        self.key.len() == 64 && self.key.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

/// Moves (or adds) `item` to the front of `list`, keeping at most `max`.
pub fn bump(list: &mut Vec<ShelfItem>, item: ShelfItem, max: usize) {
    list.retain(|i| i.key != item.key);
    list.insert(0, item);
    list.truncate(max);
}

/// Adds `item` at the end of the recent part unless it is listed already
/// (a sticker seen in a chat does not jump ahead of the ones you sent).
/// Returns whether the list changed.
pub fn note_seen(list: &mut Vec<ShelfItem>, item: ShelfItem, max: usize) -> bool {
    if list.iter().any(|i| i.key == item.key) {
        return false;
    }
    if list.len() >= max {
        // Seen-only stickers replace the oldest entry.
        list.truncate(max.saturating_sub(1));
    }
    list.push(item);
    true
}

/// Adds or removes a favourite; returns whether it is a favourite now.
pub fn toggle_favourite(list: &mut Vec<ShelfItem>, item: ShelfItem) -> bool {
    if list.iter().any(|i| i.key == item.key) {
        list.retain(|i| i.key != item.key);
        false
    } else {
        list.insert(0, item);
        list.truncate(MAX_FAVOURITES);
        true
    }
}

/// Parses a stored list (bad entries dropped).
pub fn parse_list(json: &str) -> Vec<ShelfItem> {
    serde_json::from_str::<Vec<ShelfItem>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter(ShelfItem::valid)
        .collect()
}

/// Serialises a list for the cache.
pub fn encode_list(list: &[ShelfItem]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".into())
}

/// Which shelf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `stickers/<key>.webp`.
    Sticker,
    /// `gifs/<key>.mp4` + `gifs/<key>.jpg`.
    Gif,
}

/// The shelf directories. Cheap to clone; every method blocks (call them
/// off the UI thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shelf {
    root: PathBuf,
}

impl Shelf {
    /// Shelves under `root` (the cache directory, or a private temporary
    /// directory in `--mock`).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The directory of one kind.
    pub fn dir(&self, kind: Kind) -> PathBuf {
        self.root.join(match kind {
            Kind::Sticker => "stickers",
            Kind::Gif => "gifs",
        })
    }

    /// Path of a sticker / GIF file.
    pub fn path(&self, kind: Kind, key: &str) -> PathBuf {
        self.dir(kind).join(match kind {
            Kind::Sticker => format!("{key}.webp"),
            Kind::Gif => format!("{key}.mp4"),
        })
    }

    /// Path of a GIF's poster.
    pub fn poster_path(&self, key: &str) -> PathBuf {
        self.dir(Kind::Gif).join(format!("{key}.jpg"))
    }

    /// Writes `bytes` (atomically: temporary file + rename). Existing
    /// files are kept (same key, same bytes).
    pub fn put(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if path.is_file() {
            return Ok(());
        }
        let dir = path.parent().ok_or_else(|| err("bad shelf path"))?;
        crate::config::secure_dir(dir).map_err(err)?;
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!(".tmp-{}-{n}", std::process::id()));
        std::fs::write(&tmp, bytes).map_err(err)?;
        std::fs::rename(&tmp, path).map_err(err)
    }

    /// Reads a file of at most `max` bytes.
    pub fn read(&self, path: &Path, max: u64) -> Result<Vec<u8>> {
        let len = std::fs::metadata(path).map_err(err)?.len();
        if len > max {
            return Err(err("file too large"));
        }
        std::fs::read(path).map_err(err)
    }

    /// Removes files no list mentions, then the oldest beyond the budget.
    pub fn prune(&self, kind: Kind, keep: &[&str], max_bytes: u64) {
        let dir = self.dir(kind);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.split('.').next().unwrap_or_default();
            if !keep.contains(&stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        crate::media::avatar::prune(&dir, usize::MAX, max_bytes);
    }
}

/// Fits `w` x `h` inside `side` x `side`, keeping the aspect ratio.
pub fn fit_side(w: u32, h: u32, side: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let s = f64::from(side) / f64::from(w.max(h));
    (
        ((f64::from(w) * s).round() as u32).clamp(1, side),
        ((f64::from(h) * s).round() as u32).clamp(1, side),
    )
}

/// Drops the low `bits` of every colour channel (alpha to 16 levels too):
/// lossless WebP of a photo shrinks a lot, cartoons barely change.
pub fn posterize(px: &mut [u8], bits: u32) {
    let mask = !((1u8 << bits.min(7)) - 1);
    for p in px.chunks_exact_mut(4) {
        for c in &mut p[..3] {
            *c &= mask;
        }
        p[3] = if p[3] < 8 { 0 } else { (p[3] & 0xF0) | 0x0F };
    }
}

fn encode_webp(img: &image::RgbaImage) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(err)?;
    Ok(out)
}

/// Turns an image file (PNG / JPEG / WebP / GIF, first frame) into a
/// 512x512 WebP sticker: scaled to fit, centred on transparency. `image`
/// only encodes lossless WebP, so a large result is re-encoded with fewer
/// colour levels until it is under [`STICKER_TARGET_BYTES`]; one still
/// above [`STICKER_MAX_BYTES`] is refused.
pub fn make_sticker(bytes: &[u8]) -> Result<Vec<u8>> {
    let img = crate::media::image::decode_limited(bytes)?;
    let (w, h) = fit_side(img.width(), img.height(), STICKER_SIDE);
    if w == 0 {
        return Err(err("empty image"));
    }
    let scaled = image::imageops::resize(
        &img.to_rgba8(),
        w,
        h,
        image::imageops::FilterType::CatmullRom,
    );
    let mut canvas = image::RgbaImage::new(STICKER_SIDE, STICKER_SIDE);
    image::imageops::overlay(
        &mut canvas,
        &scaled,
        i64::from((STICKER_SIDE - w) / 2),
        i64::from((STICKER_SIDE - h) / 2),
    );
    let mut out = encode_webp(&canvas)?;
    for bits in [2, 3, 4] {
        if out.len() <= STICKER_TARGET_BYTES {
            break;
        }
        let mut c = canvas.clone();
        posterize(&mut c, bits);
        out = encode_webp(&c)?;
    }
    if out.len() > STICKER_MAX_BYTES {
        return Err(err("this picture is too detailed for a sticker"));
    }
    Ok(out)
}

/// First frame of a sticker scaled to fit `side`, as RGBA (picker tiles and
/// the bubble of a sticker just sent).
pub fn still_rgba(bytes: &[u8], side: u32) -> Result<(u32, u32, Vec<u8>)> {
    let (w, h, px) = crate::media::image::decode_sticker(bytes)?;
    let img = image::RgbaImage::from_raw(w, h, px).ok_or_else(|| err("bad sticker"))?;
    let (tw, th) = fit_side(w, h, side);
    let img = if (tw, th) == (w, h) || w <= side && h <= side {
        img
    } else {
        image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle)
    };
    Ok((img.width(), img.height(), img.into_raw()))
}

/// Width and height from a WebP header.
pub fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::WebP)
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(k: char) -> ShelfItem {
        ShelfItem::sticker(k.to_string().repeat(64), 512, 512, false)
    }

    #[test]
    fn recents_are_bounded_and_sent_ones_move_to_the_front() {
        let mut l = Vec::new();
        for k in ['a', 'b', 'c'] {
            bump(&mut l, item(k), 2);
        }
        assert_eq!(l, vec![item('c'), item('b')]);
        bump(&mut l, item('b'), 2);
        assert_eq!(l[0], item('b'));
        // Seen in a chat: added at the end, never moved.
        assert!(note_seen(&mut l, item('d'), 3));
        assert!(!note_seen(&mut l, item('b'), 3));
        assert_eq!(l, vec![item('b'), item('c'), item('d')]);
        assert!(note_seen(&mut l, item('e'), 3));
        assert_eq!(l.len(), 3);
        assert_eq!(l[2], item('e'));
    }

    #[test]
    fn favourites_toggle_and_lists_round_trip() {
        let mut f = Vec::new();
        assert!(toggle_favourite(&mut f, item('a')));
        assert!(toggle_favourite(&mut f, item('b')));
        assert!(!toggle_favourite(&mut f, item('a')));
        assert_eq!(f, vec![item('b')]);
        assert_eq!(parse_list(&encode_list(&f)), f);
        // Hostile keys (paths) never survive parsing.
        let bad = r#"[{"key":"../../etc/passwd","width":1,"height":1}]"#;
        assert!(parse_list(bad).is_empty());
        assert!(parse_list("garbage").is_empty());
    }

    #[test]
    fn a_picture_becomes_a_512_px_webp_sticker() {
        let png = crate::mock::sunset_png(800, 400);
        let webp = make_sticker(&png).unwrap();
        assert_eq!(webp_size(&webp), Some((512, 512)));
        assert!(webp.len() <= STICKER_TARGET_BYTES, "{}", webp.len());
        // Centred on transparency: the top-left corner is clear.
        let (w, h, px) = still_rgba(&webp, 512).unwrap();
        assert_eq!((w, h), (512, 512));
        assert_eq!(px[3], 0);
        assert!(make_sticker(b"not an image").is_err());
        let (tw, th, _) = still_rgba(&webp, 96).unwrap();
        assert_eq!((tw, th), (96, 96));
    }

    #[test]
    fn the_shelf_keeps_listed_files_only() {
        let root = std::env::temp_dir().join(format!("ferrochat-shelf-{}", std::process::id()));
        let shelf = Shelf::new(root.clone());
        let a = key_of(b"a");
        let b = key_of(b"b");
        shelf.put(&shelf.path(Kind::Sticker, &a), b"a").unwrap();
        shelf.put(&shelf.path(Kind::Sticker, &b), b"b").unwrap();
        shelf.prune(Kind::Sticker, &[a.as_str()], 1 << 20);
        assert!(shelf.path(Kind::Sticker, &a).is_file());
        assert!(!shelf.path(Kind::Sticker, &b).exists());
        assert_eq!(
            shelf.read(&shelf.path(Kind::Sticker, &a), 10).unwrap(),
            b"a"
        );
        assert!(shelf.read(&shelf.path(Kind::Sticker, &a), 0).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
