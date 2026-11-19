//! Voice notes: Ogg/Opus decode + encode, playback and recording.
//!
//! WhatsApp voice notes are Opus in an Ogg container (`audio/ogg; codecs=opus`).
//! `symphonia` has no Opus decoder, so we demux with the pure-Rust `ogg`
//! crate and decode/encode with `opus` (libopus bindings, the reference
//! codec). Playback and capture go through `rodio` / its `cpal` backend
//! (ALSA / PipeWire on Linux).

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::error::{Error, Result};

/// Opus always runs at 48 kHz internally.
pub const OPUS_RATE: u32 = 48_000;
const FRAME: usize = 960; // 20 ms at 48 kHz
const PRE_SKIP: u16 = 312;

/// Decoded PCM, interleaved f32.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    /// Samples, interleaved.
    pub samples: Vec<f32>,
    /// Channels (1 or 2).
    pub channels: u16,
    /// Sample rate (always 48 kHz for Opus).
    pub rate: u32,
}

impl Pcm {
    /// Duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        if self.channels == 0 || self.rate == 0 {
            return 0;
        }
        self.samples.len() as u64 * 1000 / (self.channels as u64 * self.rate as u64)
    }
}

fn media_err(e: impl std::fmt::Display) -> Error {
    Error::Media(e.to_string())
}

/// Hard cap on decoded audio: 10 minutes per channel at 48 kHz.
pub const MAX_DECODED_SECONDS: u64 = 10 * 60;
/// Hard cap on Opus packets (20 ms each → 30 minutes; real notes use far fewer).
pub const MAX_PACKETS: usize = 90_000;

/// Decodes an Ogg/Opus file with no declared duration (10-minute cap).
pub fn decode_ogg_opus(bytes: &[u8]) -> Result<Pcm> {
    decode_ogg_opus_bounded(bytes, None)
}

/// Decodes an Ogg/Opus file, refusing to produce more audio than
/// `declared_secs` (plus slack) or [`MAX_DECODED_SECONDS`], whichever is
/// smaller. Protects against decompression bombs: a few hundred kB of
/// crafted packets can otherwise expand to gigabytes of PCM.
pub fn decode_ogg_opus_bounded(bytes: &[u8], declared_secs: Option<u32>) -> Result<Pcm> {
    let mut reader = ogg::PacketReader::new(Cursor::new(bytes));
    let head = reader
        .read_packet()
        .map_err(media_err)?
        .ok_or_else(|| media_err("empty ogg stream"))?;
    if head.data.len() < 19 || &head.data[0..8] != b"OpusHead" {
        return Err(media_err("not an Opus stream"));
    }
    let channels = head.data[9].clamp(1, 2) as u16;
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as usize;
    let ch = if channels == 2 {
        opus::Channels::Stereo
    } else {
        opus::Channels::Mono
    };
    let max_secs = match declared_secs {
        // Declared length + 50 % + 10 s slack, never above the hard cap.
        Some(d) if d > 0 => (d as u64 * 3 / 2 + 10).min(MAX_DECODED_SECONDS),
        _ => MAX_DECODED_SECONDS,
    };
    let max_samples = (max_secs * OPUS_RATE as u64 * channels as u64) as usize;
    let mut decoder = opus::Decoder::new(OPUS_RATE, ch).map_err(media_err)?;
    let mut out: Vec<f32> = Vec::new();
    let mut buf = vec![0f32; 5760 * channels as usize];
    let mut seen_tags = false;
    let mut packets = 0usize;
    while let Some(pkt) = reader.read_packet().map_err(media_err)? {
        if !seen_tags && pkt.data.starts_with(b"OpusTags") {
            seen_tags = true;
            continue;
        }
        if pkt.data.is_empty() {
            continue;
        }
        packets += 1;
        if packets > MAX_PACKETS {
            return Err(media_err("voice note has too many packets"));
        }
        let n = decoder
            .decode_float(&pkt.data, &mut buf, false)
            .map_err(media_err)?;
        let n = n * channels as usize;
        if out.len() + n > max_samples {
            return Err(media_err(format!(
                "voice note decodes to more than {max_secs} s of audio"
            )));
        }
        out.extend_from_slice(&buf[..n]);
    }
    let skip = (pre_skip * channels as usize).min(out.len());
    out.drain(..skip);
    Ok(Pcm {
        samples: out,
        channels,
        rate: OPUS_RATE,
    })
}

