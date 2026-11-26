//! Decoded animations (animated stickers, GIFs, GIF-picker tiles) and the
//! clock that decides which frame is on screen.
//!
//! Rules that keep idle cost at zero (same design as Ferrocord):
//! * frames are decoded off the UI thread, downscaled to the drawn size and
//!   wrapped in `image::Handle`s once (never in `view`);
//! * every asset is capped in source frames, kept frames and bytes (extra
//!   frames are merged, so the animation keeps its duration);
//! * the app never ticks for an animation: the self-playing
//!   `ui::anim_image` widget asks for a redraw only when its own next frame
//!   is due (on a shared 60 Hz grid), only while it is on screen and only
//!   while the window is focused.

use std::io::Cursor;
use std::time::Duration;

use iced::widget::image::Handle;
use image::{AnimationDecoder, ImageDecoder};

/// Fallback wait when a frame lookup falls off the end (rounding).
pub const MIN_DELAY: Duration = Duration::from_millis(10);
/// Delays of 10 ms or less mean "as fast as possible"; browsers (and the
/// official client) show them for 100 ms.
const DEFAULT_DELAY: Duration = Duration::from_millis(100);
const FAST_DELAY: Duration = Duration::from_millis(10);
/// Larger sources are refused before decoding.
pub const MAX_SOURCE_DIM: u32 = 2048;

/// What an animation may cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Source frames read at most.
    pub max_read: usize,
    /// Frames kept at most.
    pub max_frames: usize,
    /// Bytes of kept frames at most.
    pub max_bytes: usize,
    /// Frames wider or taller than this are downscaled to it.
    pub target: u32,
}

const MB: usize = 1024 * 1024;

/// A sticker in a bubble (drawn at 150 px; decoded a little larger for
/// HiDPI scaling).
pub const STICKER: Limits = Limits {
    max_read: 600,
    max_frames: 150,
    max_bytes: 8 * MB,
    target: 200,
};

/// A GIF in a bubble (decoded by the helper's frames mode).
pub const GIF: Limits = Limits {
    max_read: 0,
    max_frames: 150,
    max_bytes: 12 * MB,
    target: 330,
};

/// A GIF-picker search tile.
pub const TILE: Limits = Limits {
    max_read: 300,
    max_frames: 60,
    max_bytes: MB,
    target: 120,
};

/// One frame.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The picture.
    pub handle: Handle,
    /// How long it shows.
    pub delay: Duration,
}

/// A decoded animation. Shared through `Arc`.
#[derive(Debug)]
pub struct Anim {
    /// Frames in order.
    pub frames: Vec<Frame>,
    /// Loop length.
    pub total: Duration,
    /// RGBA bytes held.
    pub bytes: usize,
    /// Frame size.
    pub size: (u32, u32),
}

impl Anim {
    /// Frame index showing `elapsed` after the start (the animation loops).
    pub fn index_at(&self, elapsed: Duration) -> usize {
        if self.frames.len() <= 1 || self.total.is_zero() {
            return 0;
        }
        let mut t = Duration::from_nanos((elapsed.as_nanos() % self.total.as_nanos()) as u64);
        for (i, f) in self.frames.iter().enumerate() {
            if t < f.delay {
                return i;
            }
            t -= f.delay;
        }
        self.frames.len() - 1
    }

    /// Time from `elapsed` until the shown frame changes; `None` for a
    /// still image (nothing to redraw).
    pub fn until_next(&self, elapsed: Duration) -> Option<Duration> {
        if self.frames.len() <= 1 || self.total.is_zero() {
            return None;
        }
        let mut t = Duration::from_nanos((elapsed.as_nanos() % self.total.as_nanos()) as u64);
        for f in &self.frames {
            if t < f.delay {
                return Some(f.delay - t);
            }
            t -= f.delay;
        }
        Some(MIN_DELAY)
    }

    /// The first frame (a still for places that do not animate).
    pub fn first(&self) -> Option<&Handle> {
        self.frames.first().map(|f| &f.handle)
    }
}

/// Why an animation could not be made.
#[derive(Debug, PartialEq, Eq)]
pub enum AnimError {
    /// Not a GIF / WebP, or damaged.
    Unsupported,
    /// A single frame: use the static image path.
    NotAnimated,
    /// Over the size / frame / byte limits.
    TooLarge,
}

struct Raw {
    w: u32,
    h: u32,
    px: Vec<u8>,
    delay: Duration,
}

/// A frame's delay, exactly; only "as fast as possible" delays (10 ms or
/// less) become the browser default.
pub fn delay_of(d: image::Delay) -> Duration {
    let (n, den) = d.numer_denom_ms();
    let d = if den == 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos(u64::from(n) * 1_000_000 / u64::from(den))
    };
    if d <= FAST_DELAY { DEFAULT_DELAY } else { d }
}

/// Merges neighbouring frames (dropping every second one, adding its delay
/// to the kept one): half the memory, same duration.
fn decimate(v: &mut Vec<Raw>) {
    let old = std::mem::take(v);
    let mut it = old.into_iter();
    while let Some(mut a) = it.next() {
        if let Some(b) = it.next() {
            a.delay += b.delay;
        }
        v.push(a);
    }
}

fn scale_to(buf: image::RgbaImage, target: u32) -> image::RgbaImage {
    let (w, h) = buf.dimensions();
    if target == 0 || (w <= target && h <= target) {
        return buf;
    }
    let s = f64::from(target) / f64::from(w.max(h));
    let nw = ((f64::from(w) * s).round() as u32).max(1);
    let nh = ((f64::from(h) * s).round() as u32).max(1);
    image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Triangle)
}

