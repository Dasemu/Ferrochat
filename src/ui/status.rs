//! The Status tab: the list of contacts' updates, the full-window viewer
//! and the text status composer.

use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, column, container, image, mouse_area, opaque, progress_bar, row, scrollable,
    stack, text, text_input,
};
use iced::{Alignment, Background, Border, Color, ContentFit, Font, Length, font};

use super::icons::Icon;
use super::theme::{self, INTER, INTER_MEDIUM, INTER_SEMIBOLD};
use super::{El, avatar_for, clock, ellipsize, fill, icon, local_day};
use crate::app::nav::NavMsg;
use crate::app::status::{STATUS_POSTING_ENABLED, TEXT_COLORS, TEXT_FONTS, segment_fill};
use crate::app::{App, Message, Ui};
use crate::net::social::{StatusBody, StatusGroup, StatusItem};

fn nav(m: NavMsg) -> Message {
    Message::Ui(Ui::Nav(m))
}

/// `0xAARRGGBB` as an iced colour.
pub fn argb(v: u32) -> Color {
    Color::from_rgba8(
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
        ((v >> 24) & 0xff) as f32 / 255.0,
    )
}

/// The font a status text id maps to (WhatsApp's `FontType`).
pub fn status_font(id: i32) -> Font {
    match id {
        6 | 9 => INTER_SEMIBOLD,
        7 => Font {
            family: font::Family::Cursive,
            ..INTER
        },
        8 => Font {
            family: font::Family::Serif,
            weight: font::Weight::Bold,
            ..INTER
        },
        10 => Font {
            family: font::Family::Monospace,
            weight: font::Weight::Bold,
            ..INTER
        },
        _ => INTER,
    }
}

/// Text size that keeps a status readable whatever its length.
pub fn status_text_size(chars: usize) -> f32 {
    match chars {
        0..=40 => 44.0,
        41..=120 => 34.0,
        121..=300 => 26.0,
        _ => 20.0,
    }
}

/// "14:05" today, "Yesterday, 14:05" otherwise.
pub fn status_time(ts: i64, now: i64) -> String {
    if local_day(ts) == local_day(now) {
        clock(ts)
    } else {
        format!("Yesterday, {}", clock(ts))
    }
}