/// Encodes mono 48 kHz PCM into an Ogg/Opus voice note.
pub fn encode_ogg_opus(mono_48k: &[f32]) -> Result<Vec<u8>> {
    let mut enc = opus::Encoder::new(OPUS_RATE, opus::Channels::Mono, opus::Application::Voip)
        .map_err(media_err)?;
    let _ = enc.set_bitrate(opus::Bitrate::Bits(24_000));
    let serial = 0x4645_5243; // "FERC"
    let mut out = Vec::new();
    {
        let mut w = ogg::PacketWriter::new(&mut out);
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&PRE_SKIP.to_le_bytes());
        head.extend_from_slice(&OPUS_RATE.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes()); // gain
        head.push(0); // mapping family
        w.write_packet(head, serial, ogg::PacketWriteEndInfo::EndPage, 0)
            .map_err(media_err)?;
        let vendor = b"ferrochat";
        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        w.write_packet(tags, serial, ogg::PacketWriteEndInfo::EndPage, 0)
            .map_err(media_err)?;

        // Prepend pre-skip worth of silence so the decoder warm-up eats it.
        let mut pcm = vec![0f32; PRE_SKIP as usize];
        pcm.extend_from_slice(mono_48k);
        let frames = pcm.len().div_ceil(FRAME).max(1);
        pcm.resize(frames * FRAME, 0.0);
        let mut granule: u64 = 0;
        let mut packet = vec![0u8; 4000];
        for (i, frame) in pcm.chunks(FRAME).enumerate() {
            let n = enc.encode_float(frame, &mut packet).map_err(media_err)?;
            granule += FRAME as u64;
            let end = if i + 1 == frames {
                ogg::PacketWriteEndInfo::EndStream
            } else if (i + 1) % 50 == 0 {
                ogg::PacketWriteEndInfo::EndPage
            } else {
                ogg::PacketWriteEndInfo::NormalPacket
            };
            w.write_packet(packet[..n].to_vec(), serial, end, granule)
                .map_err(media_err)?;
        }
    }
    Ok(out)
}

/// 64 amplitude buckets scaled to 0..=100, like WhatsApp's `waveform` field.
pub fn waveform(samples: &[f32]) -> Vec<u8> {
    const BUCKETS: usize = 64;
    if samples.is_empty() {
        return vec![0; BUCKETS];
    }
    let per = samples.len().div_ceil(BUCKETS).max(1);
    let rms: Vec<f32> = samples
        .chunks(per)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    let max = rms.iter().cloned().fold(0f32, f32::max).max(1e-6);
    let mut out: Vec<u8> = rms
        .iter()
        .map(|r| ((r / max) * 100.0).round().clamp(0.0, 100.0) as u8)
        .collect();
    out.resize(BUCKETS, 0);
    out
}

/// Linear resampling + down-mix to mono 48 kHz.
pub fn to_mono_48k(samples: &[f32], channels: u16, rate: u32) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect();
    if rate == OPUS_RATE || rate == 0 || mono.is_empty() {
        return mono;
    }
    let ratio = rate as f64 / OPUS_RATE as f64;
    let out_len = (mono.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = mono[idx.min(mono.len() - 1)];
            let b = mono[(idx + 1).min(mono.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

enum Cmd {
    Play(Pcm, Duration),
    Pause,
    Resume,
    Stop,
}

/// Shared playback position, polled by the UI tick.
#[derive(Debug, Default)]
pub struct PlaybackState {
    position_ms: AtomicU64,
    playing: AtomicBool,
    finished: AtomicBool,
}

impl PlaybackState {
    /// Current position.
    pub fn position_ms(&self) -> u64 {
        self.position_ms.load(Ordering::Relaxed)
    }
    /// Audio is being output.
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
    /// The last clip ran to its end.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }
}

/// Audio output on a dedicated thread (the output stream is not `Send` on
/// every platform). Opened lazily on first use.
#[derive(Clone)]
pub struct Player {
    tx: mpsc::Sender<Cmd>,
    state: Arc<PlaybackState>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Player")
    }
}

impl Player {
    /// Spawns the audio thread. Fails if no output device exists.
    pub fn spawn() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let state = Arc::new(PlaybackState::default());
        let st = state.clone();
        std::thread::Builder::new()
            .name("ferrochat-audio".into())
            .spawn(move || {
                let mut stream = match rodio::OutputStreamBuilder::open_default_stream() {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(media_err(e)));
                        return;
                    }
                };
                stream.log_on_drop(false);
                let _ = ready_tx.send(Ok(()));
                let mut sink: Option<rodio::Sink> = None;
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(Cmd::Play(pcm, start)) => {
                            if let Some(s) = sink.take() {
                                s.stop();
                            }
                            let s = rodio::Sink::connect_new(stream.mixer());
                            s.append(rodio::buffer::SamplesBuffer::new(
                                pcm.channels,
                                pcm.rate,
                                pcm.samples,
                            ));
                            if !start.is_zero() {
                                let _ = s.try_seek(start);
                            }
                            s.play();
                            st.finished.store(false, Ordering::Relaxed);
                            st.playing.store(true, Ordering::Relaxed);
                            sink = Some(s);
                        }
                        Ok(Cmd::Pause) => {
                            if let Some(s) = &sink {
                                s.pause();
                            }
                            st.playing.store(false, Ordering::Relaxed);
                        }
                        Ok(Cmd::Resume) => {
                            if let Some(s) = &sink {
                                s.play();
                                st.playing.store(true, Ordering::Relaxed);
                            }
                        }
                        Ok(Cmd::Stop) => {
                            if let Some(s) = sink.take() {
                                s.stop();
                            }
                            st.playing.store(false, Ordering::Relaxed);
                            st.position_ms.store(0, Ordering::Relaxed);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if let Some(s) = &sink {
                        st.position_ms
                            .store(s.get_pos().as_millis() as u64, Ordering::Relaxed);
                        if s.empty() {
                            sink = None;
                            st.playing.store(false, Ordering::Relaxed);
                            st.finished.store(true, Ordering::Relaxed);
                        }
                    }
                }
            })
            .map_err(Error::Io)?;
        ready_rx
            .recv()
            .map_err(|_| media_err("audio thread died"))??;
        Ok(Self { tx, state })
    }

    /// Starts playing `pcm` from `start`.
    pub fn play(&self, pcm: Pcm, start: Duration) {
        let _ = self.tx.send(Cmd::Play(pcm, start));
    }
    /// Pauses.
    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Pause);
    }
    /// Resumes after a pause.
    pub fn resume(&self) {
        let _ = self.tx.send(Cmd::Resume);
    }
    /// Stops and clears.
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    /// Shared position / state.
    pub fn state(&self) -> &PlaybackState {
        &self.state
    }
}

