//! Update banner (top of the window) and the settings "Updates" section.

use iced::widget::{Space, button, column, container, row, text, toggler};
use iced::{Alignment, Font, Length};

use super::icons::Icon;
use super::theme::{self, INTER_MEDIUM};
use super::{El, icon};
use crate::app::{App, Message};
use crate::update::method::cargo_command;
use crate::update::{
    APP_NAME, Banner, CheckState, InstallMethod, Phase, Stage, UpdateMsg, Version,
};

fn msg(m: UpdateMsg) -> Message {
    Message::Update(m)
}

/// Text of a running install stage.
pub fn stage_label(version: Version, stage: Stage) -> String {
    match stage {
        Stage::Downloading {
            received,
            total: Some(t),
        } if t > 0 => format!(
            "Downloading {APP_NAME} {version}… {}%",
            (received.saturating_mul(100) / t).min(100)
        ),
        Stage::Downloading { received, .. } => format!(
            "Downloading {APP_NAME} {version}… {:.1} MB",
            received as f64 / 1_048_576.0
        ),
        Stage::Verifying => "Verifying the download…".into(),
        Stage::Installing => format!("Installing {APP_NAME} {version}…"),
    }
}

/// Result line under "Check for updates".
pub fn check_label(app: &App) -> String {
    let u = &app.updater;
    if let Some(r) = u.available() {
        return format!("{APP_NAME} {} is available.", r.version);
    }
    match &u.check {
        CheckState::Idle => "Not checked yet.".into(),
        CheckState::Checking { .. } => "Checking…".into(),
        CheckState::UpToDate => format!("{APP_NAME} is up to date."),
        CheckState::Failed(e) => format!("Could not check for updates: {e}"),
        CheckState::RateLimited => {
            "GitHub is limiting update checks right now; try again later.".into()
        }
        CheckState::Disabled => {
            "Update checks are off in this mode (mock or development build).".into()
        }
    }
}

fn small_primary<'a>(app: &'a App, label: &'a str, m: UpdateMsg) -> El<'a> {
    button(text(label).size(13).font(INTER_MEDIUM))
        .on_press(msg(m))
        .style(theme::primary_button(&app.theme))
        .padding([5, 14])
        .into()
}

fn link<'a>(app: &'a App, label: &'a str, m: UpdateMsg) -> El<'a> {
    button(text(label).size(13))
        .on_press(msg(m))
        .style(theme::link_button(&app.theme))
        .padding([5, 8])
        .into()
}

/// The slim bar above everything, when there is something to say.
pub fn banner(app: &App) -> Option<El<'_>> {
    let b = app.updater.banner()?;
    let t = &app.theme;
    let p = t.palette;
    let mut r = row![].spacing(10).align_y(Alignment::Center);
    match b {
        Banner::Available(v) => {
            r = r.push(icon(Icon::Download, 18.0, p.accent)).push(
                text(format!("{APP_NAME} {v} is available"))
                    .size(14)
                    .font(INTER_MEDIUM)
                    .color(p.text_primary),
            );
            r = match &app.updater.method {
                InstallMethod::Script(_) => {
                    r.push(small_primary(app, "Update", UpdateMsg::Install))
                }
                InstallMethod::Cargo => r
                    .push(
                        text(format!("Installed with cargo: {}", cargo_command()))
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(p.text_secondary),
                    )
                    .push(small_primary(app, "Copy command", UpdateMsg::CopyCommand)),
                InstallMethod::Unmanaged(_) => {
                    r.push(small_primary(app, "Download", UpdateMsg::OpenReleasePage))
                }
            };
            r = r.push(link(app, "Release notes", UpdateMsg::OpenReleasePage));
        }
        Banner::Progress(v, stage) => {
            r = r
                .push(icon(Icon::Download, 18.0, p.accent))
                .push(text(stage_label(v, stage)).size(14).color(p.text_primary));
            if app.updater.cancellable() {
                r = r.push(link(app, "Cancel", UpdateMsg::Cancel));
            }
        }
        Banner::Restart(v) => {
            r = r
                .push(icon(Icon::RefreshCw, 18.0, p.accent))
                .push(
                    text(format!("{APP_NAME} {v} is installed. Restart to finish."))
                        .size(14)
                        .font(INTER_MEDIUM)
                        .color(p.text_primary),
                )
                .push(small_primary(app, "Restart now", UpdateMsg::Restart))
                .push(link(app, "Later", UpdateMsg::Later));
        }
        Banner::Failed(e) => {
            r = r
                .push(icon(Icon::CircleAlert, 18.0, p.danger))
                .push(
                    text(format!("Update failed: {e}"))
                        .size(14)
                        .color(p.text_primary),
                )
                .push(link(app, "Open release page", UpdateMsg::OpenReleasePage));
        }
    }
    let closable = matches!(
        app.updater.banner(),
        Some(Banner::Available(_) | Banner::Failed(_))
    );
    let close: El<'_> = if closable {
        button(icon(Icon::X, 16.0, p.text_secondary))
            .on_press(msg(UpdateMsg::Dismiss))
            .style(theme::icon_button(t, false))
            .padding(4)
            .into()
    } else {
        Space::new().width(24).into()
    };
    Some(
        container(
            row![r.width(Length::Fill), close]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .padding([0, 16])
        .height(44)
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .style(theme::bordered(p.bg_elevated, p.border))
        .into(),
    )
}

