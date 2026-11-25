//! GIFs: short silent MP4s (`videoMessage.gifPlayback`) that the official
//! client plays muted and looped inline, plus the GIF -> MP4 conversion used
//! to send a local `.gif`.
//!
//! Both run in the `ferrochat-player` helper, so the main binary links no
//! codec:
//!
//! * **Frames mode** (`--frames <w>x<h> <frames> <bytes> <file>`): the helper
//!   decodes the picture track only, as fast as it can, scaled to fit the
//!   bubble, keeps evenly spaced frames within a frame and byte budget
//!   ([`Sampler`]), writes them as timed pictures and exits. The app turns
//!   them into an [`crate::app::anim::Anim`] that the self-playing
//!   `ui::anim_image` widget loops; no decoder process stays alive and the
//!   app is never ticked.
//! * **Encode mode** (`--gif-to-mp4 <in.gif> <out.mp4>`): GIF frames are
//!   composited, scaled to at most [`ENCODE_MAX_EDGE`] px and encoded with
//!   OpenH264 (Constrained Baseline) into an MP4 with one H.264 track.

use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::video::Failure;
use super::video_proto::{self as proto, Msg};

/// Flag that selects the frames mode of the helper.
pub const FRAMES_FLAG: &str = "--frames";
/// Flag that selects the GIF -> MP4 encoder of the helper.
pub const ENCODE_FLAG: &str = "--gif-to-mp4";
/// Only the first this-many ms of a long clip are kept.
pub const MAX_SPAN_MS: u64 = 30_000;
/// Frames closer than this are never both kept (about 30 per second).
pub const MIN_GAP_MS: u64 = 33;
/// Shortest delay a kept frame is shown for.
pub const MIN_DELAY_MS: u64 = 20;
/// The helper is stopped if it has not finished after this long.
pub const DECODE_TIMEOUT: Duration = Duration::from_secs(20);
/// Encoding a GIF is stopped after this long.
pub const ENCODE_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest edge of an encoded GIF (WhatsApp's own GIFs are about 480 px).
pub const ENCODE_MAX_EDGE: u32 = 480;
/// Source GIF frames read at most (a minute at 25 fps).
pub const ENCODE_MAX_FRAMES: usize = 1500;
/// Largest `.gif` accepted for conversion.
pub const MAX_GIF_FILE: u64 = 40 * 1024 * 1024;

/// What the helper may produce for one clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSpec {
    /// Frames are scaled down (never up) to fit `max_w` x `max_h`.
    pub max_w: u32,
    /// See `max_w`.
    pub max_h: u32,
    /// Frames kept at most.
    pub max_frames: u32,
    /// RGBA bytes kept at most (all frames together).
    pub max_bytes: u64,
}

impl FrameSpec {
    /// Command-line arguments of the frames mode, before the file.
    pub fn to_args(self) -> Vec<String> {
        vec![
            FRAMES_FLAG.into(),
            format!("{}x{}", self.max_w, self.max_h),
            self.max_frames.to_string(),
            self.max_bytes.to_string(),
        ]
    }

    /// Parses what follows [`FRAMES_FLAG`]: `<w>x<h> <frames> <bytes>`.
    pub fn parse(box_: &str, frames: &str, bytes: &str) -> Option<FrameSpec> {
        let (w, h) = box_.split_once('x')?;
        let spec = FrameSpec {
            max_w: w.parse().ok()?,
            max_h: h.parse().ok()?,
            max_frames: frames.parse().ok()?,
            max_bytes: bytes.parse().ok()?,
        };
        let sane = (1..=proto::MAX_DIM).contains(&spec.max_w)
            && (1..=proto::MAX_DIM).contains(&spec.max_h)
            && (2..=1000).contains(&spec.max_frames)
            && spec.max_bytes > 0;
        sane.then_some(spec)
    }
}

/// Picks which decoded pictures to keep: evenly spaced over the clip (at
/// most [`MAX_SPAN_MS`] of it), so a long clip drops frames instead of
/// being cut short, within the frame and byte budget.
#[derive(Debug, Clone)]
pub struct Sampler {
    allowed: u32,
    gap: u64,
    end: u64,
    next_due: Option<u64>,
    kept: u32,
}

