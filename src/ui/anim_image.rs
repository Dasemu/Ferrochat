//! A widget that plays an [`Anim`] by itself (ported from Ferrocord).
//!
//! The app never ticks for it: on every redraw the widget records the
//! frame time and, while it is playing and inside the visible viewport,
//! asks the runtime for the next redraw exactly when its frame changes
//! (`Shell::request_redraw_at`). No `Message` is produced, so `view()` and
//! layout do not run per frame, and tiny-skia's damage tracking repaints
//! only the image's rectangle. Off-screen or paused images request nothing,
//! so an idle (unfocused or hidden) window costs no CPU.
//!
//! Redraw times are rounded up to a 60 Hz grid shared by every image
//! ([`FRAME_SLOT`] from one process-wide origin), so any number of
//! animations with unrelated frame delays cost at most 60 window redraws a
//! second, each frame still landing on its own delay.

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::advanced::image as adv_image;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::widget::image::Handle;
use iced::{ContentFit, Element, Event, Length, Rectangle, Size, mouse, window};

use crate::app::anim::Anim;

/// Redraws are aligned to this grid (one 60 Hz display frame).
pub const FRAME_SLOT: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// An animated image of a fixed size.
pub struct AnimImage {
    anim: Arc<Anim>,
    epoch: Instant,
    playing: bool,
    width: f32,
    height: f32,
    fit: ContentFit,
    radius: f32,
}

/// `anim` drawn in a `width` x `height` box. All animations share the
/// app's `epoch` so they stay in step across view rebuilds.
pub fn anim_image(
    anim: Arc<Anim>,
    epoch: Instant,
    playing: bool,
    width: f32,
    height: f32,
) -> AnimImage {
    AnimImage {
        anim,
        epoch,
        playing,
        width,
        height,
        fit: ContentFit::Fill,
        radius: 0.0,
    }
}

impl AnimImage {
    /// How the frames fit the box (default: fill).
    pub fn content_fit(mut self, fit: ContentFit) -> Self {
        self.fit = fit;
        self
    }

    /// Corner radius (ignored by tiny-skia, used by wgpu).
    pub fn border_radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }
}

#[derive(Debug, Default)]
struct State {
    /// Time of the last redraw: the frame shown is picked from it.
    now: Option<Instant>,
}

/// How many self-playing animation widgets the widget tree `tree` holds.
pub fn count_in(tree: &Tree) -> usize {
    usize::from(tree.tag == tree::Tag::of::<State>())
        + tree.children.iter().map(count_in).sum::<usize>()
}

/// Frame index to draw: the first frame while paused.
pub fn shown_index(anim: &Anim, epoch: Instant, now: Option<Instant>, playing: bool) -> usize {
    match now {
        Some(now) if playing => anim.index_at(now.saturating_duration_since(epoch)),
        _ => 0,
    }
}

/// Origin of the redraw grid, shared by every animation whatever its own
/// epoch. It lies a day before the first call, so it precedes every epoch.
pub fn grid_origin() -> Instant {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *ORIGIN.get_or_init(|| {
        let now = Instant::now();
        now.checked_sub(Duration::from_secs(86_400)).unwrap_or(now)
    })
}

/// `at` rounded up to the 60 Hz grid counted from `origin`.
pub fn snap_to_slot(origin: Instant, at: Instant) -> Instant {
    let since = at.saturating_duration_since(origin).as_nanos();
    let slot = FRAME_SLOT.as_nanos();
    let k = since.div_ceil(slot);
    origin + Duration::from_nanos((k * slot) as u64)
}

/// When the next redraw is due, or `None` when nothing will change
/// (paused, or a single frame).
pub fn next_redraw(anim: &Anim, epoch: Instant, now: Instant, playing: bool) -> Option<Instant> {
    if !playing {
        return None;
    }
    let wait = anim.until_next(now.saturating_duration_since(epoch))?;
    let at = snap_to_slot(grid_origin(), now + wait);
    // Never ask for the current instant again (a redraw loop).
    Some(if at <= now { now + FRAME_SLOT } else { at })
}

