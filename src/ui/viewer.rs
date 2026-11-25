//! The full-size image viewer layer (`app::viewer`): dark overlay, top bar
//! with the sender and the actions, the picture fitted to the window and
//! drawn on a canvas so it can be zoomed around the cursor and dragged,
//! and previous / next arrows.

use iced::widget::canvas::{self, Canvas, Frame, Geometry};
use iced::widget::{Space, button, column, container, opaque, row, stack, text};
use iced::{Alignment, Color, Length, Point, Rectangle, Renderer, Theme, Vector, mouse};

use super::emoji_font::emoji_text;
use super::icons::Icon;
use super::theme::{self, INTER_MEDIUM};
use super::{El, avatar_for, clock, fill, icon};
use crate::app::viewer::{self, ImageViewer, ZOOM_STEP};
use crate::app::{App, Message, Ui};
use crate::net::Content;

/// Height of the top bar.
const BAR: f32 = 60.0;
/// Margin kept around the fitted picture.
const MARGIN: f32 = 48.0;

/// The picture with its zoom and pan, as a canvas program.
struct Picture {
    handle: Option<iced::widget::image::Handle>,
    size: (u32, u32),
    zoom: f32,
    pan: Vector,
}

/// Drag state kept by the canvas between events.
#[derive(Default)]
struct Drag {
    /// Where the button went down, and the last pointer position.
    from: Option<(Point, Point)>,
    moved: bool,
}

impl Picture {
    /// Fitted size at zoom 1 inside `bounds`.
    fn fitted(&self, bounds: Rectangle) -> (f32, f32) {
        viewer::fit(
            self.size.0,
            self.size.1,
            (bounds.width - 2.0 * MARGIN).max(1.0),
            (bounds.height - 2.0 * MARGIN).max(1.0),
        )
    }

    /// Where the picture is drawn, in canvas coordinates.
    fn rect(&self, bounds: Rectangle) -> Rectangle {
        let (w, h) = self.fitted(bounds);
        let (w, h) = (w * self.zoom, h * self.zoom);
        let c = Point::new(bounds.width / 2.0, bounds.height / 2.0) + self.pan;
        Rectangle::new(
            Point::new(c.x - w / 2.0, c.y - h / 2.0),
            iced::Size::new(w, h),
        )
    }
}

impl canvas::Program<Message> for Picture {
    type State = Drag;