impl Sampler {
    /// `None` when not even two frames of `frame_bytes` fit the budget.
    pub fn new(spec: FrameSpec, duration_ms: u64, frame_bytes: u64) -> Option<Sampler> {
        let by_bytes = spec.max_bytes / frame_bytes.max(1);
        let allowed = u64::from(spec.max_frames).min(by_bytes);
        if allowed < 2 {
            return None;
        }
        let span = if duration_ms == 0 {
            MAX_SPAN_MS
        } else {
            duration_ms.min(MAX_SPAN_MS)
        };
        // Unknown duration: keep frames at the minimum gap until full.
        let gap = if duration_ms == 0 {
            MIN_GAP_MS
        } else {
            (span / allowed).max(MIN_GAP_MS)
        };
        Some(Sampler {
            allowed: allowed as u32,
            gap,
            end: span,
            next_due: None,
            kept: 0,
        })
    }

    /// Whether the picture shown at `time_ms` is kept.
    pub fn keep(&mut self, time_ms: u64) -> bool {
        if self.done() || time_ms >= self.end {
            return false;
        }
        // Half a gap of tolerance: a 25 fps clip with a 41 ms gap keeps
        // every frame instead of every second one.
        match self.next_due {
            Some(due) if time_ms + self.gap / 2 < due => false,
            due => {
                let mut due = due.unwrap_or(time_ms) + self.gap;
                while due <= time_ms {
                    due += self.gap;
                }
                self.next_due = Some(due);
                self.kept += 1;
                true
            }
        }
    }

    /// No further picture will be kept: decoding can stop.
    pub fn done(&self) -> bool {
        self.kept >= self.allowed || self.next_due.is_some_and(|d| d >= self.end)
    }
}

/// How long each kept frame shows, from their times: until the next one,
/// and the last one until the end of the clip (or for the usual gap when
/// the duration is unknown). The loop length is the clip's.
pub fn delays(times: &[u64], duration_ms: u64) -> Vec<Duration> {
    let mut out = Vec::with_capacity(times.len());
    let gaps: Vec<u64> = times
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]))
        .collect();
    let typical = if gaps.is_empty() {
        100
    } else {
        gaps.iter().sum::<u64>() / gaps.len() as u64
    };
    for (i, &t) in times.iter().enumerate() {
        let d = match times.get(i + 1) {
            Some(&next) => next.saturating_sub(t),
            None => {
                let end = duration_ms.min(MAX_SPAN_MS);
                if end > t { end - t } else { typical }
            }
        };
        out.push(Duration::from_millis(d.clamp(MIN_DELAY_MS, 5_000)));
    }
    out
}

/// Decoded frames of a clip: `(width, height, rgba, delay)`.
pub type Frames = Vec<(u32, u32, Vec<u8>, Duration)>;

/// Runs a helper command, reading its stdout with `read` while a watchdog
/// kills it after `timeout`. Blocking: call it from a detached thread.
fn run_helper<T>(
    mut cmd: Command,
    timeout: Duration,
    read: impl FnOnce(&mut dyn Read) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Failure::Unavailable(format!("cannot start the video helper: {e}")))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Failure::Unavailable("no pipe from the video helper".into()));
    };
    // Watchdog: the reader below ends when the helper exits or is killed.
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        match stop_rx.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
            }
        }
        child.wait().ok().map(|s| s.success())
    });
    let mut reader = BufReader::with_capacity(1 << 16, stdout);
    let result = read(&mut reader);
    // Drain the pipe so the helper is never blocked on a full pipe.
    let _ = std::io::copy(&mut reader.take(64 << 20), &mut std::io::sink());
    let _ = stop_tx.send(());
    let _ = watchdog.join();
    result
}

/// Runs `helper` in frames mode on `file` and collects what it keeps.
/// Blocking: call it from a detached thread. The process is killed after
/// [`DECODE_TIMEOUT`].
pub fn decode_with(helper: &Path, file: &Path, spec: FrameSpec) -> Result<Frames, Failure> {
    let mut cmd = Command::new(helper);
    cmd.args(spec.to_args()).arg(file);
    run_helper(cmd, DECODE_TIMEOUT, |r| read_frames(r, spec))
}

