//! Conversation panel: header, bubbles, composer; empty state; image viewer.

use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, center, column, container, image, mouse_area, opaque, row, scrollable, text,
    text_input,
};
use iced::{Alignment, Color, ContentFit, Length};

use super::icons::Icon;
use super::theme::{self, FerroTheme, INTER_MEDIUM};
use super::{El, avatar, clock, day_separator, fill, gap, icon, local_day, tail, ticks};
use crate::app::state::Conversation;
use crate::app::{App, CONVERSATION_SCROLL, Message, Ui};
use crate::media::image::human_size;
use crate::net::{Content, Message as ChatMessage, ReceiptStatus, format_duration, is_group_jid};

/// Emoji offered by the picker (system emoji font renders them).
pub const EMOJI: &[&str] = &[
    "😀", "😂", "🥲", "😊", "😍", "😘", "😎", "🤔", "😅", "😭", "😡", "🥳", "😴", "🤗", "🙄", "😬",
    "👍", "👎", "👏", "🙏", "💪", "👋", "🤝", "✌️", "👌", "❤️", "🧡", "💛", "💚", "💙", "💜", "🔥",
    "✨", "🎉", "🎂", "☕", "🍕", "⚽", "✈️", "🌅", "📷", "📄", "✅", "❌", "💯",
];

/// Placeholder when no chat is open.
pub fn empty(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    container(
        column![
            container(icon(Icon::Laptop, 64.0, p.text_secondary))
                .padding(28)
                .style(theme::rounded(p.bg_elevated, 60.0)),
            text("Ferrochat for Linux").size(30).color(p.text_primary),
            text(
                "Send and receive messages from your computer. Your phone stays the \
                 primary device; Ferrochat is linked to it."
            )
            .size(14)
            .color(p.text_secondary)
            .align_x(Alignment::Center)
            .width(460),
            Space::new().height(Length::Fill).height(40),
            row![
                icon(Icon::Lock, 13.0, p.text_secondary),
                text("Your personal messages are end-to-end encrypted")
                    .size(13)
                    .color(p.text_secondary),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(18)
        .align_x(Alignment::Center),
    )
    .center(Length::Fill)
    .style(theme::bordered(
        p.bg_elevated,
        p.notice_text.scale_alpha(0.0),
    ))
    .into()
}

/// The open conversation.
pub fn view<'a>(app: &'a App, conv: &'a Conversation) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let group = is_group_jid(&conv.chat);
    let name = app.chats.display_name(&conv.chat);

    let subtitle = if group {
        let mut senders: Vec<String> = Vec::new();
        for msg in conv.messages.iter().rev().filter(|m| !m.from_me) {
            let n = msg
                .sender_name
                .clone()
                .unwrap_or_else(|| app.chats.display_name(&msg.sender));
            if !senders.contains(&n) {
                senders.push(n);
            }
            if senders.len() == 5 {
                break;
            }
        }
        senders.push("You".into());
        senders.join(", ")
    } else {
        "click here for contact info".into()
    };

    let header = container(
        row![
            avatar(t, group, m.avatar_small),
            column![
                text(name)
                    .size(16)
                    .color(p.text_primary)
                    .shaping(text::Shaping::Advanced),
                text(super::ellipsize(&subtitle, 80))
                    .size(13)
                    .color(p.text_secondary)
                    .wrapping(Wrapping::None),
            ]
            .spacing(1),
            fill(),
            button(icon(Icon::Search, m.icon, p.text_secondary))
                .padding(8)
                .style(theme::icon_button(t, false)),
            button(icon(Icon::EllipsisVertical, m.icon, p.text_secondary))
                .padding(8)
                .style(theme::icon_button(t, false)),
        ]
        .spacing(15)
        .align_y(Alignment::Center),
    )
    .height(m.header_height)
    .padding([0, 16])
    .align_y(Alignment::Center)
    .style(theme::solid(p.bg_elevated));

    // Messages
    let now = chrono::Utc::now().timestamp();
    let mut items: Vec<El<'a>> = Vec::with_capacity(conv.messages.len() + 8);
    items.push(
        container(
            container(
                row![
                    icon(Icon::Lock, 12.0, p.notice_text),
                    text(
                        "Messages are end-to-end encrypted. No one outside of this chat, not \
                         even WhatsApp, can read or listen to them."
                    )
                    .size(12.5)
                    .color(p.notice_text)
                    .align_x(Alignment::Center),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 12])
            .max_width(520)
            .style(theme::rounded(p.system_chip, 7.5)),
        )
        .center_x(Length::Fill)
        .padding([10, 0])
        .into(),
    );
    if conv.has_more {
        items.push(
            container(
                text("Scroll up to load older messages")
                    .size(12)
                    .color(p.text_secondary),
            )
            .center_x(Length::Fill)
            .into(),
        );
    }
    let mut prev: Option<&ChatMessage> = None;
    for msg in &conv.messages {
        let new_day = prev.is_none_or(|pm| local_day(pm.timestamp) != local_day(msg.timestamp));
        if new_day {
            items.push(
                container(
                    container(
                        text(day_separator(msg.timestamp, now))
                            .size(12.5)
                            .color(p.text_secondary),
                    )
                    .padding([5, 12])
                    .style(theme::rounded(p.system_chip, 7.5)),
                )
                .center_x(Length::Fill)
                .padding([10, 0])
                .into(),
            );
        }
        let first_of_group =
            new_day || prev.is_none_or(|pm| pm.from_me != msg.from_me || pm.sender != msg.sender);
        items.push(bubble(app, msg, first_of_group, group));
        prev = Some(msg);
    }

    let messages = scrollable(column(items).spacing(2).padding(iced::Padding {
        top: 4.0,
        right: 56.0,
        bottom: 12.0,
        left: 56.0,
    }))
    .id(CONVERSATION_SCROLL)
    .direction(super::thin_scrollbar())
    .anchor_bottom()
    .on_scroll(|v| {
        Message::Ui(Ui::Scrolled {
            relative_y: v.relative_offset().y,
        })
    })
    .style(theme::scrollbar(t))
    .height(Length::Fill);

    let background = container(messages)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::solid(p.bg_panel));

    let mut body = column![header];
    if app.emoji_open {
        body = body.push(iced::widget::stack![background, emoji_panel(t)]);
    } else {
        body = body.push(background);
    }
    body.push(composer(app)).into()
}

