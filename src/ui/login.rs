//! QR pairing screen ("link a device").

use iced::widget::{Space, button, center, column, container, image, row, text};
use iced::{Alignment, Length};

use super::icons::Icon;
use super::theme::{self, INTER_MEDIUM};
use super::{El, icon};
use crate::app::{App, Message, Ui};
use crate::auth::AuthState;

/// The login screen.
pub fn view(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;

    let step = |n: u32, s: &'static str| -> El<'_> {
        row![
            container(text(n.to_string()).size(14).color(p.text_primary))
                .width(26)
                .height(26)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |th: &iced::Theme| {
                    let mut st = theme::rounded(p.bg_input, 13.0)(th);
                    st.border.width = 1.0;
                    st.border.color = p.text_muted;
                    st
                }),
            text(s).size(17).color(p.text_primary),
        ]
        .spacing(14)
        .align_y(Alignment::Center)
        .into()
    };

    let steps = column![
        text("Steps to log in")
            .size(28)
            .font(theme::INTER)
            .color(p.text_primary),
        Space::new().height(18),
        step(1, "Open WhatsApp on your phone"),
        step(2, "Tap Menu or Settings and select Linked devices"),
        step(3, "Tap Link a device"),
        step(4, "Point your phone at this screen to scan the QR code"),
        Space::new().height(30),
        row![
            icon(Icon::CircleAlert, 16.0, p.notice_text),
            text(
                "Ferrochat is an unofficial client. Third-party clients break WhatsApp's \
                 terms of service and can get your account banned."
            )
            .size(13)
            .color(p.text_secondary),
        ]
        .spacing(8)
        .width(420),
    ]
    .spacing(22)
    .width(Length::Fill);

    let qr_side = 264.0;
    let qr: El<'_> = match (&app.auth, &app.qr) {
        (AuthState::ShowingQr { .. }, Some(h)) => {
            let img = image(h.clone()).width(qr_side).height(qr_side);
            if app.cfg.mock {
                button(img)
                    .padding(0)
                    .style(theme::flat_button(p.text_primary))
                    .on_press(Message::Ui(Ui::SimulateScan))
                    .into()
            } else {
                img.into()
            }
        }
        (AuthState::QrExpired, _) | (AuthState::LoggedOut { .. }, _) => {
            qr_placeholder(app, qr_side, true)
        }
        (AuthState::Failed { message }, _) => container(
            column![
                icon(Icon::CircleAlert, 40.0, p.danger),
                text(message.as_str())
                    .size(14)
                    .color(p.text_primary)
                    .align_x(Alignment::Center),
                button(text("Try again").size(14).font(INTER_MEDIUM))
                    .padding([8, 20])
                    .style(theme::primary_button(t))
                    .on_press(Message::Ui(Ui::ReloadQr)),
                reset_button(app),
            ]
            .spacing(14)
            .align_x(Alignment::Center),
        )
        .width(qr_side + 80.0)
        .height(qr_side + 60.0)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into(),
        _ => qr_placeholder(app, qr_side, false),
    };

    let status: El<'_> = match &app.auth {
        AuthState::ShowingQr {
            valid_for_secs,
            generation,
            ..
        } => text(format!(
            "Code {generation} · refreshes every {valid_for_secs}s"
        ))
        .size(13)
        .color(p.text_secondary)
        .into(),
        AuthState::Restoring | AuthState::Pairing => {
            text("Logging in…").size(13).color(p.text_secondary).into()
        }
        AuthState::LoggedOut { reason } => {
            text(reason.as_str()).size(13).color(p.notice_text).into()
        }
        _ => Space::new().height(16).into(),
    };

    let card = container(
        column![
            row![
                steps,
                column![qr, status]
                    .spacing(14)
                    .align_x(Alignment::Center)
                    .width(qr_side + 40.0),
            ]
            .spacing(40)
            .align_y(Alignment::Center),
        ]
        .padding([56, 64]),
    )
    .max_width(1000)
    .style(theme::rounded(p.bg_sidebar, 24.0));

    let brand = row![
        container(icon(Icon::MessageCircle, 22.0, p.accent_text))
            .padding(5)
            .style(theme::rounded(p.accent, 8.0)),
        text("Ferrochat")
            .size(18)
            .font(INTER_MEDIUM)
            .color(p.text_primary),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let footer = row![
        icon(Icon::Lock, 13.0, p.text_secondary),
        text("Your personal messages are end-to-end encrypted")
            .size(13)
            .color(p.text_secondary),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(
        column![
            container(brand).padding([24, 40]).width(Length::Fill),
            center(card).padding([0, 32]),
            container(footer).center_x(Length::Fill).padding(24),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::solid(p.bg_app))
    .into()
}

fn qr_placeholder(app: &App, side: f32, expired: bool) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let inner: El<'_> = if expired {
        button(
            column![
                container(icon(Icon::RefreshCw, 28.0, p.accent_text))
                    .padding(16)
                    .style(theme::rounded(p.accent, 40.0)),
                text("Click to reload QR code")
                    .size(14)
                    .color(p.text_primary)
                    .font(INTER_MEDIUM),
            ]
            .spacing(12)
            .align_x(Alignment::Center),
        )
        .style(theme::flat_button(p.text_primary))
        .on_press(Message::Ui(Ui::ReloadQr))
        .into()
    } else {
        text("Generating QR code…")
            .size(14)
            .color(p.text_secondary)
            .into()
    };
    container(inner)
        .width(side)
        .height(side)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(theme::rounded(p.bg_input, 8.0))
        .into()
}

/// Shown only when the saved session cannot be decrypted: discarding it is
/// the user's decision, never automatic.
fn reset_button(app: &App) -> El<'_> {
    let t = &app.theme;
    if !app.session_unreadable {
        return Space::new().into();
    }
    button(
        text("Discard saved session and link again")
            .size(13)
            .font(INTER_MEDIUM),
    )
    .padding([6, 4])
    .style(theme::link_button(t))
    .on_press(Message::Ui(Ui::ResetSession))
    .into()
}