/// Reads frames within the limits; the stride doubles each time a limit is
/// hit, so every kept frame stands for the same number of source frames.
fn collect(frames: image::Frames<'_>, lim: &Limits) -> Result<Vec<Raw>, AnimError> {
    let mut out: Vec<Raw> = Vec::new();
    let mut bytes = 0usize;
    let mut stride = 1usize;
    let mut merged = 0usize;
    for (n, f) in frames.enumerate() {
        if lim.max_read > 0 && n >= lim.max_read {
            break;
        }
        let Ok(f) = f else { break };
        let delay = delay_of(f.delay());
        if merged > 0
            && merged < stride
            && let Some(last) = out.last_mut()
        {
            last.delay += delay;
            merged += 1;
            continue;
        }
        let buf = scale_to(f.into_buffer(), lim.target);
        let (w, h) = buf.dimensions();
        let px = buf.into_raw();
        bytes += px.len();
        out.push(Raw { w, h, px, delay });
        merged = 1;
        while out.len() > lim.max_frames || bytes > lim.max_bytes {
            if out.len() < 2 {
                return Err(AnimError::TooLarge);
            }
            if out.len().is_multiple_of(2) {
                merged += stride;
            }
            decimate(&mut out);
            stride *= 2;
            bytes = out.iter().map(|r| r.px.len()).sum();
        }
    }
    Ok(out)
}

/// Decodes an animated WebP or GIF. Single-frame images are
/// [`AnimError::NotAnimated`].
pub fn decode(bytes: &[u8], lim: &Limits) -> Result<Anim, AnimError> {
    let cur = Cursor::new(bytes);
    let too_big = |(w, h): (u32, u32)| w > MAX_SOURCE_DIM || h > MAX_SOURCE_DIM;
    let raws = if bytes.starts_with(b"GIF8") {
        let d = image::codecs::gif::GifDecoder::new(cur).map_err(|_| AnimError::Unsupported)?;
        if too_big(d.dimensions()) {
            return Err(AnimError::TooLarge);
        }
        collect(d.into_frames(), lim)?
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let d = image::codecs::webp::WebPDecoder::new(cur).map_err(|_| AnimError::Unsupported)?;
        if too_big(d.dimensions()) {
            return Err(AnimError::TooLarge);
        }
        if !d.has_animation() {
            return Err(AnimError::NotAnimated);
        }
        collect(d.into_frames(), lim)?
    } else {
        return Err(AnimError::Unsupported);
    };
    if raws.len() < 2 {
        return Err(AnimError::NotAnimated);
    }
    Ok(build(raws))
}

fn build(raws: Vec<Raw>) -> Anim {
    let size = raws.first().map_or((0, 0), |r| (r.w, r.h));
    let mut total = Duration::ZERO;
    let mut bytes = 0;
    let frames = raws
        .into_iter()
        .map(|r| {
            total += r.delay;
            bytes += r.px.len();
            Frame {
                handle: Handle::from_rgba(r.w, r.h, r.px),
                delay: r.delay,
            }
        })
        .collect();
    Anim {
        frames,
        total,
        bytes,
        size,
    }
}

/// Builds an animation from ready RGBA frames (the helper's frames mode,
/// mock data and tests).
pub fn from_rgba_frames(frames: Vec<(u32, u32, Vec<u8>, Duration)>) -> Anim {
    build(
        frames
            .into_iter()
            .map(|(w, h, px, delay)| Raw { w, h, px, delay })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anim(delays: &[u64]) -> Anim {
        from_rgba_frames(
            delays
                .iter()
                .map(|d| (1, 1, vec![0u8; 4], Duration::from_millis(*d)))
                .collect(),
        )
    }

    #[test]
    fn frames_follow_their_delays_and_loop() {
        let a = anim(&[100, 50, 200]);
        assert_eq!(a.index_at(Duration::ZERO), 0);
        assert_eq!(a.index_at(Duration::from_millis(120)), 1);
        assert_eq!(a.index_at(Duration::from_millis(200)), 2);
        assert_eq!(a.index_at(Duration::from_millis(360)), 0);
        assert_eq!(
            a.until_next(Duration::from_millis(120)),
            Some(Duration::from_millis(30))
        );
        assert_eq!(anim(&[100]).until_next(Duration::ZERO), None);
    }

    #[test]
    fn animated_webp_stickers_decode_within_limits() {
        let webp = crate::mock::animated_sticker_webp();
        assert!(whatsapp_rust::webp::is_animated(&webp));
        let a = decode(&webp, &STICKER).unwrap();
        assert!(a.frames.len() >= 2);
        assert!(a.size.0 <= STICKER.target && a.size.1 <= STICKER.target);
        assert!(!a.total.is_zero());
        // A tight budget merges frames but keeps the loop length.
        let tight = Limits {
            max_frames: 2,
            ..STICKER
        };
        let b = decode(&webp, &tight).unwrap();
        assert!(b.frames.len() <= 2);
        assert_eq!(b.total, a.total);
        // Still images and junk are told apart.
        assert_eq!(
            decode(&crate::mock::sticker_webp(), &STICKER).unwrap_err(),
            AnimError::NotAnimated
        );
        assert_eq!(
            decode(b"junk", &STICKER).unwrap_err(),
            AnimError::Unsupported
        );
    }

    #[test]
    fn fast_delays_become_the_browser_default() {
        assert_eq!(
            delay_of(image::Delay::from_numer_denom_ms(0, 1)),
            DEFAULT_DELAY
        );
        assert_eq!(
            delay_of(image::Delay::from_numer_denom_ms(40, 1)),
            Duration::from_millis(40)
        );
    }
}
