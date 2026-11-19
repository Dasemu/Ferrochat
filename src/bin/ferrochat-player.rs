//! Video decoder helper for Ferrochat (GStreamer).
//!
//! `ferrochat-player <file>` plays one video file. It writes RGBA frames and
//! playback times to stdout and reads commands from stdin; the protocol is in
//! `ferrochat::media::video_proto`. The app spawns one helper per open video
//! and ends it when the viewer closes, so a decoder never outlives its video
//! and GStreamer is only loaded here, never in the main process.

#[cfg(unix)]
fn main() {
    player::run();
}

#[cfg(not(unix))]
fn main() {
    eprintln!("ferrochat-player: only supported on Unix");
    std::process::exit(2);
}

#[cfg(unix)]
mod player {
    use std::io::{self, BufRead, BufWriter};
    use std::os::fd::AsFd;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use ferrochat::media::video_proto::{self as proto, Cmd, Msg, PlayState};
    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer_app as gst_app;
    use gstreamer_video as gst_video;

    /// Widest / tallest frame sent to the app (aspect ratio is kept). The
    /// viewer scales it up; decoding and piping more would only cost RAM.
    const MAX_W: i32 = 960;
    const MAX_H: i32 = 540;
    /// How often the position is reported.
    const TIME_EVERY: Duration = Duration::from_millis(250);

    type Out = Arc<Mutex<BufWriter<std::fs::File>>>;

    /// Size of the first decoded frame, packed `width << 32 | height`.
    static FIRST_SIZE: AtomicU64 = AtomicU64::new(0);

    pub fn run() {
        let Some(path) = std::env::args().nth(1) else {
            eprintln!("usage: ferrochat-player <file>");
            std::process::exit(2);
        };
        // stdout as a plain file: no line buffering for binary frames.
        let out_file = match io::stdout()
            .as_fd()
            .try_clone_to_owned()
            .map(std::fs::File::from)
        {
            Ok(f) => f,
            Err(_) => std::process::exit(2),
        };
        let out: Out = Arc::new(Mutex::new(BufWriter::with_capacity(1 << 20, out_file)));
        if let Err(failure) = play(&path, &out) {
            send(&out, &failure);
            std::process::exit(1);
        }
    }

    fn send(out: &Out, m: &Msg) {
        let Ok(mut w) = out.lock() else { return };
        if proto::write_msg(&mut *w, m).is_err() {
            // The app is gone (broken pipe): nothing left to play for.
            std::process::exit(0);
        }
    }

    fn fail(text: impl Into<String>) -> Msg {
        Msg::Failed {
            missing: false,
            text: text.into(),
        }
    }

