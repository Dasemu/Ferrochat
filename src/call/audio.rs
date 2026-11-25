//! Call audio: microphone and speaker as 60 ms / 16 kHz mono frames (what
//! the library's VoIP facade takes), and a synthesized ringtone.
//!
//! `cpal` streams are not `Send`, so each device lives on its own thread;
//! opening never blocks the UI thread. Dropping [`Devices`] / [`Ringer`]
//! stops the threads and releases the devices. Errors are logged; a missing
//! microphone does not end the call (the peer hears silence).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::net::CallAudio;
use crate::net::call::{FRAME_SAMPLES, SAMPLE_RATE};

/// Received audio buffered for the speaker at most (drops the oldest).
const MAX_PLAYOUT_MS: usize = 400;
/// Frames queued towards the call engine.
const QUEUE: usize = 6;

/// Linear resampler for one mono stream (good enough for speech; the
/// library's codec runs at 16 kHz anyway).
#[derive(Debug, Clone)]
pub struct Resampler {
    step: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    /// From `from` Hz to `to` Hz.
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            step: f64::from(from.max(1)) / f64::from(to.max(1)),
            pos: 0.0,
            last: 0.0,
        }
    }

    /// Resamples `input`, appending to `out`. Keeps state across calls.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        // `pos` indexes into [last, input...]: 0 is the previous sample.
        let n = input.len() as f64;
        while self.pos < n {
            let i = self.pos.floor();
            let frac = (self.pos - i) as f32;
            let i = i as usize;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i.min(input.len() - 1)];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= n;
        if let Some(l) = input.last() {
            self.last = *l;
        }
    }
}

/// Down-mixes interleaved samples to mono.
pub fn to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    let ch = channels.max(1);
    data.chunks(ch)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect()
}

/// f32 [-1, 1] to i16.
pub fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

/// The open microphone and speaker of a call.
pub struct Devices {
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Devices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Devices")
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Devices {
    /// Opens the default microphone and speaker on worker threads.
    pub fn open() -> (CallAudio, Devices) {
        let stop = Arc::new(AtomicBool::new(false));
        let (mic_tx, mic) = async_channel::bounded::<Vec<i16>>(QUEUE);
        let (speaker, spk_rx) = async_channel::bounded::<Vec<i16>>(QUEUE);
        let s1 = stop.clone();
        let _ = std::thread::Builder::new()
            .name("ferrochat-call-mic".into())
            .spawn(move || {
                if let Err(e) = mic_thread(mic_tx, &s1) {
                    tracing::warn!(error = %e, "call microphone unavailable");
                }
            });
        let s2 = stop.clone();
        let _ = std::thread::Builder::new()
            .name("ferrochat-call-speaker".into())
            .spawn(move || {
                if let Err(e) = speaker_thread(spk_rx, &s2) {
                    tracing::warn!(error = %e, "call speaker unavailable");
                }
            });
        (CallAudio { mic, speaker }, Devices { stop })
    }
}

fn wait_for(stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Splits mono 16 kHz audio into library frames.
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<i16>,
}

impl Framer {
    /// Adds samples; returns the complete frames.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<i16>> {
        self.buf.extend(samples.iter().map(|s| to_i16(*s)));
        let mut frames = Vec::new();
        while self.buf.len() >= FRAME_SAMPLES {
            let rest = self.buf.split_off(FRAME_SAMPLES);
            frames.push(std::mem::replace(&mut self.buf, rest));
        }
        frames
    }
}

