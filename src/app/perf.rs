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
    /// Rasterizing the whole frame with tiny-skia (worst case: no damage
    /// tracking, as when the whole conversation moves).
    pub raster: Duration,
    /// `App::update` for the produced messages.
    pub app: Duration,
    /// Messages in the open window at the end.
    pub messages: usize,
    /// Scroll messages the wheel produced.
    pub scroll_events: usize,
}

impl Profile {
    fn per_frame(&self, d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0 / f64::from(self.frames.max(1))
    }

    /// One line per phase, ms per frame.
    pub fn report(&self) -> String {
        format!(
            "frames {} ({} messages open, {} scroll events): view {:.2} ms, layout {:.2} ms, events {:.2} ms, draw {:.2} ms, raster {:.2} ms, app {:.2} ms, total {:.2} ms/frame",
            self.frames,
            self.messages,
            self.scroll_events,
            self.per_frame(self.view),
            self.per_frame(self.layout),
            self.per_frame(self.events),
            self.per_frame(self.draw),
            self.per_frame(self.raster),
            self.per_frame(self.app),
            self.per_frame(
                self.view + self.layout + self.events + self.draw + self.raster + self.app
            ),
        )
    }
}

/// Opens the showcase chat with `extra` older messages in the cache and
/// scrolls up through it with the mouse wheel for `frames` frames.
pub async fn profile_scroll(extra: usize, frames: u32, raster: bool) -> Profile {
    let mut h = Harness::new().await;
    let older = long_history(SHOWCASE_CHAT, extra, 1_600_000_000);
    h.app.store.insert_messages(&older).expect("store");
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let loaded = h.app.open.as_ref().map_or(0, |c| c.messages.len());
    assert!(loaded >= 50, "only {loaded} messages open");
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
    let mut scrolled = 0usize;
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
        if raster {
            let t = Instant::now();
            let px = renderer.screenshot(iced::Size::new(1280, 800), 1.0, iced::Color::BLACK);
            std::hint::black_box(px);
            p.raster += t.elapsed();
        }
        cache = ui.into_cache();
        let t = Instant::now();
        scrolled += msgs
            .iter()
            .filter(|m| matches!(m, Message::Ui(Ui::Scrolled { .. })))
            .count();
        for m in msgs {
            let task = h.app.update(m);
            h.pump(task).await;
        }
        p.app += t.elapsed();
        p.frames += 1;
    }
    p.messages = h.app.open.as_ref().map_or(0, |c| c.messages.len());
    p.scroll_events = scrolled;
    p
}

#[tokio::test]
#[ignore = "profiling; prints numbers"]
async fn scroll_profile() {
    let p = profile_scroll(600, 120, true).await;
    println!("scroll profile (600 cached messages): {}", p.report());
}

#[tokio::test]
async fn scrolling_a_long_chat_stays_within_a_frame_budget() {
    // Generous bound for debug builds on a shared CI machine; the release
    // numbers are in the PR. Guards against per-frame work growing with the
    // chat length again.
    let p = profile_scroll(600, 40, false).await;
    let per_frame = (p.view + p.layout + p.events + p.draw).as_secs_f64() * 1000.0 / 40.0;
    println!("{}", p.report());
    assert!(per_frame < 250.0, "{}", p.report());
}

/// Full-frame raster time of the current view, ms (best of `n`).
fn raster_ms(app: &super::App, n: u32) -> f64 {
    raster_el_ms(app, n, Size::new(1280.0, 800.0), crate::ui::view)
}

/// Raster time of one element at `size`.
fn raster_el_ms(
    app: &super::App,
    n: u32,
    size: Size,
    el: impl Fn(&super::App) -> crate::ui::El<'_>,
) -> f64 {
    let mut renderer = futures::executor::block_on(<iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        Pixels(16.0),
        Some("tiny-skia"),
    ))
    .expect("tiny-skia renderer");
    let theme = app.iced_theme(iced::window::Id::unique());
    let mut cache = Cache::default();
    // Minimum, not mean: the build machine is shared and noisy.
    let mut best = Duration::MAX;
    for i in 0..=n {
        let mut ui = UserInterface::build(el(app), size, cache, &mut renderer);
        ui.draw(
            &mut renderer,
            &theme,
            &Style::default(),
            Cursor::Unavailable,
        );
        cache = ui.into_cache();
        let t = Instant::now();
        std::hint::black_box(renderer.screenshot(
            iced::Size::new(size.width as u32, size.height as u32),
            1.0,
            iced::Color::BLACK,
        ));
        if i > 0 {
            best = best.min(t.elapsed());
        }
    }
    best.as_secs_f64() * 1000.0
}

#[tokio::test]
#[ignore = "profiling; prints numbers"]
async fn raster_breakdown() {
    let mut h = Harness::new().await;
    println!("raster: chat list only {:.1} ms", raster_ms(&h.app, 25));
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    println!("raster: showcase chat open {:.1} ms", raster_ms(&h.app, 25));
    if let Some(conv) = h.app.open.clone() {
        let ms = raster_el_ms(&h.app, 25, Size::new(796.0, 800.0), move |a| {
            crate::ui::chat::view(a, Box::leak(Box::new(conv.clone())))
        });
        println!("raster: conversation pane only {ms:.1} ms");
        let images = std::mem::replace(&mut h.app.images, crate::media::ByteLru::new(1 << 20));
        let thumbs = std::mem::take(&mut h.app.thumbs);
        let conv2 = h.app.open.clone();
        let ms = raster_el_ms(&h.app, 25, Size::new(796.0, 800.0), move |a| {
            crate::ui::chat::view(a, Box::leak(Box::new(conv2.clone().unwrap_or_default())))
        });
        println!("raster: conversation pane, no pictures {ms:.1} ms");
        h.app.images = images;
        h.app.thumbs = thumbs;
        let mut empty = h.app.open.clone().unwrap_or_default();
        empty.messages.clear();
        let ms = raster_el_ms(&h.app, 25, Size::new(796.0, 800.0), move |a| {
            crate::ui::chat::view(a, Box::leak(Box::new(empty.clone())))
        });
        println!("raster: conversation pane, no messages {ms:.1} ms");
        let ms = raster_el_ms(&h.app, 25, Size::new(796.0, 800.0), |_| {
            iced::widget::container(iced::widget::text("x"))
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .style(crate::ui::theme::solid(iced::Color::from_rgb(
                    0.1, 0.1, 0.1,
                )))
                .into()
        });
        println!("raster: one solid 796x800 quad {ms:.1} ms");
    }
    let avatars = std::mem::replace(&mut h.app.avatars, crate::media::ByteLru::new(1 << 20));
    println!(
        "raster: without avatar pictures {:.1} ms",
        raster_ms(&h.app, 25)
    );
    h.app.avatars = avatars;
}
