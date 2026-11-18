//! Views. Pure functions `&App -> Element<Message>`.

pub mod chat;
pub mod icons;
pub mod login;
pub mod sidebar;
pub mod theme;

use chrono::{Datelike, Local, TimeZone};
use iced::widget::{Space, button, canvas, center, column, container, row, stack, svg, text};
use iced::{Alignment, Color, Element, Length, Point, Rectangle, Renderer, Theme, mouse};

use crate::app::{App, Message, Ui};
use crate::net::ReceiptStatus;
use icons::Icon;
use theme::FerroTheme;

/// Element alias.
pub type El<'a> = Element<'a, Message>;

/// Root view dispatcher.
pub fn view(app: &App) -> El<'_> {
    let t = &app.theme;
    let base: El<'_> = if app.auth.shows_login() {
        login::view(app)
    } else {
        let conv: El<'_> = match &app.open {
            Some(c) => chat::view(app, c),
            None => chat::empty(app),
        };
        row![
            sidebar::rail(app),
            container(sidebar::view(app))
                .width(t.metrics.sidebar_width)
                .height(Length::Fill)
                .style(theme::solid(t.palette.border))
                .padding(iced::Padding {
                    right: 1.0,
                    ..Default::default()
                }),
            container(conv).width(Length::Fill).height(Length::Fill),
        ]
        .into()
    };
    let mut layers = vec![base];
    if let Some(id) = &app.viewer {
        layers.push(chat::viewer(app, id));
    }
    if let Some(msg) = &app.toast {
        layers.push(toast(t, msg));
    }
    container(stack(layers))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::solid(t.palette.bg_app))
        .into()
}

fn toast<'a>(t: &FerroTheme, msg: &'a str) -> El<'a> {
    let p = t.palette;
    let card = container(
        row![
            text(msg).size(t.metrics.text).color(p.text_primary),
            button(icon(Icon::X, 16.0, p.text_secondary))
                .on_press(Message::Ui(Ui::DismissToast))
                .style(theme::icon_button(t, false))
                .padding(4),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([10, 16])
    .style(theme::rounded(p.bg_selected, 8.0));
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::End)
        .padding(24)
        .into()
}

/// A tinted Lucide icon.
pub fn icon<'a>(i: Icon, size: f32, color: Color) -> El<'a> {
    svg(i.handle())
        .width(size)
        .height(size)
        .style(move |_: &Theme, _| svg::Style { color: Some(color) })
        .into()
}

/// Default avatar: grey circle with a person / group glyph (recreated, not
/// the official artwork).
pub fn avatar<'a>(t: &FerroTheme, group: bool, size: f32) -> El<'a> {
    let p = t.palette;
    let glyph = if group { Icon::Users } else { Icon::User };
    container(center(icon(glyph, size * 0.55, p.avatar_fg)))
        .width(size)
        .height(size)
        .style(theme::rounded(p.avatar_bg, size / 2.0))
        .into()
}

/// Tick marks for outgoing messages.
pub fn ticks<'a>(t: &FerroTheme, status: ReceiptStatus, size: f32) -> El<'a> {
    let p = t.palette;
    match status {
        ReceiptStatus::Pending => icon(Icon::Clock, size * 0.8, p.text_secondary),
        ReceiptStatus::Failed => icon(Icon::CircleAlert, size * 0.9, p.danger),
        ReceiptStatus::Sent => icon(Icon::Check, size, p.text_secondary),
        ReceiptStatus::Delivered => icon(Icon::CheckCheck, size, p.text_secondary),
        ReceiptStatus::Read | ReceiptStatus::Played => icon(Icon::CheckCheck, size, p.success),
    }
}

/// Unread badge.
pub fn badge<'a>(t: &FerroTheme, n: u32, muted: bool) -> El<'a> {
    let p = t.palette;
    let label = if n > 999 {
        "999+".to_string()
    } else {
        n.to_string()
    };
    container(
        text(label)
            .size(12)
            .font(theme::INTER_SEMIBOLD)
            .color(p.accent_text),
    )
    .padding([1, 6])
    .height(20)
    .align_y(Alignment::Center)
    .style(theme::rounded(
        if muted { p.badge_muted } else { p.badge },
        10.0,
    ))
    .into()
}

/// Fixed-size spacer.
pub fn gap<'a>(w: f32) -> El<'a> {
    Space::new().width(w).into()
}

/// Flexible horizontal filler.
pub fn fill<'a>() -> El<'a> {
    Space::new().width(Length::Fill).into()
}

fn local(ts: i64) -> chrono::DateTime<Local> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).single().unwrap_or_default())
}

/// Local day number used for "Today"/"Yesterday" buckets.
pub fn local_day(ts: i64) -> i64 {
    local(ts).date_naive().num_days_from_ce() as i64
}

/// `HH:MM` for bubbles.
pub fn clock(ts: i64) -> String {
    local(ts).format("%H:%M").to_string()
}

/// Chat list timestamp: time today, "Yesterday", weekday, or date.
pub fn list_time(ts: i64, now: i64) -> String {
    let (d, today) = (local_day(ts), local_day(now));
    match today - d {
        0 => clock(ts),
        1 => "Yesterday".into(),
        2..=6 => local(ts).format("%A").to_string(),
        _ => local(ts).format("%d/%m/%Y").to_string(),
    }
}

/// Day separator label in the conversation.
pub fn day_separator(ts: i64, now: i64) -> String {
    let dt = local(ts);
    crate::app::state::day_label(
        local_day(ts),
        local_day(now),
        &dt.format("%A").to_string(),
        &dt.format("%-d %B %Y").to_string(),
    )
    .to_uppercase()
}

/// The little triangle that makes a bubble look like a speech bubble.
#[derive(Debug, Clone, Copy)]
pub struct Tail {
    /// Fill colour (same as the bubble).
    pub color: Color,
    /// Points right (outgoing) or left (incoming).
    pub outgoing: bool,
}

impl<M> canvas::Program<M> for Tail {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let path = canvas::Path::new(|b| {
            if self.outgoing {
                b.move_to(Point::new(0.0, 0.0));
                b.line_to(Point::new(w - 1.0, 0.0));
                b.quadratic_curve_to(Point::new(w, 0.5), Point::new(w - 1.5, 2.0));
                b.line_to(Point::new(0.0, h));
            } else {
                b.move_to(Point::new(w, 0.0));
                b.line_to(Point::new(1.0, 0.0));
                b.quadratic_curve_to(Point::new(0.0, 0.5), Point::new(1.5, 2.0));
                b.line_to(Point::new(w, h));
            }
            b.close();
        });
        frame.fill(&path, self.color);
        vec![frame.into_geometry()]
    }
}

/// A bubble tail element (8×13 px).
pub fn tail<'a>(color: Color, outgoing: bool) -> El<'a> {
    canvas(Tail { color, outgoing }).width(8).height(13).into()
}

/// Truncates to `n` characters with an ellipsis.
pub fn ellipsize(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// Vertical stack helper with no spacing.
pub fn vstack<'a>(items: Vec<El<'a>>) -> El<'a> {
    column(items).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsis() {
        assert_eq!(ellipsize("hello", 10), "hello");
        assert_eq!(ellipsize("hello world", 5), "hello…");
    }

    #[test]
    fn list_time_buckets() {
        let now = chrono::Local::now().timestamp();
        assert_eq!(list_time(now, now), clock(now));
        assert_eq!(list_time(now - 86_400, now), "Yesterday");
        assert!(list_time(now - 86_400 * 30, now).contains('/'));
    }
}