fn mic_thread(tx: async_channel::Sender<Vec<i16>>, stop: &AtomicBool) -> Result<(), String> {
    use rodio::DeviceTrait;
    use rodio::cpal::traits::{HostTrait, StreamTrait};
    let host = rodio::cpal::default_host();
    let dev = host.default_input_device().ok_or("no microphone found")?;
    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
    let channels = usize::from(cfg.channels().max(1));
    let rate = cfg.sample_rate().0;
    let state = Arc::new(Mutex::new((
        Resampler::new(rate, SAMPLE_RATE),
        Framer::default(),
    )));
    let feed = move |data: &[f32]| {
        let mono = to_mono(data, channels);
        let mut out = Vec::with_capacity(mono.len());
        if let Ok(mut g) = state.lock() {
            let (rs, fr) = &mut *g;
            rs.process(&mono, &mut out);
            for frame in fr.push(&out) {
                // Full queue: the engine is behind; drop (VoIP is loss tolerant).
                let _ = tx.try_send(frame);
            }
        }
    };
    let err_fn = |e| tracing::warn!(error = %e, "call microphone stream error");
    let stream = match cfg.sample_format() {
        rodio::cpal::SampleFormat::F32 => dev.build_input_stream(
            &cfg.into(),
            move |data: &[f32], _: &_| feed(data),
            err_fn,
            None,
        ),
        rodio::cpal::SampleFormat::I16 => dev.build_input_stream(
            &cfg.into(),
            move |data: &[i16], _: &_| {
                let f: Vec<f32> = data
                    .iter()
                    .map(|s| f32::from(*s) / f32::from(i16::MAX))
                    .collect();
                feed(&f)
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    wait_for(stop);
    Ok(())
}

fn speaker_thread(rx: async_channel::Receiver<Vec<i16>>, stop: &AtomicBool) -> Result<(), String> {
    use rodio::DeviceTrait;
    use rodio::cpal::traits::{HostTrait, StreamTrait};
    let host = rodio::cpal::default_host();
    let dev = host.default_output_device().ok_or("no speaker found")?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let channels = usize::from(cfg.channels().max(1));
    let rate = cfg.sample_rate().0;
    let cap = rate as usize * MAX_PLAYOUT_MS / 1000;
    let buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(cap)));
    let b1 = buf.clone();
    let pull = move |out: &mut dyn FnMut(usize, f32), frames: usize| {
        if let Ok(mut q) = b1.lock() {
            for i in 0..frames {
                out(i, q.pop_front().unwrap_or(0.0));
            }
        }
    };
    let err_fn = |e| tracing::warn!(error = %e, "call speaker stream error");
    let stream = match cfg.sample_format() {
        rodio::cpal::SampleFormat::F32 => dev.build_output_stream(
            &cfg.into(),
            move |data: &mut [f32], _: &_| {
                let frames = data.len() / channels;
                pull(
                    &mut |i, s| data[i * channels..(i + 1) * channels].fill(s),
                    frames,
                );
            },
            err_fn,
            None,
        ),
        rodio::cpal::SampleFormat::I16 => dev.build_output_stream(
            &cfg.into(),
            move |data: &mut [i16], _: &_| {
                let frames = data.len() / channels;
                pull(
                    &mut |i, s| data[i * channels..(i + 1) * channels].fill(to_i16(s)),
                    frames,
                );
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    let mut rs = Resampler::new(SAMPLE_RATE, rate);
    let mut out = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        match rx.try_recv() {
            Ok(frame) => {
                let f: Vec<f32> = frame
                    .iter()
                    .map(|s| f32::from(*s) / f32::from(i16::MAX))
                    .collect();
                out.clear();
                rs.process(&f, &mut out);
                if let Ok(mut q) = buf.lock() {
                    q.extend(out.iter().copied());
                    while q.len() > cap {
                        q.pop_front();
                    }
                }
            }
            Err(async_channel::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(async_channel::TryRecvError::Closed) => wait_for(stop),
        }
    }
    Ok(())
}

/// Which tone to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Incoming call ringtone.
    Ring,
    /// What the caller hears while the peer's phone rings.
    Ringback,
}

/// One cycle of a tone at `rate` Hz, mono: a short melody of sine bursts
/// then silence (synthesized; no recorded ringtone is shipped).
pub fn tone_cycle(tone: Tone, rate: u32) -> Vec<f32> {
    let rate_f = rate as f32;
    let mut out = Vec::new();
    let mut burst = |freqs: &[f32], ms: u32, gain: f32| {
        let n = (rate * ms / 1000) as usize;
        for i in 0..n {
            let t = i as f32 / rate_f;
            // 10 ms fade in/out avoids clicks.
            let edge = (i.min(n - i) as f32 / (rate_f * 0.01)).min(1.0);
            let s: f32 = freqs
                .iter()
                .map(|f| (2.0 * std::f32::consts::PI * f * t).sin())
                .sum::<f32>()
                / freqs.len() as f32;
            out.push(s * gain * edge);
        }
    };
    match tone {
        Tone::Ring => {
            for f in [659.25, 783.99, 987.77, 783.99] {
                burst(&[f], 140, 0.35);
                burst(&[0.0], 30, 0.0);
            }
            burst(&[0.0], 1600, 0.0);
        }
        Tone::Ringback => {
            burst(&[440.0, 480.0], 1200, 0.18);
            burst(&[0.0], 2800, 0.0);
        }
    }
    out
}

/// A looping tone; stops when dropped.
pub struct Ringer {
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Ringer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ringer")
    }
}

impl Drop for Ringer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Ringer {
    /// Starts playing `tone` in a loop on the default output.
    pub fn start(tone: Tone) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let _ = std::thread::Builder::new()
            .name("ferrochat-ringer".into())
            .spawn(move || {
                let mut stream = match rodio::OutputStreamBuilder::open_default_stream() {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "no audio output for the ringtone");
                        return;
                    }
                };
                stream.log_on_drop(false);
                let rate = 48_000;
                let cycle = tone_cycle(tone, rate);
                let secs = cycle.len() as f32 / rate as f32;
                let sink = rodio::Sink::connect_new(stream.mixer());
                while !s.load(Ordering::SeqCst) {
                    if sink.len() < 2 {
                        sink.append(rodio::buffer::SamplesBuffer::new(1, rate, cycle.clone()));
                    }
                    std::thread::sleep(Duration::from_secs_f32((secs / 4.0).min(0.2)));
                }
                sink.stop();
            });
        Self { stop }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_the_rate_ratio_across_chunks() {
        let mut rs = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for _ in 0..10 {
            rs.process(&[0.5; 480], &mut out);
        }
        assert_eq!(out.len(), 1600);
        let mut up = Resampler::new(16_000, 48_000);
        let mut out2 = Vec::new();
        up.process(&[0.25; 960], &mut out2);
        assert_eq!(out2.len(), 2880);
        assert!(out2[10..].iter().all(|s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn framer_emits_whole_library_frames() {
        let mut f = Framer::default();
        assert!(f.push(&[0.0; 500]).is_empty());
        let frames = f.push(&[1.0; 1500]);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|fr| fr.len() == FRAME_SAMPLES));
        assert_eq!(frames[1][0], i16::MAX);
    }

    #[test]
    fn mono_and_clamping() {
        assert_eq!(to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), -i16::MAX);
    }

    #[test]
    fn tones_are_bounded_and_loop_sized() {
        for t in [Tone::Ring, Tone::Ringback] {
            let c = tone_cycle(t, 8_000);
            assert!(c.len() > 8_000);
            assert!(c.iter().all(|s| s.abs() <= 1.0));
        }
    }
}