/// Reads a frames-mode stream to its end.
pub fn read_frames<R: Read + ?Sized>(r: &mut R, spec: FrameSpec) -> Result<Frames, Failure> {
    let mut duration = None;
    let mut pics: Vec<(u64, u32, u32, Vec<u8>)> = Vec::new();
    let mut bytes = 0u64;
    let mut r = r;
    loop {
        match proto::read_msg(&mut r) {
            Ok(Some(Msg::Ready { duration_ms, .. })) => duration = Some(duration_ms),
            Ok(Some(Msg::Picture {
                time_ms,
                width,
                height,
                rgba,
            })) => {
                // The helper is trusted to keep the budget, but never more
                // than asked is held here either.
                if width > spec.max_w || height > spec.max_h {
                    return Err(Failure::Broken("frame larger than asked".into()));
                }
                bytes += rgba.len() as u64;
                if pics.len() as u32 >= spec.max_frames || bytes > spec.max_bytes {
                    break;
                }
                pics.push((time_ms, width, height, rgba));
            }
            Ok(Some(Msg::Failed { missing, text })) => {
                return Err(if missing {
                    Failure::MissingCodec(text)
                } else {
                    Failure::Broken(text)
                });
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => return Err(Failure::Broken(format!("bad helper output: {e}"))),
        }
    }
    let Some(duration) = duration else {
        return Err(Failure::Unavailable("the helper exited".into()));
    };
    if pics.is_empty() {
        return Err(Failure::Broken("no picture".into()));
    }
    let times: Vec<u64> = pics.iter().map(|p| p.0).collect();
    Ok(pics
        .into_iter()
        .zip(delays(&times, duration))
        .map(|((_, w, h, px), d)| (w, h, px, d))
        .collect())
}

/// Facts about an MP4 that is about to be sent as a GIF.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// Picture width (as displayed).
    pub width: u32,
    /// Picture height.
    pub height: u32,
    /// Duration in ms (0 = unknown).
    pub duration_ms: u64,
    /// First picture, RGBA, scaled to fit [`PROBE_BOX`].
    pub rgba: Vec<u8>,
    /// Size of `rgba`.
    pub frame_size: (u32, u32),
}

/// The poster frame of a probe fits this box.
pub const PROBE_BOX: (u32, u32) = (480, 480);

/// Decodes the first picture of `file` (frames mode with a two-frame
/// budget) and reports its size and duration. Blocking.
pub fn probe_with(helper: &Path, file: &Path) -> Result<Probe, Failure> {
    let spec = FrameSpec {
        max_w: PROBE_BOX.0,
        max_h: PROBE_BOX.1,
        max_frames: 2,
        max_bytes: u64::from(PROBE_BOX.0 * PROBE_BOX.1 * 4) * 2,
    };
    let mut cmd = Command::new(helper);
    cmd.args(spec.to_args()).arg(file);
    run_helper(cmd, DECODE_TIMEOUT, |r| {
        let mut r = r;
        let mut size = None;
        loop {
            match proto::read_msg(&mut r) {
                Ok(Some(Msg::Ready {
                    width,
                    height,
                    duration_ms,
                })) => size = Some((width, height, duration_ms)),
                Ok(Some(Msg::Picture {
                    width: fw,
                    height: fh,
                    rgba,
                    ..
                })) => {
                    let (width, height, duration_ms) = size.unwrap_or((fw, fh, 0));
                    return Ok(Probe {
                        width,
                        height,
                        duration_ms,
                        rgba,
                        frame_size: (fw, fh),
                    });
                }
                Ok(Some(Msg::Failed { missing, text })) => {
                    return Err(if missing {
                        Failure::MissingCodec(text)
                    } else {
                        Failure::Broken(text)
                    });
                }
                Ok(Some(_)) => {}
                Ok(None) => return Err(Failure::Broken("no picture".into())),
                Err(e) => return Err(Failure::Broken(format!("bad helper output: {e}"))),
            }
        }
    })
}