/// Longest voice note that can be recorded.
pub const MAX_RECORDING_SECS: u64 = 30 * 60;

#[derive(Default)]
struct Capture {
    samples: Vec<f32>,
    channels: u16,
    rate: u32,
    max_samples: usize,
    error: Option<String>,
    full: bool,
}

/// Microphone capture for voice notes. The `cpal` stream is not `Send`, so
/// it lives on its own thread; `start` returns immediately (no device I/O on
/// the UI thread) and failures show up in [`Recorder::error`].
pub struct Recorder {
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    cap: Arc<Mutex<Capture>>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Recorder")
    }
}

fn push_capped(cap: &Mutex<Capture>, data: impl Iterator<Item = f32>) {
    if let Ok(mut c) = cap.lock() {
        let room = c.max_samples.saturating_sub(c.samples.len());
        c.samples.extend(data.take(room));
        c.full = c.samples.len() >= c.max_samples;
    }
}

fn capture_thread(cap: Arc<Mutex<Capture>>, stop: mpsc::Receiver<()>) -> Result<()> {
    use rodio::DeviceTrait;
    use rodio::cpal::traits::{HostTrait, StreamTrait};
    let host = rodio::cpal::default_host();
    let dev = host
        .default_input_device()
        .ok_or_else(|| media_err("no microphone found"))?;
    let cfg = dev.default_input_config().map_err(media_err)?;
    if let Ok(mut c) = cap.lock() {
        c.channels = cfg.channels().max(1);
        c.rate = cfg.sample_rate().0;
        c.max_samples = (MAX_RECORDING_SECS * c.rate as u64 * c.channels as u64)
            .min(usize::MAX as u64) as usize;
    }
    let err_fn = |e| tracing::warn!(error = %e, "microphone stream error");
    let (c1, c2) = (cap.clone(), cap.clone());
    let stream = match cfg.sample_format() {
        rodio::cpal::SampleFormat::F32 => dev.build_input_stream(
            &cfg.into(),
            move |data: &[f32], _: &_| push_capped(&c1, data.iter().copied()),
            err_fn,
            None,
        ),
        rodio::cpal::SampleFormat::I16 => dev.build_input_stream(
            &cfg.into(),
            move |data: &[i16], _: &_| {
                push_capped(&c2, data.iter().map(|s| *s as f32 / i16::MAX as f32))
            },
            err_fn,
            None,
        ),
        other => return Err(media_err(format!("unsupported sample format {other:?}"))),
    }
    .map_err(media_err)?;
    stream.play().map_err(media_err)?;
    let _ = stop.recv();
    drop(stream);
    Ok(())
}