    fn update(
        &self,
        state: &mut Drag,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let pos = cursor.position_in(bounds);
        match event {
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let p = pos?;
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / 40.0,
                };
                if y == 0.0 {
                    return None;
                }
                let factor = ZOOM_STEP.powf(y.clamp(-3.0, 3.0));
                let at = p - Point::new(bounds.width / 2.0, bounds.height / 2.0);
                Some(
                    canvas::Action::publish(Message::Ui(Ui::ViewerZoom { factor, at }))
                        .and_capture(),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let p = pos?;
                state.from = Some((p, p));
                state.moved = false;
                Some(canvas::Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let (start, last) = state.from?;
                let p = cursor.position_in(bounds).unwrap_or(last);
                let d = p - last;
                state.from = Some((start, p));
                if (p - start).x.abs() + (p - start).y.abs() > 3.0 {
                    state.moved = true;
                }
                (self.zoom > viewer::MIN_ZOOM && state.moved).then(|| {
                    canvas::Action::publish(Message::Ui(Ui::ViewerPan {
                        delta: d,
                        fitted: self.fitted(bounds),
                    }))
                    .and_capture()
                })
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let (start, _) = state.from.take()?;
                if state.moved {
                    return Some(canvas::Action::capture());
                }
                // A click outside the picture closes the viewer.
                if !self.rect(bounds).contains(start) {
                    return Some(
                        canvas::Action::publish(Message::Ui(Ui::CloseViewer)).and_capture(),
                    );
                }
                None
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Drag,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if let Some(h) = &self.handle {
            frame.draw_image(self.rect(bounds), canvas::Image::new(h.clone()));
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Drag,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let Some(p) = cursor.position_in(bounds) else {
            return mouse::Interaction::default();
        };
        if !self.rect(bounds).contains(p) {
            return mouse::Interaction::default();
        }
        if self.zoom > viewer::MIN_ZOOM {
            if state.from.is_some() {
                mouse::Interaction::Grabbing
            } else {
                mouse::Interaction::Grab
            }
        } else {
            mouse::Interaction::ZoomIn
        }
    }
}

/// The viewer layer.
pub fn view<'a>(app: &'a App, v: &'a ImageViewer) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let white = Color::WHITE;
    let msg = app.find_message(&v.id);
    let (w, h, thumb) = match msg.map(|m| &m.content) {
        Some(&Content::Image { width, height, .. }) => {
            (width, height, app.thumbs.get(&v.id).cloned())
        }
        _ => (0, 0, None),
    };
    // The full picture, else the blurred inline thumbnail while it loads.
    let full = app.viewer_handle().cloned();
    let loading = full.is_none() && !v.failed;
    let picture = Canvas::new(Picture {
        handle: full.or(thumb),
        size: (w, h),
        zoom: v.zoom,
        pan: v.pan,
    })
    .width(Length::Fill)
    .height(Length::Fill);

    let ctl = |i: Icon, tip: Ui| -> El<'a> {
        button(icon(i, 22.0, white))
            .padding(9)
            .style(theme::icon_button(t, false))
            .on_press(Message::Ui(tip))
            .into()
    };
    let who: El<'a> = match msg {
        Some(m) => {
            let name = if m.from_me {
                "You".to_string()
            } else {
                app.chats.sender_name(&m.sender, m.sender_name.as_deref())
            };
            let who_jid = if m.from_me {
                app.own_jid.clone().unwrap_or_default()
            } else {
                m.sender.clone()
            };
            row![
                avatar_for(app, &who_jid, false, 40.0),
                column![
                    emoji_text(name, 15.0, white, INTER_MEDIUM, false),
                    text(clock(m.timestamp)).size(12.5).color(p.text_secondary),
                ]
                .spacing(2),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        }
        None => Space::new().into(),
    };
    let bar = container(
        row![
            who,
            fill(),
            ctl(
                Icon::ZoomOut,
                Ui::ViewerZoom {
                    factor: 1.0 / ZOOM_STEP,
                    at: Vector::ZERO
                }
            ),
            ctl(
                Icon::ZoomIn,
                Ui::ViewerZoom {
                    factor: ZOOM_STEP,
                    at: Vector::ZERO
                }
            ),
            ctl(Icon::Download, Ui::ViewerSave),
            ctl(Icon::ExternalLink, Ui::ViewerOpenExternal),
            ctl(Icon::X, Ui::CloseViewer),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .height(BAR)
    .padding([0, 16])
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .style(theme::solid(p.bg_app));

    let ids = app.viewer_ids();
    let pos = ids.iter().position(|x| *x == v.id);
    let arrow = |i: Icon, d: i32, enabled: bool| -> El<'a> {
        if !enabled {
            return Space::new().width(52).into();
        }
        button(icon(i, 28.0, white))
            .padding(12)
            .style(theme::icon_button(t, false))
            .on_press(Message::Ui(Ui::ViewerStep(d)))
            .into()
    };
    let arrows = container(
        row![
            arrow(Icon::ChevronLeft, -1, pos.is_some_and(|i| i > 0)),
            fill(),
            arrow(
                Icon::ChevronRight,
                1,
                pos.is_some_and(|i| i + 1 < ids.len())
            ),
        ]
        .align_y(Alignment::Center),
    )
    .padding([0, 12])
    .height(Length::Fill)
    .align_y(Alignment::Center);

    // The bar is a layer drawn after the picture: tiny-skia does not clip
    // images, so a zoomed picture would otherwise paint over it.
    let mut layers: Vec<El<'a>> = vec![
        container(picture)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding::ZERO.top(BAR))
            .style(theme::solid(p.bg_app.scale_alpha(0.97)))
            .into(),
        container(arrows)
            .padding(iced::Padding::ZERO.top(BAR))
            .into(),
    ];
    if loading || v.failed {
        layers.push(
            container(
                container(
                    text(if v.failed {
                        "The photo could not be downloaded."
                    } else {
                        "Loading…"
                    })
                    .size(14)
                    .color(white),
                )
                .padding([6, 14])
                .style(theme::rounded(Color::from_rgba(0.0, 0.0, 0.0, 0.6), 14.0)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::End)
            .padding(28)
            .into(),
        );
    }
    layers.push(
        column![opaque(bar), Space::new().height(Length::Fill)]
            .width(Length::Fill)
            .into(),
    );
    opaque(stack(layers))
}