/// Converts `gif` into an H.264 MP4 at `out` with the helper. Blocking.
pub fn encode_gif_with(helper: &Path, gif: &Path, out: &Path) -> Result<(), Failure> {
    let mut cmd = Command::new(helper);
    cmd.arg(ENCODE_FLAG).arg(gif).arg(out);
    run_helper(cmd, ENCODE_TIMEOUT, |r| {
        let mut r = r;
        let mut done = false;
        loop {
            match proto::read_msg(&mut r) {
                Ok(Some(Msg::Ready { .. })) => done = true,
                Ok(Some(Msg::Failed { missing, text })) => {
                    return Err(if missing {
                        Failure::MissingCodec(text)
                    } else {
                        Failure::Broken(text)
                    });
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => return Err(Failure::Broken(format!("bad helper output: {e}"))),
            }
        }
        if done && out.is_file() {
            Ok(())
        } else {
            Err(Failure::Unavailable("the helper exited".into()))
        }
    })
}

/// Splits an Annex-B byte stream (`00 00 01` / `00 00 00 01` start codes)
/// into NAL units, start codes removed.
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &(_, body)) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&(s, _)| s);
        // A four-byte start code leaves its leading zero behind.
        while end > body && data[end - 1] == 0 && k + 1 < starts.len() {
            end -= 1;
        }
        if end > body {
            out.push(&data[body..end]);
        }
    }
    out
}