impl<M, T, R> Widget<M, T, R> for AnimImage
where
    R: adv_image::Renderer<Handle = Handle> + renderer::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.width), Length::Fixed(self.height))
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &R, limits: &layout::Limits) -> layout::Node {
        let size = Size::new(self.width, self.height);
        layout::Node::new(limits.resolve(
            Length::Fixed(size.width),
            Length::Fixed(size.height),
            size,
        ))
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &R,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<State>();
            state.now = Some(*now);
            // Off-screen images stop asking; scrolling back redraws them.
            if layout.bounds().intersects(viewport)
                && let Some(at) = next_redraw(&self.anim, self.epoch, *now, self.playing)
            {
                shell.request_redraw_at(at);
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        _theme: &T,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let i = shown_index(&self.anim, self.epoch, state.now, self.playing);
        let Some(frame) = self.anim.frames.get(i) else {
            return;
        };
        let bounds = layout.bounds();
        let (w, h) = self.anim.size;
        let fitted = self.fit.fit(Size::new(w as f32, h as f32), bounds.size());
        let drawn = Rectangle {
            x: bounds.x + (bounds.width - fitted.width) / 2.0,
            y: bounds.y + (bounds.height - fitted.height) / 2.0,
            width: fitted.width,
            height: fitted.height,
        };
        let mut image = adv_image::Image::new(frame.handle.clone());
        image.border_radius = self.radius.into();
        if fitted.width > bounds.width + 0.5 || fitted.height > bounds.height + 0.5 {
            renderer.with_layer(bounds, |r| r.draw_image(image, drawn, bounds));
        } else {
            renderer.draw_image(image, drawn, bounds);
        }
    }
}

impl<'a, M, T, R> From<AnimImage> for Element<'a, M, T, R>
where
    R: adv_image::Renderer<Handle = Handle> + renderer::Renderer + 'a,
{
    fn from(image: AnimImage) -> Self {
        Element::new(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::anim::from_rgba_frames;

    fn anim_ms(delays: &[u64]) -> Anim {
        from_rgba_frames(
            delays
                .iter()
                .map(|d| (1, 1, vec![0u8; 4], Duration::from_millis(*d)))
                .collect(),
        )
    }

    #[test]
    fn paused_shows_the_first_frame_and_asks_nothing() {
        let a = anim_ms(&[100, 50, 200]);
        let t0 = Instant::now();
        let later = t0 + Duration::from_millis(120);
        assert_eq!(shown_index(&a, t0, Some(later), false), 0);
        assert_eq!(next_redraw(&a, t0, later, false), None);
    }

    #[test]
    fn a_still_image_never_redraws() {
        let a = anim_ms(&[100]);
        let t0 = Instant::now();
        assert_eq!(next_redraw(&a, t0, t0, true), None);
    }

    #[test]
    fn next_redraw_lands_on_the_frame_change() {
        let a = anim_ms(&[100, 50, 200]);
        let t0 = Instant::now();
        let now = t0 + Duration::from_millis(120);
        let at = next_redraw(&a, t0, now, true).unwrap();
        assert!(at >= t0 + Duration::from_millis(150));
        assert!(at <= t0 + Duration::from_millis(150) + FRAME_SLOT);
        assert_eq!(shown_index(&a, t0, Some(at), true), 2);
    }

    #[test]
    fn redraws_of_many_animations_share_the_60_hz_grid() {
        let t0 = Instant::now();
        let mut slots = std::collections::BTreeSet::new();
        for d in 11..60u64 {
            let x = anim_ms(&[d, d + 3]);
            let mut now = t0;
            while let Some(at) = next_redraw(&x, t0, now, true) {
                assert!(at > now, "redraw loop");
                if at - t0 > Duration::from_secs(1) {
                    break;
                }
                let since = (at - grid_origin()).as_nanos();
                assert_eq!(since % FRAME_SLOT.as_nanos(), 0);
                slots.insert(at);
                now = at;
            }
        }
        // Never more than 60 window redraws a second, whatever the count.
        assert!(slots.len() <= 61, "{}", slots.len());
    }
}
