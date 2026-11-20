//! The audio track: AAC from the MP4 with symphonia (pure Rust), played
//! through rodio. Audio is decoded a little ahead of the clock and queued;
//! a seek clears the queue and repositions the reader.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_AAC, Decoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::units::Time;
use symphonia::default::codecs::AacDecoder;
use symphonia::default::formats::IsoMp4Reader;

/// How far ahead of the clock audio is queued.
pub const AHEAD_MS: i64 = 700;

/// Decoded audio source.
pub struct AudioTrack {
    reader: IsoMp4Reader,
    decoder: AacDecoder,
    track: u32,
    rate: u32,
    channels: u16,
    /// End of the audio decoded so far, ms.
    pub decoded_until: i64,
    /// No more packets.
    pub done: bool,
}

impl AudioTrack {
    /// Opens the AAC track. `Ok(None)` when there is none; `Err` names the
    /// step that failed (shown on stderr only: the video still plays).
    pub fn open(path: &Path) -> Result<Option<Self>, String> {
        let f = File::open(path).map_err(|e| format!("open: {e}"))?;
        let mss = MediaSourceStream::new(Box::new(f), Default::default());
        let reader = IsoMp4Reader::try_new(mss, &FormatOptions::default())
            .map_err(|e| format!("mp4 audio demux: {e}"))?;
        let Some(track) = reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec == CODEC_TYPE_AAC)
        else {
            return Ok(None);
        };
        let params = track.codec_params.clone();
        let track = track.id;
        let decoder = AacDecoder::try_new(&params, &DecoderOptions::default())
            .map_err(|e| format!("AAC decoder: {e}"))?;
        // The MP4 sample entry often leaves the layout to the decoder config
        // (AudioSpecificConfig): ask the decoder, which has parsed it.
        let dp = decoder.codec_params();
        // Missing here, they are taken from the first decoded packet.
        let rate = dp.sample_rate.or(params.sample_rate).unwrap_or(0);
        let channels = dp
            .channels
            .or(params.channels)
            .map(|c| c.count() as u16)
            .unwrap_or(0);
        Ok(Some(Self {
            reader,
            decoder,
            track,
            rate,
            channels,
            decoded_until: 0,
            done: false,
        }))
    }

    /// Output format.
    pub fn format(&self) -> (u16, u32) {
        (self.channels, self.rate)
    }

    /// Next chunk of interleaved samples and its start time (ms), or `None`
    /// at the end. Damaged packets are skipped.
    pub fn next_chunk(&mut self) -> Option<(i64, Vec<f32>)> {
        for _ in 0..64 {
            let packet = match self.reader.next_packet() {
                Ok(p) => p,
                Err(_) => {
                    self.done = true;
                    return None;
                }
            };
            if packet.track_id() != self.track {
                continue;
            }
            let Ok(decoded) = self.decoder.decode(&packet) else {
                continue;
            };
            let spec = *decoded.spec();
            let ch = spec.channels.count() as u16;
            if (self.channels == 0 || self.rate == 0) && (1..=8).contains(&ch) && spec.rate > 0 {
                self.channels = ch;
                self.rate = spec.rate;
            }
            let start = (packet.ts() as i64).saturating_mul(1000) / i64::from(self.rate.max(1));
            if ch != self.channels || spec.rate != self.rate {
                // A mid-stream format change: not something phones produce.
                continue;
            }
            let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            let samples = buf.samples().to_vec();
            let frames = samples.len() as i64 / i64::from(self.channels.max(1));
            self.decoded_until = start + frames * 1000 / i64::from(self.rate.max(1));
            return Some((start, samples));
        }
        None
    }

    /// Repositions to `ms`.
    pub fn seek(&mut self, ms: u64) {
        let _ = self.reader.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(ms as f64 / 1000.0),
                track_id: Some(self.track),
            },
        );
        self.decoder.reset();
        self.decoded_until = ms as i64;
        self.done = false;
    }
}

/// The sound card side. Missing (no device, CI): playback is silent.
pub struct Output {
    // Field order: the sink must drop before its stream.
    sink: rodio::Sink,
    _stream: rodio::OutputStream,
}

impl Output {
    /// Opens the default output device.
    pub fn open() -> Option<Self> {
        let mut stream = rodio::OutputStreamBuilder::open_default_stream().ok()?;
        stream.log_on_drop(false);
        let sink = rodio::Sink::connect_new(stream.mixer());
        Some(Self {
            sink,
            _stream: stream,
        })
    }

    /// Queues samples.
    pub fn push(&self, channels: u16, rate: u32, samples: Vec<f32>) {
        self.sink
            .append(rodio::buffer::SamplesBuffer::new(channels, rate, samples));
    }

    /// Drops everything queued (seek).
    pub fn clear(&self, playing: bool) {
        self.sink.clear();
        if playing {
            self.sink.play();
        }
    }

    /// Pause / resume.
    pub fn set_playing(&self, playing: bool) {
        if playing {
            self.sink.play()
        } else {
            self.sink.pause()
        }
    }

    /// Volume 0..=1 (0 when muted).
    pub fn set_volume(&self, v: f32) {
        self.sink.set_volume(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/mock-clip.mp4")
    }

    #[test]
    fn the_aac_track_of_a_phone_style_clip_decodes() {
        let mut a = AudioTrack::open(&clip()).unwrap().expect("aac track");
        let mut samples = 0usize;
        while let Some((_, s)) = a.next_chunk() {
            samples += s.len();
        }
        let (ch, rate) = a.format();
        assert_eq!((ch, rate), (1, 44_100));
        let secs = samples as f64 / f64::from(rate) / f64::from(ch);
        assert!((2.8..3.3).contains(&secs), "{secs} s of audio");
        assert!(a.done);
    }

    #[test]
    fn seeking_restarts_near_the_target() {
        let mut a = AudioTrack::open(&clip()).unwrap().unwrap();
        a.seek(2000);
        let (start, _) = a.next_chunk().unwrap();
        assert!((1900..=2100).contains(&start), "{start}");
    }

    #[test]
    fn a_file_without_audio_has_no_track() {
        let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rotated-portrait.mp4");
        assert!(AudioTrack::open(&f).unwrap().is_none());
    }
}