fn emoji_panel(t: &FerroTheme) -> El<'_> {
    let p = t.palette;
    let grid = iced::widget::grid(EMOJI.iter().map(|e| {
        button(text(*e).size(24).shaping(text::Shaping::Advanced))
            .padding(6)
            .style(theme::icon_button(t, false))
            .on_press(Message::Ui(Ui::Emoji(e)))
            .into()
    }))
    .columns(15)
    .spacing(2);
    container(
        container(grid)
            .padding(10)
            .width(Length::Fill)
            .style(theme::solid(p.bg_elevated)),
    )
    .height(Length::Fill)
    .align_y(Alignment::End)
    .into()
}

fn composer(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    if let Some(since) = app.recording_since {
        let secs = since.elapsed().as_secs() as u32;
        return container(
            row![
                fill(),
                button(icon(Icon::Trash2, m.icon, p.text_secondary))
                    .padding(8)
                    .style(theme::icon_button(t, false))
                    .on_press(Message::Ui(Ui::StopRecording { send: false })),
                container(Space::new().width(10).height(10)).style(theme::rounded(p.danger, 5.0)),
                text(format_duration(secs)).size(16).color(p.text_primary),
                button(icon(Icon::SendHorizontal, 20.0, p.accent_text))
                    .padding(11)
                    .style(theme::primary_button(t))
                    .on_press(Message::Ui(Ui::StopRecording { send: true })),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .padding([8, 16])
        .style(theme::solid(p.bg_elevated))
        .into();
    }
    let has_text = !app.composer.trim().is_empty();
    let input = text_input("Type a message", &app.composer)
        .on_input(|s| Message::Ui(Ui::Composer(s)))
        .on_submit(Message::Ui(Ui::Send))
        .size(m.text + 0.5)
        .padding([9, 0])
        .style(theme::input(t));
    let field = container(
        row![
            button(icon(Icon::Plus, 24.0, p.text_secondary))
                .padding(6)
                .style(theme::icon_button(t, false))
                .on_press(Message::Ui(Ui::Attach)),
            button(icon(
                Icon::Smile,
                22.0,
                if app.emoji_open {
                    p.accent
                } else {
                    p.text_secondary
                }
            ))
            .padding(6)
            .style(theme::icon_button(t, false))
            .on_press(Message::Ui(Ui::ToggleEmoji)),
            input,
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding([2, 8])
    .width(Length::Fill)
    .style(theme::rounded(p.bg_selected, 22.0));
    let action: El<'_> = if has_text {
        button(icon(Icon::SendHorizontal, 20.0, p.accent_text))
            .padding(11)
            .style(theme::primary_button(t))
            .on_press(Message::Ui(Ui::Send))
            .into()
    } else {
        button(icon(Icon::Mic, 22.0, p.text_secondary))
            .padding(10)
            .style(theme::icon_button(t, false))
            .on_press(Message::Ui(Ui::StartRecording))
            .into()
    };
    container(row![field, action].spacing(10).align_y(Alignment::Center))
        .padding([8, 16])
        .style(theme::solid(p.bg_elevated))
        .into()
}

fn meta_row<'a>(t: &FerroTheme, msg: &ChatMessage) -> El<'a> {
    let p = t.palette;
    let color = if msg.from_me {
        p.bubble_out_meta
    } else {
        p.text_secondary
    };
    let mut r = row![
        text(clock(msg.timestamp))
            .size(t.metrics.text_tiny)
            .color(color)
    ]
    .spacing(3)
    .align_y(Alignment::Center);
    if msg.from_me {
        r = r.push(ticks(t, msg.status, 15.0));
    }
    r.into()
}

fn bubble<'a>(app: &'a App, msg: &'a ChatMessage, first: bool, group: bool) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let out = msg.from_me;
    let fg = p.text_primary;

    let mut col = column![].spacing(3);
    if group && !out && first {
        let who = msg
            .sender_name
            .clone()
            .unwrap_or_else(|| app.chats.display_name(&msg.sender));
        col = col.push(
            text(who)
                .size(12.5)
                .font(INTER_MEDIUM)
                .color(theme::sender_color(&msg.sender))
                .shaping(text::Shaping::Advanced),
        );
    }

    let body: El<'a> = match &msg.content {
        Content::Text(s) => {
            // Short messages keep the time on the same line, like the original.
            let short = s.chars().count() < 38 && !s.contains('\n');
            let txt = text(s.as_str())
                .size(m.text)
                .color(fg)
                .shaping(text::Shaping::Advanced);
            if short {
                return finish(
                    app,
                    msg,
                    first,
                    col.push(
                        row![txt, gap(10.0), container(meta_row(t, msg)).padding(iced::Padding {
                            top: 6.0,
                            ..Default::default()
                        })]
                        .align_y(Alignment::End),
                    )
                    .into(),
                );
            }
            column![txt, container(meta_row(t, msg)).align_right(Length::Fill)]
                .spacing(1)
                .into()
        }
        Content::Image {
            caption,
            width,
            height,
            media,
            ..
        } => {
            let (w, h) = fit(*width, *height, 330.0, 330.0);
            let handle = media
                .as_ref()
                .and_then(|r| app.images.peek(&r.cache_key()))
                .or_else(|| app.thumbs.get(&msg.id))
                .cloned();
            let pic: El<'a> = match handle {
                Some(handle) => image(handle)
                    .width(w)
                    .height(h)
                    .content_fit(ContentFit::Cover)
                    .border_radius(6.0)
                    .into(),
                None => container(icon(Icon::Image, 40.0, p.text_secondary))
                    .width(w)
                    .height(h)
                    .center_x(w)
                    .center_y(h)
                    .style(theme::rounded(p.inset_in, 6.0))
                    .into(),
            };
            let pic = button(pic)
                .padding(0)
                .style(theme::flat_button(fg))
                .on_press(Message::Ui(Ui::OpenImage(msg.id.clone())));
            let mut c = column![pic].spacing(4).width(w);
            if let Some(cap) = caption {
                c = c.push(
                    text(cap.as_str())
                        .size(m.text)
                        .color(fg)
                        .shaping(text::Shaping::Advanced),
                );
            }
            c.push(container(meta_row(t, msg)).align_right(Length::Fill)).into()
        }
        Content::Document {
            file_name,
            mimetype,
            size,
            pages,
            caption,
            ..
        } => {
            let ext = file_name
                .rsplit_once('.')
                .map(|(_, e)| e.to_uppercase())
                .unwrap_or_else(|| mimetype.rsplit('/').next().unwrap_or("FILE").to_uppercase());
            let mut info = Vec::new();
            if let Some(pg) = pages {
                info.push(format!("{pg} page{}", if *pg == 1 { "" } else { "s" }));
            }
            info.push(ext.clone());
            info.push(human_size(*size));
            let inset = if out { p.inset_out } else { p.inset_in };
            let card = button(
                row![
                    container(
                        column![
                            icon(Icon::FileText, 26.0, Color::WHITE),
                        ]
                        .align_x(Alignment::Center)
                    )
                    .padding([6, 6])
                    .style(theme::rounded(doc_color(&ext), 4.0)),
                    column![
                        text(super::ellipsize(file_name, 34))
                            .size(m.text)
                            .color(fg)
                            .wrapping(Wrapping::None),
                        text(info.join(" • ")).size(12).color(if out {
                            p.bubble_out_meta
                        } else {
                            p.text_secondary
                        }),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    container(icon(Icon::Download, 18.0, p.text_secondary))
                        .padding(7)
                        .style(move |th: &iced::Theme| {
                            let mut s = theme::rounded(Color::TRANSPARENT, 20.0)(th);
                            s.border.width = 1.5;
                            s.border.color = p.text_secondary.scale_alpha(0.6);
                            s
                        }),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding([10, 10])
            .width(300)
            .style(move |th, st| {
                let mut s = theme::flat_button(fg)(th, st);
                s.background = Some(iced::Background::Color(inset));
                s
            })
            .on_press(Message::Ui(Ui::OpenDocument(msg.id.clone())));
            let mut c = column![card].spacing(4);
            if let Some(cap) = caption {
                c = c.push(text(cap.as_str()).size(m.text).color(fg));
            }
            c.push(container(meta_row(t, msg)).align_right(Length::Fill)).into()
        }
        Content::Voice {
            seconds, waveform, ..
        } => voice(app, msg, *seconds, waveform),
        Content::Unsupported(kind) => column![
            text(format!("This message type ({kind}) isn't supported yet. Open WhatsApp on your phone to view it."))
                .size(m.text)
                .color(p.text_secondary),
            container(meta_row(t, msg)).align_right(Length::Fill)
        ]
        .into(),
        Content::ViewOnce => row![
            icon(Icon::CircleDashed, 16.0, p.text_secondary),
            text("View once message: open it on your phone")
                .size(m.text)
                .color(p.text_secondary),
            gap(8.0),
            meta_row(t, msg),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
        Content::Deleted => row![
            icon(Icon::CircleAlert, 15.0, p.text_secondary),
            text(if out { "You deleted this message" } else { "This message was deleted" })
                .size(m.text)
                .color(p.text_secondary),
            gap(8.0),
            meta_row(t, msg),
        ]
        .spacing(5)
        .align_y(Alignment::Center)
        .into(),
    };
    finish(app, msg, first, col.push(body).into())
}

/// Wraps content in the bubble chrome, tail and alignment.
fn finish<'a>(app: &'a App, msg: &'a ChatMessage, first: bool, content: El<'a>) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let out = msg.from_me;
    let color = if out { p.bubble_out } else { p.bubble_in };
    let mut b = container(content)
        .padding(iced::Padding {
            top: 6.0,
            right: 8.0,
            bottom: 5.0,
            left: 9.0,
        })
        .max_width(t.metrics.bubble_max)
        .style(theme::bubble(t, out, first));
    if matches!(msg.content, Content::Image { .. }) {
        b = b.padding(3);
    }
    let bubble: El<'a> = if msg.status == ReceiptStatus::Failed && out {
        mouse_area(b)
            .on_press(Message::Ui(Ui::Retry(msg.id.clone())))
            .into()
    } else {
        b.into()
    };
    let tail_or_gap: El<'a> = if first { tail(color, out) } else { gap(8.0) };
    let line = if out {
        row![fill(), bubble, tail_or_gap]
    } else {
        row![tail_or_gap, bubble, fill()]
    };
    container(line.align_y(Alignment::Start))
        .padding(iced::Padding {
            top: if first { 6.0 } else { 0.0 },
            ..Default::default()
        })
        .into()
}

fn voice<'a>(app: &'a App, msg: &'a ChatMessage, seconds: u32, wf: &'a [u8]) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let out = msg.from_me;
    let state = app.voice.as_ref().filter(|v| v.id == msg.id);
    let playing = state.is_some_and(|v| !v.paused && !v.loading);
    let loading = state.is_some_and(|v| v.loading);
    let progress = state
        .filter(|v| v.duration_ms > 0)
        .map(|v| v.position_ms as f32 / v.duration_ms as f32)
        .unwrap_or(0.0);

    let btn_icon = if playing { Icon::Pause } else { Icon::Play };
    let play = button(icon(btn_icon, 22.0, p.text_secondary))
        .padding(6)
        .style(theme::flat_button(p.text_secondary))
        .on_press(Message::Ui(if playing {
            Ui::PauseVoice
        } else {
            Ui::PlayVoice(msg.id.clone())
        }));

    // 40 bars resampled from the 64-sample waveform.
    const BARS: usize = 40;
    let samples: Vec<u8> = if wf.is_empty() {
        vec![20; BARS]
    } else {
        (0..BARS).map(|i| wf[i * wf.len() / BARS]).collect()
    };
    let played_bars = (progress * BARS as f32).round() as usize;
    let played_color = if out {
        Color::from_rgb8(0xe9, 0xed, 0xef)
    } else {
        p.success
    };
    let idle_color = if out { p.bubble_out_meta } else { p.text_muted };
    let bars = row(samples.iter().enumerate().map(|(i, v)| {
        let h = 3.0 + (*v as f32 / 100.0) * 22.0;
        container(Space::new().width(2.5).height(h))
            .style(theme::rounded(
                if i < played_bars {
                    played_color
                } else {
                    idle_color
                },
                1.5,
            ))
            .into()
    }))
    .spacing(1.5)
    .align_y(Alignment::Center)
    .height(26);

    let left = state
        .filter(|v| v.position_ms > 0)
        .map(|v| format_duration((v.position_ms / 1000) as u32))
        .unwrap_or_else(|| format_duration(seconds));
    let meta_color = if out {
        p.bubble_out_meta
    } else {
        p.text_secondary
    };
    let mic_badge = container(icon(
        Icon::Mic,
        14.0,
        if msg.status == ReceiptStatus::Played || !out {
            p.success
        } else {
            p.text_secondary
        },
    ))
    .padding(2);
    let avatar_part: El<'a> = iced::widget::stack![
        avatar(t, false, 46.0),
        container(mic_badge)
            .width(46)
            .height(46)
            .align_x(Alignment::End)
            .align_y(Alignment::End)
    ]
    .into();

    let player = column![
        row![play, bars].spacing(4).align_y(Alignment::Center),
        row![
            gap(38.0),
            text(if loading {
                "Loading…".to_string()
            } else {
                left
            })
            .size(t.metrics.text_tiny)
            .color(meta_color),
            fill(),
            meta_row(t, msg)
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(0)
    .width(280);

    if out {
        row![player, avatar_part]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
    } else {
        row![avatar_part, player]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
    }
}

fn doc_color(ext: &str) -> Color {
    match ext {
        "PDF" => Color::from_rgb8(0xe3, 0x4c, 0x3d),
        "DOC" | "DOCX" | "ODT" => Color::from_rgb8(0x3d, 0x7b, 0xe3),
        "XLS" | "XLSX" | "ODS" | "CSV" => Color::from_rgb8(0x2f, 0xa5, 0x5d),
        _ => Color::from_rgb8(0x79, 0x86, 0x8f),
    }
}

/// Scales `(w, h)` to fit in `(max_w, max_h)`, keeping a sane minimum.
pub fn fit(w: u32, h: u32, max_w: f32, max_h: f32) -> (f32, f32) {
    if w == 0 || h == 0 {
        return (max_w, max_w * 0.66);
    }
    let (w, h) = (w as f32, h as f32);
    let s = (max_w / w).min(max_h / h).clamp(0.01, 1.0);
    ((w * s).max(120.0), (h * s).max(80.0))
}

/// Full-size image viewer overlay.
pub fn viewer<'a>(app: &'a App, id: &'a str) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let msg = app.open.as_ref().and_then(|c| c.get(id));
    let handle = msg
        .and_then(|m| m.content.media())
        .and_then(|r| app.images.peek(&r.cache_key()))
        .or_else(|| app.thumbs.get(id))
        .cloned();
    let content: El<'a> = match handle {
        Some(h) => image(h).content_fit(ContentFit::Contain).into(),
        None => text("Loading…").size(16).color(p.text_secondary).into(),
    };
    opaque(
        mouse_area(
            container(column![
                row![
                    fill(),
                    button(icon(Icon::X, 24.0, p.text_secondary))
                        .padding(10)
                        .style(theme::icon_button(t, false))
                        .on_press(Message::Ui(Ui::CloseViewer)),
                ]
                .padding(10),
                center(content).padding(40),
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::solid(p.bg_app.scale_alpha(0.96))),
        )
        .on_press(Message::Ui(Ui::CloseViewer)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_bounds() {
        let (w, h) = fit(1280, 640, 330.0, 330.0);
        assert!((w - 330.0).abs() < 0.1);
        assert!((h - 165.0).abs() < 0.1);
        let (w, _) = fit(10, 10, 330.0, 330.0);
        assert_eq!(w, 120.0);
        assert_eq!(fit(0, 0, 300.0, 300.0).0, 300.0);
    }
}
