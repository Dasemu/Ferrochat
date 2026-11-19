//! The app's own icon (`assets/ferrochat*.svg`, original artwork) rendered to
//! raw pixels for the tray (ARGB32) and the window (RGBA).

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

/// The normal icon.
pub const NORMAL_SVG: &[u8] = include_bytes!("../assets/ferrochat.svg");
/// The icon with an unread dot, shown while chats are unread.
pub const ATTENTION_SVG: &[u8] = include_bytes!("../assets/ferrochat-attention.svg");
/// Sizes the tray offers to the panel.
pub const TRAY_SIZES: [u32; 3] = [22, 32, 48];

/// A square raster icon with straight (not premultiplied) alpha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    /// Width and height in pixels.
    pub size: u32,
    /// RGBA bytes, `size * size * 4`.
    pub rgba: Vec<u8>,
}

impl Raster {
    /// ARGB32 in network byte order, the StatusNotifierItem pixmap format.
    pub fn argb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for px in self.rgba.chunks_exact(4) {
            out.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
        }
        out
    }
}

/// Renders `svg` at `size` x `size`. `None` if the SVG cannot be parsed.
pub fn render(svg: &[u8], size: u32) -> Option<Raster> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = Pixmap::new(size, size)?;
    let scale = size as f32 / tree.size().width().max(1.0);
    resvg::render(
        &tree,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(Raster { size, rgba })
}

/// The window icon (64 px), if it renders.
pub fn window_icon() -> Option<iced::window::Icon> {
    let r = render(NORMAL_SVG, 64)?;
    iced::window::icon::from_rgba(r.rgba, r.size, r.size).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_tray_size() {
        for s in TRAY_SIZES {
            for svg in [NORMAL_SVG, ATTENTION_SVG] {
                let r = render(svg, s).expect("renders");
                assert_eq!(r.rgba.len(), (s * s * 4) as usize);
                assert_eq!(r.argb().len(), r.rgba.len());
            }
        }
    }

    #[test]
    fn corners_are_transparent_and_centre_is_opaque() {
        let r = render(NORMAL_SVG, 48).expect("renders");
        assert_eq!(r.rgba[3], 0, "rounded corner is transparent");
        let centre = ((24 * 48) + 24) * 4;
        assert_eq!(r.rgba[centre + 3], 255);
    }

    #[test]
    fn argb_is_alpha_first() {
        let r = Raster {
            size: 1,
            rgba: vec![1, 2, 3, 4],
        };
        assert_eq!(r.argb(), vec![4, 1, 2, 3]);
    }

    #[test]
    fn attention_icon_differs_in_the_corner_dot() {
        let a = render(NORMAL_SVG, 48).expect("a");
        let b = render(ATTENTION_SVG, 48).expect("b");
        assert_ne!(a.rgba, b.rgba);
    }

    #[test]
    fn bad_svg_is_none() {
        assert!(render(b"nope", 22).is_none());
    }
}
