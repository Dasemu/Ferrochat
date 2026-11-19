//! Profile pictures: decoding to small RGBA tiles and the on-disk cache.
//!
//! Network access lives in the protocol client; this module only turns bytes
//! into bounded tiles and decides what the disk cache holds. Both the
//! in-memory LRU (in `App`) and the disk cache stay bounded.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{Error, Result};

/// Edge of the square tile kept in memory (px). The largest avatar drawn
/// is 49 px; 96 keeps it crisp on 2x displays.
pub const TILE_PX: u32 = 96;
/// Source images above this many pixels are refused before decoding.
pub const MAX_SOURCE_PIXELS: u64 = 4_000_000;
/// Bytes of one decoded tile.
pub const TILE_BYTES: usize = (TILE_PX * TILE_PX * 4) as usize;
/// A cached picture is re-fetched after this long (changes also arrive as
/// `PictureUpdate` events, which invalidate immediately).
pub const FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
/// "No picture" answers are re-checked after this long.
pub const NONE_FRESH_FOR: Duration = Duration::from_secs(12 * 3600);
/// Disk cache bounds.
pub const MAX_DISK_FILES: usize = 1500;
/// Disk cache bound in bytes.
pub const MAX_DISK_BYTES: u64 = 32 * 1024 * 1024;

/// A decoded, square, downscaled avatar.
#[derive(Clone, PartialEq, Eq)]
pub struct Tile {
    /// Width = height = [`TILE_PX`].
    pub side: u32,
    /// RGBA pixels, `side * side * 4` bytes.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tile({0}x{0})", self.side)
    }
}

/// Decodes JPEG / PNG / WebP bytes into a centre-cropped square tile.
/// Never panics; oversize or corrupt input is an error.
pub fn prepare(bytes: &[u8]) -> Result<Tile> {
    super::image::check_image_bounds_max(bytes, MAX_SOURCE_PIXELS)?;
    let img = super::image::decode_limited(bytes)?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(Error::Media("empty image".into()));
    }
    let side = w.min(h);
    let cropped = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let tile = cropped.resize_exact(TILE_PX, TILE_PX, image::imageops::FilterType::Triangle);
    let mut rgba = tile.to_rgba8().into_raw();
    circle_mask(&mut rgba, TILE_PX);
    Ok(Tile {
        side: TILE_PX,
        rgba,
    })
}

/// Makes everything outside the inscribed circle transparent (with a
/// one-pixel antialiased edge). Done on the pixels because the CPU renderer
/// ignores image corner radii.
pub fn circle_mask(rgba: &mut [u8], side: u32) {
    let r = side as f32 / 2.0;
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
            let coverage = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            if let Some(a) = rgba.get_mut(((y * side + x) * 4 + 3) as usize) {
                *a = (f32::from(*a) * coverage) as u8;
            }
        }
    }
}

/// FNV-1a 64: a stable, dependency-free name for a JID so file names do not
/// expose phone numbers and survive Rust upgrades.
pub fn name_hash(jid: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in jid.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// What the disk cache says about a JID.
#[derive(Debug, PartialEq, Eq)]
pub enum Cached {
    /// A fresh picture.
    Picture(Vec<u8>),
    /// The contact has no (visible) picture, checked recently.
    NoPicture,
    /// Nothing usable: ask the server.
    Miss,
}

fn pic_path(dir: &Path, jid: &str) -> PathBuf {
    dir.join(format!("{}.img", name_hash(jid)))
}

fn none_path(dir: &Path, jid: &str) -> PathBuf {
    dir.join(format!("{}.none", name_hash(jid)))
}

fn age(path: &Path, now: SystemTime) -> Option<Duration> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(now.duration_since(m).unwrap_or_default())
}

/// Reads the disk cache entry for `jid`.
pub fn read(dir: &Path, jid: &str, now: SystemTime) -> Cached {
    let pic = pic_path(dir, jid);
    if age(&pic, now).is_some_and(|a| a < FRESH_FOR)
        && let Ok(bytes) = std::fs::read(&pic)
        && bytes.len() <= crate::net::whatsapp::MAX_AVATAR_BYTES
    {
        return Cached::Picture(bytes);
    }
    if age(&none_path(dir, jid), now).is_some_and(|a| a < NONE_FRESH_FOR) {
        return Cached::NoPicture;
    }
    Cached::Miss
}

/// Stores the server's answer (`None` = no picture). Best effort.
pub fn write(dir: &Path, jid: &str, bytes: Option<&[u8]>) -> Result<()> {
    crate::config::secure_dir(dir)?;
    match bytes {
        Some(b) => {
            let _ = std::fs::remove_file(none_path(dir, jid));
            std::fs::write(pic_path(dir, jid), b)?;
        }
        None => {
            let _ = std::fs::remove_file(pic_path(dir, jid));
            std::fs::write(none_path(dir, jid), b"")?;
        }
    }
    Ok(())
}

