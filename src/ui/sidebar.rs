//! Navigation rail and chat list column.

use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, row, scrollable, stack, text, text_input};
use iced::{Alignment, Length};

use super::icons::Icon;
use super::theme::{self, INTER_MEDIUM, INTER_SEMIBOLD};
use super::{El, avatar, badge, ellipsize, fill, gap, icon, list_time, ticks};
use crate::app::state::{ChatEntry, Filter};
use crate::app::{App, Message, Ui};
use crate::net::Connection;

/// Left navigation rail. Only "Chats" is functional; the other sections
/// (status, channels, communities) are out of scope and shown disabled.
pub fn rail(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let item = |i: Icon, active: bool| -> El<'_> {
        container(icon(
            i,
            22.0,
            if active {
                p.text_primary
            } else {
                p.text_secondary
            },
        ))
        .padding(9)
        .style(if active {
            theme::rounded(p.bg_selected, 20.0)
        } else {
            theme::rounded(iced::Color::TRANSPARENT, 20.0)
        })
        .into()
    };
    let unread: u32 = app.chats.entries.iter().map(|e| e.meta.unread).sum();
    let chats_item: El<'_> = if unread > 0 {
        stack![
            item(Icon::MessageCircle, true),
            container(badge(t, unread, false))
                .width(40)
                .align_x(Alignment::End)
                .padding(iced::Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 18.0
                })
        ]
        .into()
    } else {
        item(Icon::MessageCircle, true)
    };
    container(
        column![
            chats_item,
            item(Icon::CircleDashed, false),
            item(Icon::UsersRound, false),
            Space::new().height(Length::Fill),
            item(Icon::Settings, false),
            container(avatar(t, false, 32.0)).padding(4),
        ]
        .spacing(10)
        .align_x(Alignment::Center),
    )
    .width(t.metrics.rail_width)
    .height(Length::Fill)
    .padding([12, 0])
    .center_x(t.metrics.rail_width)
    .style(theme::solid(p.bg_rail))
    .into()
}