    fn play(path: &str, out: &Out) -> Result<(), Msg> {
        gst::init().map_err(|e| fail(format!("GStreamer could not start: {e}")))?;

        let abs = std::fs::canonicalize(path).map_err(|_| fail("the video file is missing"))?;
        let uri = gst::glib::filename_to_uri(&abs, None::<&str>)
            .map_err(|_| fail("the video path is not valid"))?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", uri.as_str())
            .build()
            .map_err(|_| Msg::Failed {
                missing: true,
                text: "the GStreamer playbin element (gst-plugins-base)".into(),
            })?;
        // Video and audio only: embedded subtitles must not be burnt into
        // the picture, and `soft-volume` makes the volume property work.
        playbin.set_property_from_str("flags", "video+audio+soft-volume");

        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .field("width", gst::IntRange::new(1, MAX_W))
            .field("height", gst::IntRange::new(1, MAX_H))
            .build();
        let frames = out.clone();
        let preroll = out.clone();
        let sink = gst_app::AppSink::builder()
            .caps(&caps)
            .max_buffers(1)
            .drop(true)
            .sync(true)
            .callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_preroll(move |s| {
                        let sample = s.pull_preroll().map_err(|_| gst::FlowError::Error)?;
                        emit_frame(&preroll, &sample);
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .new_sample(move |s| {
                        let sample = s.pull_sample().map_err(|_| gst::FlowError::Error)?;
                        emit_frame(&frames, &sample);
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            )
            .build();
        playbin.set_property("video-sink", &sink);
        // No sound card (CI, containers): play silently instead of failing.
        if gst::ElementFactory::find("autoaudiosink").is_none()
            && let Ok(fake) = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
        {
            playbin.set_property("audio-sink", &fake);
        }

        let bus = playbin.bus().ok_or_else(|| fail("no message bus"))?;
        playbin
            .set_state(gst::State::Paused)
            .map_err(|_| fail("the video could not be opened"))?;

        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                match line {
                    Ok(l) => {
                        if let Some(c) = Cmd::parse(&l)
                            && tx.send(c).is_err()
                        {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            // stdin closed: the app is gone.
            let _ = tx.send(Cmd::Quit);
        });

        let mut ready_sent = false;
        let mut want_play = true;
        let mut ended = false;
        let mut volume = 1.0f64;
        let started = Instant::now();
        let mut last_time = Instant::now() - TIME_EVERY;

        loop {
            // Commands first: seeks and pauses should feel immediate.
            match rx.recv_timeout(Duration::from_millis(15)) {
                Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => {
                    let _ = playbin.set_state(gst::State::Null);
                    return Ok(());
                }
                Ok(Cmd::Play) => {
                    want_play = true;
                    if ended {
                        ended = false;
                        let _ = playbin.seek_simple(
                            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                            gst::ClockTime::ZERO,
                        );
                    }
                    if ready_sent {
                        let _ = playbin.set_state(gst::State::Playing);
                    }
                }
                Ok(Cmd::Pause) => {
                    want_play = false;
                    let _ = playbin.set_state(gst::State::Paused);
                }
                Ok(Cmd::Seek(ms)) => {
                    ended = false;
                    let _ = playbin.seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                        gst::ClockTime::from_mseconds(ms),
                    );
                }
                Ok(Cmd::Volume(v)) => {
                    volume = f64::from(v) / 100.0;
                    playbin.set_property("volume", volume);
                }
                Ok(Cmd::Mute(m)) => playbin.set_property("mute", m),
                Err(RecvTimeoutError::Timeout) => {}
            }

            while let Some(msg) = bus.pop() {
                use gst::MessageView as V;
                match msg.view() {
                    V::Error(e) => {
                        let text = e.error().to_string();
                        let missing = e.error().matches(gst::StreamError::CodecNotFound)
                            || e.error().matches(gst::CoreError::MissingPlugin);
                        return Err(Msg::Failed { missing, text });
                    }
                    V::Eos(_) => {
                        ended = true;
                        let _ = playbin.set_state(gst::State::Paused);
                    }
                    V::Element(el) => {
                        if let Some(s) = el.structure()
                            && s.name() == "missing-plugin"
                        {
                            let what = s
                                .get::<String>("name")
                                .unwrap_or_else(|_| "a decoder".into());
                            return Err(Msg::Failed {
                                missing: true,
                                text: what,
                            });
                        }
                    }
                    _ => {}
                }
            }

            let size = FIRST_SIZE.load(Ordering::Relaxed);
            let no_video = !ready_sent && size == 0 && started.elapsed() > Duration::from_secs(4);
            if !ready_sent && (size != 0 || no_video) {
                ready_sent = true;
                let dur_q = playbin.query_duration::<gst::ClockTime>();
                if no_video && dur_q.is_none() {
                    // Nothing decodable and no known length: not a media file.
                    return Err(fail("the file has no playable video or audio"));
                }
                let dur = dur_q.map_or(0, gst::ClockTime::mseconds);
                send(
                    out,
                    &Msg::Ready {
                        width: (size >> 32) as u32,
                        height: (size & 0xffff_ffff) as u32,
                        duration_ms: dur,
                    },
                );
                playbin.set_property("volume", volume);
                if want_play {
                    let _ = playbin.set_state(gst::State::Playing);
                }
            }

            if ready_sent && last_time.elapsed() >= TIME_EVERY {
                last_time = Instant::now();
                let pos = playbin
                    .query_position::<gst::ClockTime>()
                    .map_or(0, gst::ClockTime::mseconds);
                let dur = playbin
                    .query_duration::<gst::ClockTime>()
                    .map_or(0, gst::ClockTime::mseconds);
                let state = if ended {
                    PlayState::Ended
                } else if want_play {
                    PlayState::Playing
                } else {
                    PlayState::Paused
                };
                send(
                    out,
                    &Msg::Time {
                        position_ms: if ended { dur } else { pos },
                        duration_ms: dur,
                        state,
                    },
                );
            }
        }
    }

    /// Copies a sample's RGBA pixels (dropping row padding) and sends them.
    fn emit_frame(out: &Out, sample: &gst::Sample) {
        let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
            return;
        };
        let Ok(info) = gst_video::VideoInfo::from_caps(caps) else {
            return;
        };
        let (w, h) = (info.width(), info.height());
        if w == 0 || h == 0 || w > proto::MAX_DIM || h > proto::MAX_DIM {
            return;
        }
        let Ok(frame) = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info) else {
            return;
        };
        let Ok(plane) = frame.plane_data(0) else {
            return;
        };
        let stride = info.stride()[0].max(0) as usize;
        let row = w as usize * 4;
        let rgba: Vec<u8> = if stride == row {
            plane
                .get(..row * h as usize)
                .map(<[u8]>::to_vec)
                .unwrap_or_default()
        } else {
            let mut v = Vec::with_capacity(row * h as usize);
            for y in 0..h as usize {
                match plane.get(y * stride..y * stride + row) {
                    Some(r) => v.extend_from_slice(r),
                    None => return,
                }
            }
            v
        };
        if rgba.len() != row * h as usize {
            return;
        }
        let _ = FIRST_SIZE.compare_exchange(
            0,
            (u64::from(w) << 32) | u64::from(h),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        let Ok(mut wr) = out.lock() else { return };
        if proto::write_frame(&mut *wr, w, h, &rgba).is_err() {
            std::process::exit(0);
        }
    }
}
