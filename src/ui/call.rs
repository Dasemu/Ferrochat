//! Call views: the call bubble in a chat and the call panel (incoming call
//! with Accept / Decline, outgoing / active call with mute and hang up).

use iced::widget::{Space, button, column, container, mouse_area, row, text};
use iced::{Alignment, Length};

use super::icons::Icon;
use super::{El, avatar_for, icon, theme};
use crate::app::App;
use crate::app::call::{CallUi, ui};
use crate::call::Phase;
use crate::net::{CallLog, Message as ChatMessage, is_group_jid};

/// The body of a call bubble: icon, "Missed voice call" / "Voice call",
/// the detail line and the time. A click calls back (voice calls).
pub fn log_body<'a>(app: &'a App, msg: &'a ChatMessage, log: &CallLog, meta: El<'a>) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let incoming = !msg.from_me;
    let missed = log.is_missed(incoming);
    let glyph = match (log.video, missed, incoming) {
        (true, _, _) => Icon::Video,
        (false, true, _) => Icon::PhoneMissed,
        (false, false, true) => Icon::PhoneIncoming,
        (false, false, false) => Icon::PhoneOutgoing,
    };
    let tint = if missed { p.danger } else { p.text_primary };
    let badge = container(icon(glyph, 18.0, tint))
        .width(36)
        .height(36)
        .center_x(36)
        .center_y(36)
        .style(theme::rounded(
            if incoming { p.inset_in } else { p.inset_out },
            18.0,
        ));
    let sub = if incoming {
        p.text_secondary
    } else {
        p.bubble_out_meta
    };
    let body = row![
        badge,
        column![
            text(log.title(incoming)).size(t.metrics.text).color(tint),
            text(log.detail(incoming)).size(12.5).color(sub),
        ]
        .spacing(1),
        Space::new().width(14),
        container(meta).align_bottom(Length::Shrink),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if log.group || is_group_jid(&msg.chat) {
        return body.into();
    }
    mouse_area(body)
        .interaction(iced::mouse::Interaction::Pointer)
        .on_press(ui(CallUi::Start {
            chat: msg.chat.clone(),
            video: log.video,
        }))
        .into()
}

/// The voice / video buttons of a chat header.
pub fn header_buttons<'a>(app: &'a App, chat: &str) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    let m = t.metrics;
    let busy = app.calls.busy();
    let btn = |glyph: Icon, video: bool| {
        let b = button(icon(glyph, m.icon, p.text_secondary))
            .padding(8)
            .style(theme::icon_button(t, false));
        if busy {
            b
        } else {
            b.on_press(ui(CallUi::Start {
                chat: chat.to_string(),
                video,
            }))
        }
    };
    row![btn(Icon::Video, true), btn(Icon::Phone, false)]
        .spacing(4)
        .into()
}

fn round<'a>(glyph: Icon, bg: iced::Color, msg: CallUi) -> El<'a> {
    button(
        container(icon(glyph, 22.0, iced::Color::WHITE))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(theme::CALL_BUTTON)
    .height(theme::CALL_BUTTON)
    .padding(0)
    .style(theme::call_button(bg))
    .on_press(ui(msg))
    .into()
}

/// The call panel, floating over the window while a call rings or runs.
pub fn panel(app: &App) -> Option<El<'_>> {
    let c = app.calls.current.as_ref()?;
    let t = &app.theme;
    let p = t.palette;
    let now = chrono::Utc::now().timestamp();
    let name = app.chats.plain_name(&c.peer);
    let mut col = column![
        avatar_for(app, &c.peer, false, theme::CALL_AVATAR),
        text(name).size(20).color(p.text_primary),
    ]
    .spacing(8)
    .align_x(Alignment::Center);
    if c.group {
        col = col.push(
            text(app.chats.plain_name(&c.chat))
                .size(13)
                .color(p.text_secondary),
        );
    }
    col = col.push(text(c.status(now)).size(14).color(p.text_secondary));
    let note = match c.phase {
        Phase::Incoming if c.group => {
            Some("Group calls are not supported yet: answer on your phone.")
        }
        Phase::Incoming if c.video => {
            Some("Video isn't supported yet: you'll answer with voice only.")
        }
        _ => None,
    };
    if let Some(n) = note {
        col = col.push(
            text(n)
                .size(12.5)
                .color(p.text_muted)
                .align_x(Alignment::Center),
        );
    }
    let buttons: El<'_> = match c.phase {
        Phase::Incoming => {
            let mut r = row![round(Icon::PhoneOff, p.danger, CallUi::Decline)].spacing(48);
            if c.can_answer() {
                r = r.push(round(Icon::Phone, p.accent, CallUi::Accept));
            }
            r.into()
        }
        _ => row![
            round(
                if c.muted { Icon::MicOff } else { Icon::Mic },
                if c.muted {
                    p.text_secondary
                } else {
                    p.bg_selected
                },
                CallUi::ToggleMute,
            ),
            round(Icon::PhoneOff, p.danger, CallUi::HangUp),
        ]
        .spacing(48)
        .into(),
    };
    col = col.push(Space::new().height(12)).push(buttons);
    let card = container(col)
        .width(theme::CALL_PANEL_WIDTH)
        .padding(24)
        .style(theme::bordered(p.bg_elevated, p.border));
    Some(
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Start)
            .padding(48)
            .into(),
    )
}