fn ring<'a>(app: &'a App, jid: &str, unseen: bool) -> El<'a> {
    let p = app.theme.palette;
    let color = if unseen { p.accent } else { p.text_muted };
    container(avatar_for(app, jid, false, 44.0))
        .padding(3)
        .style(move |_: &iced::Theme| container::Style {
            border: Border {
                color,
                width: 2.0,
                radius: 29.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn name_of(app: &App, author: &str, hint: Option<&str>) -> String {
    app.chats.sender_name(author, hint)
}

fn row_of<'a>(app: &'a App, g: &StatusGroup, now: i64) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let hint = g.items.last().and_then(|i| i.author_name.as_deref());
    let name = name_of(app, &g.author, hint);
    let count = g.items.len();
    let sub = if count > 1 {
        format!("{count} updates · {}", status_time(g.newest(), now))
    } else {
        status_time(g.newest(), now)
    };
    let inner = row![
        ring(app, &g.author, !g.all_viewed()),
        column![
            text(ellipsize(&name, 34))
                .size(16)
                .color(p.text_primary)
                .wrapping(Wrapping::None)
                .shaping(text::Shaping::Advanced),
            text(sub).size(13).color(p.text_secondary),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(14)
    .align_y(Alignment::Center);
    button(container(inner).padding([8, 16]))
        .padding(0)
        .width(Length::Fill)
        .style(theme::chat_row(t, false))
        .on_press(nav(NavMsg::OpenStatus(g.author.clone())))
        .into()
}

fn label<'a>(app: &'a App, s: &'static str) -> El<'a> {
    container(
        text(s)
            .size(13.5)
            .font(INTER_MEDIUM)
            .color(app.theme.palette.text_secondary),
    )
    .padding([12, 20])
    .into()
}

/// The status list column.
pub fn list(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let now = chrono::Utc::now().timestamp();
    let split = app.status_split();
    let mut header = row![
        text("Status")
            .size(m.text_title)
            .font(INTER_SEMIBOLD)
            .color(p.text_primary),
        fill(),
    ];
    if STATUS_POSTING_ENABLED {
        header = header.push(
            button(icon(Icon::Plus, m.icon, p.text_secondary))
                .padding(8)
                .style(theme::icon_button(t, app.status.compose.is_some()))
                .on_press(nav(NavMsg::StatusCompose)),
        );
    }
    let header = header
        .spacing(6)
        .align_y(Alignment::Center)
        .padding(iced::Padding {
            top: 10.0,
            right: 10.0,
            bottom: 6.0,
            left: 20.0,
        });

    // "My status".
    let mine_row: El<'_> = {
        let (sub, on) = match &split.mine {
            Some(g) => (
                format!(
                    "{} update{} · {}",
                    g.items.len(),
                    if g.items.len() == 1 { "" } else { "s" },
                    status_time(g.newest(), now)
                ),
                Some(nav(NavMsg::OpenStatus(g.author.clone()))),
            ),
            None if STATUS_POSTING_ENABLED => (
                "Click to add a status update".to_string(),
                Some(nav(NavMsg::StatusCompose)),
            ),
            None => ("No status updates".to_string(), None),
        };
        let own = app.own_jid.clone().unwrap_or_default();
        let mut lead = stack![container(avatar_for(app, &own, false, 49.0)).padding(2)];
        if STATUS_POSTING_ENABLED && split.mine.is_none() {
            lead = lead.push(
                container(
                    container(icon(Icon::Plus, 12.0, p.accent_text))
                        .padding(2)
                        .style(theme::rounded(p.accent, 9.0)),
                )
                .align_right(53)
                .align_bottom(53),
            );
        }
        let lead: El<'_> = lead.width(53).height(53).into();
        button(
            container(
                row![
                    lead,
                    column![
                        text("My status").size(16).color(p.text_primary),
                        text(sub).size(13).color(p.text_secondary),
                    ]
                    .spacing(2)
                ]
                .spacing(14)
                .align_y(Alignment::Center),
            )
            .padding([8, 16]),
        )
        .padding(0)
        .width(Length::Fill)
        .style(theme::chat_row(t, false))
        .on_press_maybe(on)
        .into()
    };

    let mut items: Vec<El<'_>> = vec![mine_row];
    if split.recent.is_empty() && split.viewed.is_empty() {
        items.push(
            container(
                column![
                    icon(Icon::CircleDashed, 44.0, p.text_secondary),
                    text("No status updates").size(16).color(p.text_primary),
                    text("Updates from your contacts in the last 24 hours show up here.")
                        .size(13.5)
                        .color(p.text_secondary)
                        .align_x(Alignment::Center),
                ]
                .spacing(10)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .padding(40)
            .into(),
        );
    }
    if !split.recent.is_empty() {
        items.push(label(app, "RECENT"));
        for g in &split.recent {
            items.push(row_of(app, g, now));
        }
    }
    if !split.viewed.is_empty() {
        items.push(label(app, "VIEWED"));
        for g in &split.viewed {
            items.push(row_of(app, g, now));
        }
    }
    container(
        column![
            header,
            scrollable(column(items))
                .direction(super::thin_scrollbar())
                .style(theme::scrollbar(t))
                .height(Length::Fill),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::solid(p.bg_sidebar))
    .into()
}

/// The text status composer, shown in the conversation pane.
pub fn composer(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let Some(c) = &app.status.compose else {
        return Space::new().into();
    };
    let bg = argb(TEXT_COLORS[c.color % TEXT_COLORS.len()]);
    let font = status_font(TEXT_FONTS[c.font % TEXT_FONTS.len()]);
    let white = Color::WHITE;
    let can_send = !c.text.trim().is_empty() && !c.sending;
    let send = button(icon(Icon::SendHorizontal, 24.0, p.accent_text))
        .padding(14)
        .style(theme::primary_button(t));
    let send = if can_send {
        send.on_press(nav(NavMsg::StatusPost))
    } else {
        send
    };
    let tool = |i: Icon, on: Message| -> El<'_> {
        button(icon(i, 22.0, white))
            .padding(10)
            .style(theme::icon_button(t, false))
            .on_press(on)
            .into()
    };
    let audience = app.status_audience().len();
    let input = text_input("Type a status", &c.text)
        .on_input(|s| nav(NavMsg::StatusComposeText(s)))
        .on_submit_maybe(can_send.then(|| nav(NavMsg::StatusPost)))
        .size(status_text_size(c.text.chars().count()))
        .font(font)
        .padding(8)
        .align_x(Alignment::Center)
        .style(move |_: &iced::Theme, _| iced::widget::text_input::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            icon: white,
            placeholder: Color { a: 0.6, ..white },
            value: white,
            selection: Color { a: 0.35, ..white },
        });
    container(
        column![
            row![
                tool(Icon::X, nav(NavMsg::StatusComposeCancel)),
                fill(),
                tool(Icon::Type, nav(NavMsg::StatusComposeFont)),
                tool(Icon::Palette, nav(NavMsg::StatusComposeColor)),
            ]
            .padding([8, 12])
            .align_y(Alignment::Center),
            container(container(input).width(Length::Fixed(620.0))).center(Length::Fill),
            row![
                text(format!(
                    "Visible to your {audience} saved contact{}",
                    if audience == 1 { "" } else { "s" }
                ))
                .size(13)
                .color(Color { a: 0.8, ..white }),
                fill(),
                send,
            ]
            .padding([12, 20])
            .align_y(Alignment::Center),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::solid(bg))
    .into()
}

/// The full-window viewer, when open.
pub fn viewer(app: &App) -> Option<El<'_>> {
    let v = app.status.viewer.as_ref()?;
    let item = app.current_status()?;
    let t = &app.theme;
    let now = chrono::Utc::now().timestamp();
    let white = Color::WHITE;
    let dim = Color::from_rgba(1.0, 1.0, 1.0, 0.8);
    let split = app.status_split();
    let group = split
        .recent
        .iter()
        .chain(split.viewed.iter())
        .chain(split.mine.iter())
        .find(|g| g.author == item.author)?;
    let progress = app.status_progress(&item);

    // Content.
    let content: El<'_> = match &item.body {
        StatusBody::Text {
            text: body,
            background,
            font,
        } => container(
            container(
                text(body.clone())
                    .size(status_text_size(body.chars().count()))
                    .font(status_font(*font))
                    .color(white)
                    .align_x(Alignment::Center)
                    .shaping(text::Shaping::Advanced),
            )
            .max_width(640)
            .padding([0, 24]),
        )
        .center(Length::Fill)
        .style(theme::solid(argb(*background)))
        .into(),
        StatusBody::Image { media, .. } => {
            let full = media
                .as_ref()
                .and_then(|m| app.status.images.peek(&m.cache_key()));
            picture(app, &item, full.cloned(), "Loading…")
        }
        StatusBody::Video { .. } => {
            let frame = app
                .video
                .as_ref()
                .filter(|vid| vid.id == item.id)
                .and_then(|vid| vid.frame.clone());
            let failed = app.video.as_ref().is_some_and(|vid| {
                vid.id == item.id && matches!(vid.phase, crate::app::video::VideoPhase::Failed(_))
            });
            picture(
                app,
                &item,
                frame,
                if failed {
                    "This video can't be played."
                } else {
                    "Loading video…"
                },
            )
        }
    };

    // Progress segments.
    let mut segs = row![].spacing(4);
    for i in 0..group.items.len() {
        segs = segs.push(
            progress_bar(0.0..=1.0, segment_fill(i, v.index, progress))
                .length(Length::Fill)
                .girth(3.0)
                .style(|_: &iced::Theme| progress_bar::Style {
                    background: Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.35)),
                    bar: Background::Color(Color::WHITE),
                    border: Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                }),
        );
    }
    let hint = group.items.last().and_then(|i| i.author_name.as_deref());
    let who = if item.from_me {
        "My status".to_string()
    } else {
        name_of(app, &item.author, hint)
    };
    let ctl = |i: Icon, on: Message| -> El<'_> {
        button(icon(i, 22.0, white))
            .padding(8)
            .style(theme::icon_button(t, false))
            .on_press(on)
            .into()
    };
    let header = row![
        avatar_for(app, &item.author, false, 40.0),
        column![
            text(ellipsize(&who, 30))
                .size(16)
                .font(INTER_MEDIUM)
                .color(white),
            text(status_time(item.timestamp, now)).size(13).color(dim),
        ]
        .spacing(1),
        fill(),
        ctl(
            if v.paused { Icon::Play } else { Icon::Pause },
            nav(NavMsg::StatusPause)
        ),
        ctl(Icon::X, nav(NavMsg::CloseStatus)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let top = container(column![segs, header].spacing(10).padding(iced::Padding {
        top: 12.0,
        right: 16.0,
        bottom: 10.0,
        left: 16.0,
    }))
    .width(Length::Fill)
    .style(|_: &iced::Theme| container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.35))),
        ..Default::default()
    });

    // Caption.
    let caption: El<'_> = match item.body.caption() {
        Some(c) => container(
            container(
                text(c.to_string())
                    .size(17)
                    .color(white)
                    .align_x(Alignment::Center)
                    .shaping(text::Shaping::Advanced),
            )
            .max_width(640)
            .padding([10, 18])
            .style(|_: &iced::Theme| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                border: Border {
                    radius: 10.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        )
        .center_x(Length::Fill)
        .padding([0, 20])
        .into(),
        None => Space::new().into(),
    };

    // Click zones: left third goes back, the rest forward.
    let zones = row![
        mouse_area(
            Space::new()
                .width(Length::FillPortion(1))
                .height(Length::Fill)
        )
        .on_press(nav(NavMsg::StatusPrev)),
        mouse_area(
            Space::new()
                .width(Length::FillPortion(2))
                .height(Length::Fill)
        )
        .on_press(nav(NavMsg::StatusNext)),
    ];
    let arrows = row![
        ctl(Icon::ChevronLeft, nav(NavMsg::StatusPrev)),
        fill(),
        ctl(Icon::ChevronRight, nav(NavMsg::StatusNext)),
    ]
    .padding([0, 12])
    .align_y(Alignment::Center);

    Some(opaque(
        container(stack![
            content,
            zones,
            container(arrows).center_y(Length::Fill),
            column![top, Space::new().height(Length::Fill), caption].padding(iced::Padding {
                bottom: 28.0,
                ..Default::default()
            }),
        ])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::solid(Color::BLACK)),
    ))
}

/// A picture (or video frame) centred on black, with the thumbnail and a
/// label while it loads.
fn picture<'a>(
    app: &'a App,
    item: &StatusItem,
    full: Option<iced::widget::image::Handle>,
    loading: &'static str,
) -> El<'a> {
    let white = Color::WHITE;
    match full {
        Some(h) => container(
            image(h)
                .content_fit(ContentFit::Contain)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .center(Length::Fill)
        .into(),
        None => {
            let backdrop: El<'a> =
                match app
                    .status
                    .thumbs
                    .peek(&crate::app::status::StatusState::thumb_key(
                        &item.author,
                        &item.id,
                    )) {
                    Some(h) => image(h.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .opacity(0.45_f32)
                        .into(),
                    None => Space::new().into(),
                };
            stack![
                backdrop,
                container(text(loading).size(16).color(white)).center(Length::Fill)
            ]
            .into()
        }
    }
}
