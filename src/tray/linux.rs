//! StatusNotifierItem service via `ksni` (zbus, no GTK).

use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::{Category, Icon, MenuItem, OfflineReason, Status, ToolTip, Tray, TrayMethods};

use super::{Attention, MenuEntry, TrayCmd, TrayModel};
use crate::config::APP_ID;
use crate::ext::{self, External};
use crate::icon;

struct FerroTray {
    model: TrayModel,
    normal: Vec<Icon>,
    attention: Vec<Icon>,
}

fn pixmaps(svg: &[u8]) -> Vec<Icon> {
    icon::TRAY_SIZES
        .iter()
        .filter_map(|&s| icon::render(svg, s))
        .map(|r| Icon {
            width: r.size as i32,
            height: r.size as i32,
            data: r.argb(),
        })
        .collect()
}

impl FerroTray {
    fn new(model: TrayModel) -> Self {
        Self {
            model,
            normal: pixmaps(icon::NORMAL_SVG),
            attention: pixmaps(icon::ATTENTION_SVG),
        }
    }
}

fn send(cmd: TrayCmd) {
    ext::send(External::Tray(cmd));
}

impl Tray for FerroTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "Ferrochat".into()
    }

    fn category(&self) -> Category {
        Category::Communications
    }

    fn status(&self) -> Status {
        match self.model.attention() {
            Attention::Idle => Status::Active,
            Attention::NeedsAttention => Status::NeedsAttention,
        }
    }

    // Left click toggles the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        send(TrayCmd::Toggle);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        send(TrayCmd::Toggle);
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.normal.clone()
    }

    fn attention_icon_pixmap(&self) -> Vec<Icon> {
        self.attention.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.model.tooltip(),
            icon_pixmap: match self.model.attention() {
                Attention::Idle => self.normal.clone(),
                Attention::NeedsAttention => self.attention.clone(),
            },
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.model
            .menu()
            .into_iter()
            .map(|e| match e {
                MenuEntry::Item { label, cmd } => StandardItem {
                    label: label.into(),
                    activate: Box::new(move |_: &mut Self| send(cmd)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::Check {
                    label,
                    checked,
                    cmd,
                } => CheckmarkItem {
                    label: label.into(),
                    checked,
                    activate: Box::new(move |_: &mut Self| send(cmd)),
                    ..Default::default()
                }
                .into(),
                MenuEntry::Separator => MenuItem::Separator,
            })
            .collect()
    }

    // The panel (re)appeared: the tray is usable again.
    fn watcher_online(&self) {
        ext::send(External::TrayUp(true));
    }

    // The panel went away: the app must not stay hidden. Keep the service
    // alive so the icon returns when a panel does.
    fn watcher_offline(&self, _reason: OfflineReason) -> bool {
        ext::send(External::TrayUp(false));
        true
    }
}

/// Registers the tray. Returns `false` when there is no StatusNotifierWatcher
/// / host on the session bus (so the app must not hide to the tray). Keeps
/// the tray in sync with [`super::publish`] in a background task.
pub async fn start(initial: TrayModel) -> bool {
    let mut rx = super::subscribe();
    let handle = match FerroTray::new(initial).spawn().await {
        Ok(h) => h,
        Err(e) => {
            tracing::info!(error = %e, "no system tray available");
            return false;
        }
    };
    tokio::spawn(async move {
        // Apply whatever changed while the service was starting.
        loop {
            let model = rx.borrow_and_update().clone();
            if handle.update(|t| t.model = model).await.is_none() {
                break;
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
    true
}
