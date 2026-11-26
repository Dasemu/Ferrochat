//! A small loading spinner (a ring of dots, one lit) that animates itself.
//!
//! It shows nothing until `delay` after `since` (a download that finishes
//! quickly never flashes a spinner) and, like `anim_image`, it never ticks
//! the app: on each redraw it asks for the next one only while it is on
//! screen, at [`STEP`] intervals, so it costs nothing once the picture is
//! there, scrolled away or the window is unfocused.

use std::time::{Duration, Instant};

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::{Border, Color, Element, Event, Length, Rectangle, Size, mouse, window};

/// Time between two lit dots.
pub const STEP: Duration = Duration::from_millis(110);
/// Dots in the ring.
const DOTS: usize = 8;

/// A spinner of `size` that appears `delay` after `since`.
pub struct Spinner {
    since: Instant,
    delay: Duration,
    size: f32,
    color: Color,
}

/// See [`Spinner`].
pub fn spinner(since: Instant, delay: Duration, size: f32, color: Color) -> Spinner {
    Spinner {
        since,
        delay,
        size,
        color,
    }
}

#[derive(Debug, Default)]
struct State {
    now: Option<Instant>,
}

/// The lit dot at `now`, or `None` while the spinner is still hidden.
pub fn lit_dot(since: Instant, delay: Duration, now: Instant) -> Option<usize> {
    let shown = since + delay;
    if now < shown {
        return None;
    }
    let steps = now.duration_since(shown).as_millis() / STEP.as_millis();
    Some((steps % DOTS as u128) as usize)
}

/// When to redraw next: when it appears, then once per [`STEP`].
pub fn next_redraw(since: Instant, delay: Duration, now: Instant) -> Instant {
    let shown = since + delay;
    if now < shown {
        return shown;
    }
    let steps = now.duration_since(shown).as_millis() / STEP.as_millis() + 1;
    shown + STEP * steps as u32
}

impl<M, T, R: renderer::Renderer> Widget<M, T, R> for Spinner {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &R, limits: &layout::Limits) -> layout::Node {
        let size = Size::new(self.size, self.size);
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
            tree.state.downcast_mut::<State>().now = Some(*now);
            if layout.bounds().intersects(viewport) {
                shell.request_redraw_at(next_redraw(self.since, self.delay, *now));
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
        let Some(now) = tree.state.downcast_ref::<State>().now else {
            return;
        };
        let Some(lit) = lit_dot(self.since, self.delay, now) else {
            return;
        };
        let b = layout.bounds();
        // Dark disc behind the dots, readable on any picture.
        renderer.fill_quad(
            renderer::Quad {
                bounds: b,
                border: Border::default().rounded(b.width / 2.0),
                ..renderer::Quad::default()
            },
            Color::from_rgba(0.0, 0.0, 0.0, 0.45),
        );
        let (cx, cy) = (b.x + b.width / 2.0, b.y + b.height / 2.0);
        let ring = b.width * 0.3;
        let dot = (b.width * 0.12).max(2.0);
        for i in 0..DOTS {
            let a = i as f32 / DOTS as f32 * std::f32::consts::TAU;
            // The lit dot is opaque, the ones behind it fade out.
            let age = (lit + DOTS - i) % DOTS;
            let alpha = 1.0 - age as f32 / DOTS as f32 * 0.8;
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: cx + ring * a.sin() - dot / 2.0,
                        y: cy - ring * a.cos() - dot / 2.0,
                        width: dot,
                        height: dot,
                    },
                    border: Border::default().rounded(dot / 2.0),
                    ..renderer::Quad::default()
                },
                self.color.scale_alpha(alpha),
            );
        }
    }
}

impl<'a, M, T, R: renderer::Renderer + 'a> From<Spinner> for Element<'a, M, T, R> {
    fn from(s: Spinner) -> Self {
        Element::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_until_the_delay_then_steps_around_the_ring() {
        let t0 = Instant::now();
        let d = Duration::from_secs(1);
        assert_eq!(lit_dot(t0, d, t0), None);
        assert_eq!(next_redraw(t0, d, t0), t0 + d, "wakes up once, to appear");
        assert_eq!(lit_dot(t0, d, t0 + d), Some(0));
        assert_eq!(lit_dot(t0, d, t0 + d + STEP * 3), Some(3));
        assert_eq!(lit_dot(t0, d, t0 + d + STEP * 9), Some(1));
        let now = t0 + d + STEP * 2 + Duration::from_millis(5);
        assert_eq!(next_redraw(t0, d, now), t0 + d + STEP * 3);
    }
}