/// The chat list column.
pub fn view(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;

    let header = row![
        text("Chats")
            .size(m.text_title)
            .font(INTER_SEMIBOLD)
            .color(p.text_primary),
        fill(),
        button(icon(Icon::MessageSquarePlus, m.icon, p.text_secondary))
            .padding(8)
            .style(theme::icon_button(t, false)),
        button(icon(Icon::EllipsisVertical, m.icon, p.text_secondary))
            .padding(8)
            .style(theme::icon_button(t, app.menu_open))
            .on_press(Message::Ui(Ui::ToggleMenu)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .padding(iced::Padding {
        top: 10.0,
        right: 10.0,
        bottom: 6.0,
        left: 20.0,
    });

    let search = container(
        row![
            icon(Icon::Search, 18.0, p.text_secondary),
            text_input("Search or start a new chat", &app.search)
                .on_input(|s| Message::Ui(Ui::Search(s)))
                .size(m.text)
                .padding([6, 0])
                .style(theme::input(t)),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding([3, 16])
    .style(theme::rounded(p.bg_input, 20.0));

    let chips = row(Filter::ALL.iter().map(|f| {
        button(text(f.label()).size(14).font(INTER_MEDIUM))
            .padding([6, 12])
            .style(theme::chip(t, app.filter == *f))
            .on_press(Message::Ui(Ui::Filter(*f)))
            .into()
    }))
    .spacing(8);

    let mut top = column![
        header,
        container(search).padding([4, 14]),
        container(chips).padding([10, 16])
    ];

    if app.connection != Connection::Connected {
        let label = match app.connection {
            Connection::Reconnecting { attempt } => format!("Reconnecting… (attempt {attempt})"),
            Connection::Connecting => "Connecting…".into(),
            _ => "Computer not connected".into(),
        };
        top = top.push(banner(app, Icon::WifiOff, label));
    } else if let Some(pr) = app.sync_progress {
        top = top.push(banner(
            app,
            Icon::RefreshCw,
            format!("Syncing older messages… {pr}%"),
        ));
    }

    let now = chrono::Utc::now().timestamp();
    let entries = app.chats.filtered(&app.search, app.filter);
    let list: El<'_> = if entries.is_empty() {
        container(
            text(if app.search.is_empty() {
                "No chats yet"
            } else {
                "No chats, contacts or messages found"
            })
            .size(m.text)
            .color(p.text_secondary),
        )
        .center_x(Length::Fill)
        .padding(40)
        .into()
    } else {
        let mut col = column![];
        let archived = app.chats.archived_count();
        if archived > 0 && app.filter == Filter::All && app.search.is_empty() {
            col = col.push(
                container(
                    row![
                        container(icon(Icon::Archive, 20.0, p.accent))
                            .width(49)
                            .center_x(49),
                        text("Archived").size(16).color(p.text_primary),
                        fill(),
                        text(archived.to_string()).size(12).color(p.accent),
                    ]
                    .spacing(15)
                    .align_y(Alignment::Center),
                )
                .padding([14, 16]),
            );
        }
        for e in entries {
            col = col.push(chat_row(app, e, now));
        }
        scrollable(col)
            .direction(super::thin_scrollbar())
            .style(theme::scrollbar(t))
            .height(Length::Fill)
            .into()
    };

    let body = column![top, list].height(Length::Fill);

    let mut layers = vec![body.into()];
    if app.menu_open {
        layers.push(menu(app));
    }
    container(stack(layers))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::solid(p.bg_sidebar))
        .into()
}

fn banner(app: &App, i: Icon, label: String) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    container(
        row![
            container(icon(i, 20.0, p.bg_sidebar))
                .padding(10)
                .style(theme::rounded(p.notice_text, 22.0)),
            text(label).size(14).color(p.text_primary),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding([10, 16])
    .width(Length::Fill)
    .style(theme::solid(p.system_chip))
    .into()
}

fn menu(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let item = |label: &'static str, i: Icon, msg: Option<Message>| -> El<'_> {
        let b = button(
            row![
                icon(i, 18.0, p.text_secondary),
                text(label).size(14.5).color(p.text_primary)
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([10, 20])
        .style(theme::chat_row(t, false));
        match msg {
            Some(m) => b.on_press(m).into(),
            None => b.into(),
        }
    };
    let card = container(
        column![
            item("New group", Icon::Users, None),
            item("Starred messages", Icon::Star, None),
            item("Settings", Icon::Settings, None),
            item("Log out", Icon::LogOut, Some(Message::Ui(Ui::Logout))),
        ]
        .width(220),
    )
    .padding([8, 0])
    .style(move |th: &iced::Theme| {
        let mut s = theme::rounded(p.bg_elevated, 6.0)(th);
        s.shadow = iced::Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 10.0,
        };
        s
    });
    container(card)
        .width(Length::Fill)
        .align_x(Alignment::End)
        .padding(iced::Padding {
            top: 52.0,
            right: 12.0,
            bottom: 0.0,
            left: 0.0,
        })
        .into()
}

fn chat_row<'a>(app: &'a App, e: &'a ChatEntry, now: i64) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let selected = app.open.as_ref().is_some_and(|c| c.chat == e.meta.id);
    let name = app.chats.display_name(&e.meta.id);
    let unread = e.meta.unread;

    let time = e
        .last
        .as_ref()
        .map(|l| list_time(l.timestamp, now))
        .unwrap_or_else(|| {
            if e.meta.last_ts > 0 {
                list_time(e.meta.last_ts, now)
            } else {
                String::new()
            }
        });

    let mut preview_row = row![].spacing(3).align_y(Alignment::Center);
    if let Some(l) = &e.last {
        if l.from_me {
            preview_row = preview_row.push(ticks(t, l.status, 16.0));
        } else if e.is_group() {
            let who = l
                .sender_name
                .clone()
                .unwrap_or_else(|| app.chats.display_name(&l.sender));
            preview_row = preview_row.push(
                text(format!("{}: ", ellipsize(&who, 14)))
                    .size(m.text_small + 0.5)
                    .color(p.text_secondary)
                    .wrapping(Wrapping::None),
            );
        }
        preview_row = preview_row.push(
            text(ellipsize(&l.content.preview(), 42))
                .size(m.text_small + 0.5)
                .color(p.text_secondary)
                .wrapping(Wrapping::None)
                .shaping(text::Shaping::Advanced),
        );
    }
    preview_row = preview_row.push(fill());
    if e.meta.muted {
        preview_row = preview_row.push(icon(Icon::BellOff, 15.0, p.text_secondary));
    }
    if e.meta.pinned {
        preview_row = preview_row.push(icon(Icon::Pin, 15.0, p.text_secondary));
    }
    if unread > 0 {
        preview_row = preview_row
            .push(gap(4.0))
            .push(badge(t, unread, e.meta.muted));
    }

    let content = column![
        row![
            text(ellipsize(&name, 34))
                .size(16.5)
                .color(p.text_primary)
                .wrapping(Wrapping::None)
                .shaping(text::Shaping::Advanced),
            fill(),
            text(time).size(12).color(if unread > 0 && !e.meta.muted {
                p.accent
            } else {
                p.text_secondary
            }),
        ]
        .align_y(Alignment::Center),
        preview_row,
    ]
    .spacing(3)
    .width(Length::Fill);

    let line = container(Space::new().height(1))
        .width(Length::Fill)
        .style(theme::solid(if selected {
            iced::Color::TRANSPARENT
        } else {
            p.border
        }));

    let inner = row![
        container(avatar(t, e.is_group(), m.avatar)).padding([0, 15]),
        column![
            Space::new().height(Length::Fill),
            container(content).padding(iced::Padding {
                top: 0.0,
                right: 15.0,
                bottom: 0.0,
                left: 0.0
            }),
            Space::new().height(Length::Fill),
            line,
        ]
        .height(m.row_height),
    ]
    .align_y(Alignment::Center);

    button(inner)
        .padding(0)
        .width(Length::Fill)
        .height(m.row_height)
        .style(theme::chat_row(t, selected))
        .on_press(Message::Ui(Ui::OpenChat(e.meta.id.clone())))
        .into()
}
