//! Settings page: background (tray) and notification toggles.

use iced::widget::{Space, button, column, container, row, scrollable, text, toggler};
use iced::{Alignment, Length};

use super::icons::Icon;
use super::theme::{self, INTER_MEDIUM, INTER_SEMIBOLD};
use super::{El, icon};
use crate::app::{App, Message, Setting, Ui};

/// Id of the settings page scrollable.
pub const SCROLL_ID: &str = "settings";

/// The settings page, shown in place of the conversation.
pub fn view(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let header = container(
        row![
            button(icon(Icon::ArrowLeft, 20.0, p.text_secondary))
                .on_press(Message::Ui(Ui::CloseSettings))
                .style(theme::icon_button(t, false))
                .padding(8),
            text("Settings")
                .size(19)
                .font(INTER_SEMIBOLD)
                .color(p.text_primary),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([0, 16])
    .height(t.metrics.header_height)
    .width(Length::Fill)
    .align_y(Alignment::Center)
    .style(theme::solid(p.bg_elevated));

    let mut background = column![section(app, "Background")].spacing(2);
    background = background
        .push(toggle_row(
            app,
            "Keep running in the system tray",
            "Closing the window hides it; Ferrochat stays connected and keeps notifying you.",
            app.settings.close_to_tray,
            Setting::CloseToTray,
        ))
        .push(toggle_row(
            app,
            "Start minimized to tray",
            "Launch without a window. Click the tray icon to open it.",
            app.settings.start_minimized,
            Setting::StartMinimized,
        ))
        .push(toggle_row(
            app,
            "Start Ferrochat at login",
            "Adds an autostart entry that launches Ferrochat minimized.",
            app.autostart,
            Setting::Autostart,
        ));
    if app.tray_checked && !app.tray_up() {
        background = background.push(
            container(
                text("No system tray was found on this desktop, so closing the window quits.")
                    .size(13)
                    .color(p.notice_text),
            )
            .padding([8, 20]),
        );
    }

    let notifications = column![
        section(app, "Notifications"),
        toggle_row(
            app,
            "Mute notifications",
            "No desktop notifications for new messages. Unread counts still update.",
            app.settings.muted,
            Setting::Muted,
        ),
    ]
    .spacing(2);

    let body = container(
        column![
            background,
            Space::new().height(16),
            notifications,
            Space::new().height(16),
            super::update::section(app),
        ]
        .width(Length::Fill)
        .max_width(620),
    )
    .padding([20, 24])
    .center_x(Length::Fill);

    // A stable id keeps the scroll position across redraws.
    let body = scrollable(body)
        .id(SCROLL_ID)
        .direction(super::thin_scrollbar())
        .style(theme::scrollbar(t))
        .height(Length::Fill);
    container(column![header, body])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::solid(p.bg_panel))
        .into()
}

fn section<'a>(app: &'a App, title: &'static str) -> El<'a> {
    container(
        text(title)
            .size(14)
            .font(INTER_MEDIUM)
            .color(app.theme.palette.accent),
    )
    .padding([8, 20])
    .into()
}

fn toggle_row<'a>(
    app: &'a App,
    title: &'static str,
    detail: &'static str,
    on: bool,
    setting: Setting,
) -> El<'a> {
    let t = &app.theme;
    let p = t.palette;
    container(
        row![
            column![
                text(title).size(15).color(p.text_primary),
                text(detail).size(13).color(p.text_secondary),
            ]
            .spacing(3)
            .width(Length::Fill),
            toggler(on)
                .on_toggle(move |_| Message::Ui(Ui::Toggle(setting)))
                .style(theme::toggler(t))
                .size(22),
        ]
        .spacing(16)
        .align_y(Alignment::Center),
    )
    .padding([12, 20])
    .style(theme::rounded(p.bg_sidebar, 8.0))
    .into()
}