/// Forgets `jid` (its picture changed).
pub fn invalidate(dir: &Path, jid: &str) {
    let _ = std::fs::remove_file(pic_path(dir, jid));
    let _ = std::fs::remove_file(none_path(dir, jid));
}

/// Deletes the oldest files until at most `max_files` files and `max_bytes`
/// bytes remain. Returns how many files were removed.
pub fn prune(dir: &Path, max_files: usize, max_bytes: u64) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            md.is_file().then(|| {
                (
                    md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    md.len(),
                    e.path(),
                )
            })
        })
        .collect();
    files.sort_by_key(|(t, _, _)| std::cmp::Reverse(*t)); // newest first
    let (mut count, mut bytes, mut removed) = (0usize, 0u64, 0usize);
    for (_, len, path) in files {
        count += 1;
        bytes += len;
        if (count > max_files || bytes > max_bytes) && std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 7]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn tmp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ferrochat-avatar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn prepare_crops_square_and_resizes() {
        let t = prepare(&png(300, 150)).unwrap();
        assert_eq!(t.side, TILE_PX);
        assert_eq!(t.rgba.len(), TILE_BYTES);
        let t = prepare(&png(20, 20)).unwrap();
        assert_eq!(t.rgba.len(), TILE_BYTES);
        // Round: transparent corners, opaque centre.
        let at = |x: u32, y: u32| t.rgba[((y * TILE_PX + x) * 4 + 3) as usize];
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(TILE_PX - 1, TILE_PX - 1), 0);
        assert_eq!(at(TILE_PX / 2, TILE_PX / 2), 255);
    }

    #[test]
    fn prepare_rejects_garbage_and_bombs() {
        assert!(prepare(b"not an image").is_err());
        assert!(prepare(&[]).is_err());
        // A header claiming 20000x20000 must be refused without decoding.
        let big = png(3000, 3000);
        assert!(prepare(&big).is_err(), "9 Mpx > cap");
    }

    #[test]
    fn hash_is_stable_and_hides_the_number() {
        assert_eq!(
            name_hash("34600111222@s.whatsapp.net"),
            name_hash("34600111222@s.whatsapp.net")
        );
        assert_ne!(name_hash("a"), name_hash("b"));
        assert!(!name_hash("34600111222@s.whatsapp.net").contains("346"));
        assert_eq!(name_hash("").len(), 16);
    }

    #[test]
    fn disk_cache_roundtrip_and_freshness() {
        let d = tmp("rt");
        let now = SystemTime::now();
        assert_eq!(read(&d, "a", now), Cached::Miss);
        write(&d, "a", Some(b"JPEGDATA")).unwrap();
        assert_eq!(read(&d, "a", now), Cached::Picture(b"JPEGDATA".to_vec()));
        // Stale after a week.
        assert_eq!(
            read(&d, "a", now + FRESH_FOR + Duration::from_secs(1)),
            Cached::Miss
        );
        write(&d, "b", None).unwrap();
        assert_eq!(read(&d, "b", now), Cached::NoPicture);
        assert_eq!(
            read(&d, "b", now + NONE_FRESH_FOR + Duration::from_secs(1)),
            Cached::Miss
        );
        // A new picture replaces the "none" marker and vice versa.
        write(&d, "b", Some(b"X")).unwrap();
        assert_eq!(read(&d, "b", now), Cached::Picture(b"X".to_vec()));
        invalidate(&d, "b");
        assert_eq!(read(&d, "b", now), Cached::Miss);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn disk_cache_is_pruned_oldest_first() {
        let d = tmp("prune");
        for i in 0..10 {
            write(&d, &format!("j{i}"), Some(&[0u8; 100])).unwrap();
            std::thread::sleep(Duration::from_millis(15));
        }
        let removed = prune(&d, 4, u64::MAX);
        assert_eq!(removed, 6);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 4);
        // Newest survive.
        assert!(matches!(
            read(&d, "j9", SystemTime::now()),
            Cached::Picture(_)
        ));
        assert_eq!(read(&d, "j0", SystemTime::now()), Cached::Miss);
        let removed = prune(&d, 100, 250);
        assert_eq!(removed, 2, "byte cap keeps two 100-byte files");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn oversize_cache_file_is_ignored() {
        let d = tmp("big");
        std::fs::write(
            pic_path(&d, "a"),
            vec![0u8; crate::net::whatsapp::MAX_AVATAR_BYTES + 1],
        )
        .unwrap();
        assert_eq!(read(&d, "a", SystemTime::now()), Cached::Miss);
        let _ = std::fs::remove_dir_all(d);
    }
}