impl Recorder {
    /// Starts capturing from the default input device on a worker thread.
    pub fn start() -> Self {
        let (stop, stop_rx) = mpsc::channel();
        let cap = Arc::new(Mutex::new(Capture {
            max_samples: (MAX_RECORDING_SECS * OPUS_RATE as u64 * 2) as usize,
            ..Default::default()
        }));
        let c = cap.clone();
        let thread = std::thread::Builder::new()
            .name("ferrochat-mic".into())
            .spawn(move || {
                if let Err(e) = capture_thread(c.clone(), stop_rx)
                    && let Ok(mut g) = c.lock()
                {
                    g.error = Some(e.user_message());
                }
            })
            .ok();
        if thread.is_none()
            && let Ok(mut g) = cap.lock()
        {
            g.error = Some("could not start the recording thread".into());
        }
        Self { stop, thread, cap }
    }

    /// Error raised by the capture thread, if any.
    pub fn error(&self) -> Option<String> {
        self.cap.lock().ok().and_then(|c| c.error.clone())
    }

    /// The length cap was reached.
    pub fn is_full(&self) -> bool {
        self.cap.lock().is_ok_and(|c| c.full)
    }

    /// Stops capturing and returns mono 48 kHz samples. Blocking (joins the
    /// thread and resamples): call from `spawn_blocking`.
    pub fn finish(mut self) -> Vec<f32> {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let c = match self.cap.lock() {
            Ok(mut c) => std::mem::take(&mut *c),
            Err(_) => return Vec::new(),
        };
        to_mono_48k(&c.samples, c.channels.max(1), c.rate)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32) -> Vec<f32> {
        let n = (OPUS_RATE as f32 * seconds) as usize;
        (0..n)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / OPUS_RATE as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn opus_roundtrip_keeps_duration() {
        let pcm = tone(1.0);
        let ogg = encode_ogg_opus(&pcm).unwrap();
        assert!(ogg.starts_with(b"OggS"));
        let decoded = decode_ogg_opus(&ogg).unwrap();
        assert_eq!(decoded.channels, 1);
        let ms = decoded.duration_ms();
        assert!((990..=1030).contains(&ms), "duration {ms}");
        let energy: f32 = decoded.samples.iter().map(|s| s * s).sum();
        assert!(energy > 100.0);
    }

    /// Same construction as the reviewer's reproduction: tiny code-3 packets
    /// that each expand to 120 ms of audio.
    fn bomb(packets: u64) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut w = ogg::PacketWriter::new(&mut out);
            let mut head = b"OpusHead".to_vec();
            head.extend_from_slice(&[1, 1]);
            head.extend_from_slice(&312u16.to_le_bytes());
            head.extend_from_slice(&48_000u32.to_le_bytes());
            head.extend_from_slice(&[0, 0, 0]);
            w.write_packet(head, 1, ogg::PacketWriteEndInfo::EndPage, 0)
                .unwrap();
            let mut tags = b"OpusTags".to_vec();
            tags.extend_from_slice(&[0; 8]);
            w.write_packet(tags, 1, ogg::PacketWriteEndInfo::EndPage, 0)
                .unwrap();
            for i in 0..packets {
                let end = if i + 1 == packets {
                    ogg::PacketWriteEndInfo::EndStream
                } else {
                    ogg::PacketWriteEndInfo::NormalPacket
                };
                w.write_packet(vec![0x7B, 0x02], 1, end, (i + 1) * 5760)
                    .unwrap();
            }
        }
        out
    }

    #[test]
    fn decompression_bomb_is_rejected() {
        // 20 000 tiny packets expand to minutes of audio from ~200 kB of input.
        let b = bomb(20_000);
        assert!(b.len() < 400_000);
        assert!(decode_ogg_opus(&b).is_err());
        // A declared 9 s note may not expand to a minute.
        assert!(decode_ogg_opus_bounded(&bomb(1500), Some(9)).is_err());
        // Within bounds it still decodes.
        assert!(decode_ogg_opus_bounded(&bomb(50), Some(9)).is_ok());
    }

    #[test]
    fn capture_buffer_is_capped() {
        let cap = Mutex::new(Capture {
            max_samples: 10,
            ..Default::default()
        });
        push_capped(&cap, std::iter::repeat_n(0.1, 7));
        push_capped(&cap, std::iter::repeat_n(0.1, 7));
        let c = cap.lock().unwrap();
        assert_eq!(c.samples.len(), 10);
        assert!(c.full);
    }

    #[test]
    fn rejects_non_opus() {
        assert!(decode_ogg_opus(b"not ogg at all").is_err());
    }

    #[test]
    fn waveform_has_64_buckets() {
        let w = waveform(&tone(0.5));
        assert_eq!(w.len(), 64);
        assert!(w.iter().all(|v| *v <= 100));
        assert_eq!(waveform(&[]), vec![0; 64]);
    }

    #[test]
    fn resample_halves_length_from_96k() {
        let s = vec![0.5f32; 9600 * 2];
        let out = to_mono_48k(&s, 2, 96_000);
        assert_eq!(out.len(), 4800);
        assert!((out[10] - 0.5).abs() < 1e-6);
    }
}
