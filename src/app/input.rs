//! Pointer position, tracked without a message per mouse move.
//!
//! iced's `mouse_area` reports no window position for right clicks. The
//! event subscription writes the latest cursor position here (and returns no
//! message, so nothing is updated or redrawn); context menus read it when the
//! right click arrives.

use std::sync::atomic::{AtomicU32, Ordering};

use iced::Point;

static X: AtomicU32 = AtomicU32::new(0);
static Y: AtomicU32 = AtomicU32::new(0);

/// Records the cursor position (logical pixels).
pub fn set(p: Point) {
    X.store(p.x.to_bits(), Ordering::Relaxed);
    Y.store(p.y.to_bits(), Ordering::Relaxed);
}

/// The last known cursor position.
pub fn get() -> Point {
    Point::new(
        f32::from_bits(X.load(Ordering::Relaxed)),
        f32::from_bits(Y.load(Ordering::Relaxed)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_round_trips() {
        set(Point::new(12.5, 340.0));
        assert_eq!(get(), Point::new(12.5, 340.0));
    }
}
