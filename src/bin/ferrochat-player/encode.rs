//! `--gif-to-mp4 <in.gif> <out.mp4>`: turns an animated GIF into the H.264
//! MP4 WhatsApp sends as a GIF (`gifPlayback`).
//!
//! The GIF is decoded and composited by the `image` crate, scaled to at most
//! `ENCODE_MAX_EDGE` px (even sides), flattened on white (H.264 has no
//! alpha), encoded with OpenH264 (Baseline, no B-frames, so samples are in
//! presentation order) and muxed with the `mp4` crate's writer. Frame
//! delays become sample durations, so uneven GIF timing is kept exactly.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use ferrochat::media::gifv::{
    ENCODE_MAX_EDGE, ENCODE_MAX_FRAMES, MAX_GIF_FILE, annexb_to_sample, encode_size, split_annexb,
};
use image::AnimationDecoder;
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, Profile, RateControlMode,
    UsageType,
};
use openh264::formats::{RgbSliceU8, YUVBuffer};

/// Largest GIF canvas decoded.
const MAX_SOURCE_EDGE: u32 = 4096;
/// Delays this short mean "as fast as possible"; browsers show 100 ms.
const FAST_DELAY_MS: u32 = 10;
const DEFAULT_DELAY_MS: u32 = 100;
/// Media timescale: milliseconds.
const TIMESCALE: u32 = 1000;

/// One composited, scaled RGB frame and how long it shows.
struct Rgb {
    px: Vec<u8>,
    delay_ms: u32,
}

/// Converts `input` into `output`. Errors are user-facing (no paths).
pub fn gif_to_mp4(input: &Path, output: &Path) -> Result<(u32, u32, u64), String> {
    let len = std::fs::metadata(input)
        .map_err(|_| "the GIF file cannot be read".to_string())?
        .len();
    if len > MAX_GIF_FILE {
        return Err("the GIF is larger than 40 MB".into());
    }
    let f = File::open(input).map_err(|_| "the GIF file cannot be read".to_string())?;
    let decoder = image::codecs::gif::GifDecoder::new(BufReader::new(f))
        .map_err(|e| format!("not a GIF file ({e})"))?;
    let (sw, sh) = image::ImageDecoder::dimensions(&decoder);
    if sw == 0 || sh == 0 || sw > MAX_SOURCE_EDGE || sh > MAX_SOURCE_EDGE {
        return Err("the GIF is too large".into());
    }
    let (w, h) = encode_size(sw, sh, ENCODE_MAX_EDGE);
    let mut frames = Vec::new();
    for f in decoder.into_frames().take(ENCODE_MAX_FRAMES) {
        let f = f.map_err(|e| format!("the GIF is damaged ({e})"))?;
        let (n, d) = f.delay().numer_denom_ms();
        let ms = n.checked_div(d).unwrap_or(0);
        let delay_ms = if ms <= FAST_DELAY_MS {
            DEFAULT_DELAY_MS
        } else {
            ms
        };
        let buf = f.into_buffer();
        let buf = if buf.dimensions() == (w, h) {
            buf
        } else {
            image::imageops::resize(&buf, w, h, image::imageops::FilterType::Triangle)
        };
        frames.push(Rgb {
            px: flatten_on_white(buf.as_raw()),
            delay_ms,
        });
    }
    if frames.is_empty() {
        return Err("the GIF has no frames".into());
    }
    let duration: u64 = frames.iter().map(|f| u64::from(f.delay_ms)).sum();
    encode(&frames, w, h, output)?;
    Ok((w, h, duration))
}

