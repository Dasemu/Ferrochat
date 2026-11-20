//! `width_of`: an invisible element as wide as its content and zero pixels
//! high. A Shrink column sizes Fill children to its widest non-Fill child,
//! so putting this next to a Fill element makes that element at least as
//! wide as the measured content (up to the available width) without
//! drawing anything or taking vertical space.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget};
use iced::{Element, Length, Rectangle, Size, mouse};

struct WidthOf<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

/// An invisible, zero-height element as wide as `content`.
pub fn width_of<'a, Message: 'a, Theme: 'a, Renderer: renderer::Renderer + 'a>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> Element<'a, Message, Theme, Renderer> {
    Element::new(WidthOf {
        content: content.into(),
    })
}

impl<Message, Theme, Renderer: renderer::Renderer> Widget<Message, Theme, Renderer>
    for WidthOf<'_, Message, Theme, Renderer>
{
    fn size(&self) -> Size<Length> {
        // Not `Fixed(0)` height: columns drop children whose size is void.
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let Some(child_tree) = tree.children.first_mut() else {
            return layout::Node::new(Size::ZERO);
        };
        // Measured with the real width limit and unlimited height (a zero
        // height limit would lay out no line of text at all).
        let inner = layout::Limits::new(Size::ZERO, Size::new(limits.max().width, f32::INFINITY));
        let child = self
            .content
            .as_widget_mut()
            .layout(child_tree, renderer, &inner);
        let width = child.size().width.min(limits.max().width);
        layout::Node::new(Size::new(width, 0.0))
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}
