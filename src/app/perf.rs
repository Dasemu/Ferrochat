//! Scroll profiling: the real `App`, the real view and iced's
//! `UserInterface` with the headless tiny-skia renderer. Run with
//! `cargo test --release scroll_profile -- --ignored --nocapture`
//! (debug numbers are much higher but compare the same way).

use std::time::{Duration, Instant};

use iced::advanced::clipboard;
use iced::advanced::renderer::{Headless, Style};
use iced::mouse::{Cursor, ScrollDelta};
use iced::{Event, Pixels, Point, Size};
use iced_runtime::user_interface::{Cache, UserInterface};

use super::test_support::Harness;
use super::{Message, Ui};
use crate::mock::{SHOWCASE_CHAT, long_history};

/// Per-phase totals over a scroll run.
#[derive(Debug, Default)]
pub struct Profile {
    /// Frames.
    pub frames: u32,
    /// Building the element tree (`view`).
    pub view: Duration,
    /// Diff + layout (`UserInterface::build`).
    pub layout: Duration,
    /// Event handling (`UserInterface::update`).
    pub events: Duration,
    /// Drawing (primitive recording).
    pub draw: Duration,
    /// `App::update` for the produced messages.
    pub app: Duration,
}

impl Profile {
    fn per_frame(&self, d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0 / f64::from(self.frames.max(1))
    }

    /// One line per phase, ms per frame.
    pub fn report(&self) -> String {
        format!(
            "frames {}: view {:.2} ms, layout {:.2} ms, events {:.2} ms, draw {:.2} ms, app {:.2} ms, total {:.2} ms/frame",
            self.frames,
            self.per_frame(self.view),
            self.per_frame(self.layout),
            self.per_frame(self.events),
            self.per_frame(self.draw),
            self.per_frame(self.app),
            self.per_frame(self.view + self.layout + self.events + self.draw + self.app),
        )
    }
}

/// Opens the showcase chat with `extra` older messages in the cache and
/// scrolls up through it with the mouse wheel for `frames` frames.
pub async fn profile_scroll(extra: usize, frames: u32) -> Profile {
    let mut h = Harness::new().await;
    let older = long_history(SHOWCASE_CHAT, extra, 1_600_000_000);
    h.app.store.insert_messages(&older).expect("store");
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let mut renderer = futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        Pixels(16.0),
        Some("tiny-skia"),
    ))
    .expect("tiny-skia renderer");
    let theme = h.app.iced_theme(iced::window::Id::unique());
    let size = Size::new(1280.0, 800.0);
    let cursor = Cursor::Available(Point::new(900.0, 400.0));
    let mut cache = Cache::default();
    let mut p = Profile::default();
    for _ in 0..frames {
        let t = Instant::now();
        let el = crate::ui::view(&h.app);
        p.view += t.elapsed();
        let t = Instant::now();
        let mut ui = UserInterface::build(el, size, cache, &mut renderer);
        p.layout += t.elapsed();
        let t = Instant::now();
        let mut msgs: Vec<Message> = Vec::new();
        let wheel = Event::Mouse(iced::mouse::Event::WheelScrolled {
            delta: ScrollDelta::Pixels { x: 0.0, y: 60.0 },
        });
        let _ = ui.update(
            &[wheel],
            cursor,
            &mut renderer,
            &mut clipboard::Null,
            &mut msgs,
        );
        p.events += t.elapsed();
        let t = Instant::now();
        ui.draw(&mut renderer, &theme, &Style::default(), cursor);
        p.draw += t.elapsed();
        cache = ui.into_cache();
        let t = Instant::now();
        for m in msgs {
            let task = h.app.update(m);
            h.pump(task).await;
        }
        p.app += t.elapsed();
        p.frames += 1;
    }
    p
}

#[tokio::test]
#[ignore = "profiling; prints numbers"]
async fn scroll_profile() {
    let p = profile_scroll(600, 300).await;
    println!("scroll profile (600 cached messages): {}", p.report());
}

#[tokio::test]
async fn scrolling_a_long_chat_stays_within_a_frame_budget() {
    // Generous bound for debug builds on a shared CI machine; the release
    // numbers are in the PR. Guards against per-frame work growing with the
    // chat length again.
    let p = profile_scroll(600, 40).await;
    let per_frame = (p.view + p.layout + p.events + p.draw).as_secs_f64() * 1000.0 / 40.0;
    println!("{}", p.report());
    assert!(per_frame < 250.0, "{}", p.report());
}