fn card<'a>(app: &'a App, body: El<'a>) -> El<'a> {
    container(body)
        .padding([12, 20])
        .width(Length::Fill)
        .style(theme::rounded(app.theme.palette.bg_sidebar, 8.0))
        .into()
}

fn lines<'a>(app: &'a App, title: String, detail: String) -> El<'a> {
    let p = app.theme.palette;
    column![
        text(title).size(15).color(p.text_primary),
        text(detail).size(13).color(p.text_secondary),
    ]
    .spacing(3)
    .width(Length::Fill)
    .into()
}

/// The "Updates" section of the settings page.
pub fn section(app: &App) -> El<'_> {
    let t = &app.theme;
    let p = t.palette;
    let u = &app.updater;
    let mut col = column![
        container(text("Updates").size(14).font(INTER_MEDIUM).color(p.accent)).padding([8, 20])
    ]
    .spacing(2);

    col = col.push(card(
        app,
        row![
            lines(
                app,
                format!("{APP_NAME} {}", u.current),
                format!("Installed with: {}", u.method.label()),
            ),
            link(app, "Release notes", UpdateMsg::OpenReleasePage),
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .into(),
    ));

    let checking = matches!(u.check, CheckState::Checking { .. });
    let check_button = button(
        text(if checking {
            "Checking…"
        } else {
            "Check for updates"
        })
        .size(13),
    )
    .on_press_maybe((!checking).then(|| msg(UpdateMsg::CheckNow)))
    .style(theme::primary_button(t))
    .padding([6, 14]);
    col = col.push(card(
        app,
        row![
            lines(app, "Check for updates".into(), check_label(app)),
            check_button
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .into(),
    ));

    // What can be done about an available update, or the install's state.
    let action: Option<El<'_>> = match &u.phase {
        Phase::Running { version, stage } => {
            let mut r = row![lines(
                app,
                stage_label(*version, *stage),
                "The current version keeps running until you restart.".into(),
            )]
            .spacing(16)
            .align_y(Alignment::Center);
            if u.cancellable() {
                r = r.push(link(app, "Cancel", UpdateMsg::Cancel));
            }
            Some(r.into())
        }
        Phase::Installed(v) => Some(
            row![
                lines(app, format!("{APP_NAME} {v} is installed"), "Restart to finish the update.".into()),
                small_primary(app, "Restart now", UpdateMsg::Restart),
            ]
            .spacing(16)
            .align_y(Alignment::Center)
            .into(),
        ),
        Phase::Failed(e) => Some(
            row![
                lines(app, "The update failed".into(), e.clone()),
                link(app, "Open release page", UpdateMsg::OpenReleasePage),
            ]
            .spacing(16)
            .align_y(Alignment::Center)
            .into(),
        ),
        Phase::Idle => u.available().map(|r| {
            let (detail, button): (String, El<'_>) = match &u.method {
                InstallMethod::Script(_) => (
                    "Downloads the release, checks it and installs it in place.".into(),
                    small_primary(app, "Update now", UpdateMsg::Install),
                ),
                InstallMethod::Cargo => (
                    format!("Installed with cargo. Run: {}", cargo_command()),
                    small_primary(app, "Copy command", UpdateMsg::CopyCommand),
                ),
                InstallMethod::Unmanaged(_) => (
                    "This copy cannot update itself; download the new version from the release page.".into(),
                    small_primary(app, "Open release page", UpdateMsg::OpenReleasePage),
                ),
            };
            row![lines(app, format!("{APP_NAME} {} is available", r.version), detail), button]
                .spacing(16)
                .align_y(Alignment::Center)
                .into()
        }),
    };
    if let Some(a) = action {
        col = col.push(card(app, a));
    }

    col = col.push(card(
        app,
        row![
            lines(
                app,
                "Check for updates automatically".into(),
                "At start and every few hours. Only the latest release is asked for; nothing about you is sent.".into(),
            ),
            toggler(app.settings.check_updates)
                .on_toggle(|_| msg(UpdateMsg::ToggleAuto))
                .style(theme::toggler(t))
                .size(22),
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .into(),
    ));
    col.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_labels() {
        let v = Version::new(1, 2, 3);
        assert_eq!(
            stage_label(
                v,
                Stage::Downloading {
                    received: 50,
                    total: Some(200)
                }
            ),
            "Downloading Ferrochat 1.2.3… 25%"
        );
        assert_eq!(
            stage_label(
                v,
                Stage::Downloading {
                    received: 1_048_576,
                    total: None
                }
            ),
            "Downloading Ferrochat 1.2.3… 1.0 MB"
        );
        assert_eq!(stage_label(v, Stage::Verifying), "Verifying the download…");
        assert_eq!(
            stage_label(v, Stage::Installing),
            "Installing Ferrochat 1.2.3…"
        );
    }
}