/// RGBA -> RGB over a white background.
fn flatten_on_white(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for p in rgba.chunks_exact(4) {
        let a = u32::from(p[3]);
        for c in &p[..3] {
            out.push(((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    out
}

fn encode(frames: &[Rgb], w: u32, h: u32, output: &Path) -> Result<(), String> {
    let total: u64 = frames.iter().map(|f| u64::from(f.delay_ms)).sum();
    let fps = (frames.len() as f32 * 1000.0 / total.max(1) as f32).clamp(1.0, 60.0);
    let config = EncoderConfig::new()
        .profile(Profile::Baseline)
        .usage_type(UsageType::CameraVideoRealTime)
        .rate_control_mode(RateControlMode::Quality)
        .bitrate(BitRate::from_bps(1_200_000))
        .max_frame_rate(FrameRate::from_hz(fps))
        // A skipped frame would leave a hole in the sample table.
        .skip_frames(false)
        // A keyframe about every two seconds, so players can seek / loop.
        .intra_frame_period(IntraFramePeriod::from_num_frames((fps * 2.0) as u32 + 1));
    let mut encoder = Encoder::with_api_config(openh264::OpenH264API::from_source(), config)
        .map_err(|e| format!("the H.264 encoder could not start: {e}"))?;

    let mut sps = None;
    let mut pps = None;
    // (sample bytes, is_sync, duration)
    let mut samples: Vec<(Vec<u8>, bool, u32)> = Vec::with_capacity(frames.len());
    for f in frames {
        let yuv = YUVBuffer::from_rgb8_source(RgbSliceU8::new(&f.px, (w as usize, h as usize)));
        let bs = encoder
            .encode(&yuv)
            .map_err(|e| format!("H.264 encoding failed: {e}"))?;
        let data = bs.to_vec();
        let nals = split_annexb(&data);
        for nal in &nals {
            match nal.first().map(|b| b & 0x1f) {
                Some(7) if sps.is_none() => sps = Some(nal.to_vec()),
                Some(8) if pps.is_none() => pps = Some(nal.to_vec()),
                _ => {}
            }
        }
        let (sample, idr) = annexb_to_sample(&nals);
        if sample.is_empty() {
            // Nothing coded for this picture: the previous one shows longer.
            if let Some(last) = samples.last_mut() {
                last.2 += f.delay_ms;
            }
            continue;
        }
        samples.push((sample, idr, f.delay_ms));
    }
    let (Some(sps), Some(pps)) = (sps, pps) else {
        return Err("the H.264 encoder produced no stream header".into());
    };
    if samples.first().is_none_or(|s| !s.1) {
        return Err("the H.264 stream does not start with a keyframe".into());
    }

    let file = File::create(output).map_err(|_| "cannot write the converted GIF".to_string())?;
    let config = mp4::Mp4Config {
        major_brand: "isom".parse().map_err(|_| "mp4 brand".to_string())?,
        minor_version: 512,
        compatible_brands: ["isom", "iso2", "avc1", "mp41"]
            .iter()
            .filter_map(|b| b.parse().ok())
            .collect(),
        timescale: TIMESCALE,
    };
    let mut writer = mp4::Mp4Writer::write_start(BufWriter::new(file), &config)
        .map_err(|e| format!("cannot write the MP4: {e}"))?;
    writer
        .add_track(&mp4::TrackConfig {
            track_type: mp4::TrackType::Video,
            timescale: TIMESCALE,
            language: "und".into(),
            media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                width: w as u16,
                height: h as u16,
                seq_param_set: sps,
                pic_param_set: pps,
            }),
        })
        .map_err(|e| format!("cannot write the MP4: {e}"))?;
    let mut t = 0u64;
    for (bytes, sync, dur) in samples {
        writer
            .write_sample(
                1,
                &mp4::Mp4Sample {
                    start_time: t,
                    duration: dur,
                    rendering_offset: 0,
                    is_sync: sync,
                    bytes: mp4::Bytes::from(bytes),
                },
            )
            .map_err(|e| format!("cannot write the MP4: {e}"))?;
        t += u64::from(dur);
    }
    writer
        .write_end()
        .map_err(|e| format!("cannot write the MP4: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparency_is_flattened_on_white() {
        assert_eq!(flatten_on_white(&[10, 20, 30, 255]), vec![10, 20, 30]);
        assert_eq!(flatten_on_white(&[0, 0, 0, 0]), vec![255, 255, 255]);
        let half = flatten_on_white(&[0, 0, 0, 128]);
        assert!((126..=128).contains(&half[0]));
    }
}