/// One encoded GIF frame as MP4 sample data: the NAL units of the picture
/// as 4-byte length-prefixed units (SPS / PPS / AUD removed; they go into
/// `avcC`), plus whether it is an IDR.
pub fn annexb_to_sample(nals: &[&[u8]]) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut idr = false;
    for nal in nals {
        let Some(&h) = nal.first() else { continue };
        match h & 0x1f {
            7..=9 => continue,
            5 => idr = true,
            _ => {}
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    (out, idr)
}

/// Even output size of at most `max_edge` px for a `w` x `h` GIF (H.264
/// 4:2:0 needs even sides); never upscaled.
pub fn encode_size(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let scale = (f64::from(max_edge) / f64::from(w.max(h))).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(f64::from(w) * scale), even(f64::from(h) * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: FrameSpec = FrameSpec {
        max_w: 400,
        max_h: 300,
        max_frames: 48,
        max_bytes: 6 * 1024 * 1024,
    };

    #[test]
    fn args_round_trip_and_garbage_is_refused() {
        let a = SPEC.to_args();
        assert_eq!(a[0], FRAMES_FLAG);
        assert_eq!(FrameSpec::parse(&a[1], &a[2], &a[3]), Some(SPEC));
        assert_eq!(FrameSpec::parse("400", "48", "1"), None);
        assert_eq!(FrameSpec::parse("0x300", "48", "1"), None);
        assert_eq!(FrameSpec::parse("400x300", "1", "1"), None);
        assert_eq!(FrameSpec::parse("400x300", "48", "0"), None);
        assert_eq!(FrameSpec::parse("99999x1", "48", "1"), None);
    }

    /// Feeds pictures every `step` ms up to `dur` and returns the kept times.
    fn run(s: &mut Sampler, step: u64, dur: u64) -> Vec<u64> {
        let mut kept = Vec::new();
        let mut t = 0;
        while t < dur && !s.done() {
            if s.keep(t) {
                kept.push(t);
            }
            t += step;
        }
        kept
    }

    #[test]
    fn a_short_clip_keeps_every_frame_up_to_30_fps() {
        let mut s = Sampler::new(SPEC, 2_000, 200 * 100 * 4).unwrap();
        let kept = run(&mut s, 40, 2_000);
        assert_eq!(kept.len(), 48);
        assert!(kept.windows(2).all(|w| w[1] - w[0] >= MIN_GAP_MS));
        // 60 fps source: thinned to about 30.
        let mut s = Sampler::new(SPEC, 1_000, 200 * 100 * 4).unwrap();
        let kept = run(&mut s, 16, 1_000);
        assert!((25..=31).contains(&kept.len()), "{}", kept.len());
    }

    #[test]
    fn a_long_clip_is_spread_not_cut() {
        let frame = 400 * 225 * 4;
        let mut s = Sampler::new(SPEC, 10_000, frame).unwrap();
        let kept = run(&mut s, 40, 10_000);
        let allowed = (SPEC.max_bytes / frame) as usize;
        assert_eq!(kept.len(), allowed);
        assert!(*kept.last().unwrap() > 8_000, "covers the clip: {kept:?}");
    }

    #[test]
    fn huge_frames_are_refused() {
        assert!(Sampler::new(SPEC, 1_000, SPEC.max_bytes).is_none());
    }

    #[test]
    fn delays_follow_the_times_and_end_with_the_clip() {
        let d = delays(&[0, 100, 250], 400);
        assert_eq!(d, [100, 150, 150].map(Duration::from_millis).to_vec());
        assert_eq!(delays(&[0, 100, 200], 0)[2], Duration::from_millis(100));
        assert_eq!(delays(&[5, 5], 10)[0], Duration::from_millis(MIN_DELAY_MS));
    }

    fn stream(msgs: &[Msg]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in msgs {
            proto::write_msg(&mut buf, m).unwrap();
        }
        buf
    }

    fn pic(t: u64) -> Msg {
        Msg::Picture {
            time_ms: t,
            width: 2,
            height: 1,
            rgba: vec![1; 8],
        }
    }

    fn ready() -> Msg {
        Msg::Ready {
            width: 2,
            height: 1,
            duration_ms: 300,
        }
    }

    #[test]
    fn a_frames_stream_becomes_timed_frames() {
        let buf = stream(&[ready(), pic(0), pic(100), pic(200)]);
        let f = read_frames(&mut buf.as_slice(), SPEC).unwrap();
        assert_eq!(f.len(), 3);
        assert!(f.iter().all(|x| x.3 == Duration::from_millis(100)));
    }

    #[test]
    fn failures_and_hostile_streams_are_errors() {
        let failed = stream(&[Msg::Failed {
            missing: true,
            text: "VP9".into(),
        }]);
        assert_eq!(
            read_frames(&mut failed.as_slice(), SPEC),
            Err(Failure::MissingCodec("VP9".into()))
        );
        assert!(matches!(
            read_frames(&mut [].as_slice(), SPEC),
            Err(Failure::Unavailable(_))
        ));
        let big = stream(&[
            ready(),
            Msg::Picture {
                time_ms: 0,
                width: 401,
                height: 1,
                rgba: vec![0; 401 * 4],
            },
        ]);
        assert!(read_frames(&mut big.as_slice(), SPEC).is_err());
        // More frames than asked are never held.
        let mut many = vec![ready()];
        many.extend((0..100).map(|i| pic(i * 40)));
        let f = read_frames(&mut stream(&many).as_slice(), SPEC).unwrap();
        assert_eq!(f.len(), SPEC.max_frames as usize);
    }

    #[test]
    fn annexb_units_become_length_prefixed_samples() {
        // SPS, PPS (dropped: they go into avcC) and an IDR slice.
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9, 9,
        ];
        let nals = split_annexb(&data);
        assert_eq!(nals, vec![&[0x67, 1, 2][..], &[0x68, 3], &[0x65, 9, 9, 9]]);
        let (sample, idr) = annexb_to_sample(&nals);
        assert!(idr);
        assert_eq!(sample, vec![0, 0, 0, 4, 0x65, 9, 9, 9]);
        let (p, idr) = annexb_to_sample(&[&[0x41, 7]]);
        assert!(!idr);
        assert_eq!(p, vec![0, 0, 0, 2, 0x41, 7]);
    }

    #[test]
    fn encode_sizes_are_even_and_never_upscaled() {
        assert_eq!(encode_size(1000, 500, 480), (480, 240));
        assert_eq!(encode_size(101, 51, 480), (100, 50));
        assert_eq!(encode_size(1, 1, 480), (2, 2));
        assert_eq!(encode_size(0, 10, 480), (0, 0));
        assert_eq!(encode_size(500, 1000, 480), (240, 480));
    }
}
