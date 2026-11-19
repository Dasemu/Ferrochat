//! The StatusNotifierItem service against a fake StatusNotifierWatcher on a
//! private session bus. Run it under a bus:
//! `dbus-run-session -- cargo test --test tray_sni`
//! (without `DBUS_SESSION_BUS_ADDRESS` the test is skipped).
#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferrochat::ext::{self, External};
use ferrochat::tray::{self, TrayCmd, TrayModel};
use zbus::object_server::SignalEmitter;

#[derive(Clone, Default)]
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        if let Ok(mut v) = self.items.lock() {
            v.push(service.to_string());
        }
        let _ = Self::status_notifier_item_registered(&emitter, service).await;
    }

    async fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().map(|v| v.clone()).unwrap_or_default()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;
}

type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registers_exports_properties_and_forwards_clicks() {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        eprintln!("skipped: no session bus (run under dbus-run-session)");
        return;
    }
    let mut events = ext::take_receiver().expect("receiver");

    // 1. No StatusNotifierWatcher on the bus: the tray reports "unavailable",
    //    so the app will not hide to it.
    assert!(
        !tray::start(TrayModel::default()).await,
        "no watcher means no tray"
    );

    // 2. A fake watcher appears: the item registers and exports properties.
    let watcher = Watcher::default();
    let conn = zbus::connection::Builder::session()
        .expect("session bus")
        .name("org.kde.StatusNotifierWatcher")
        .expect("name")
        .serve_at("/StatusNotifierWatcher", watcher.clone())
        .expect("serve")
        .build()
        .await
        .expect("connect");

    let model = TrayModel {
        unread_chats: 0,
        window_visible: true,
        ..Default::default()
    };
    assert!(tray::start(model).await, "watcher present means tray up");

    let service = watcher
        .items
        .lock()
        .expect("lock")
        .first()
        .cloned()
        .expect("item registered");
    // SNI hosts re-read on the New* signals; do not let zbus cache properties.
    let item = zbus::proxy::Builder::<zbus::Proxy>::new(&conn)
        .destination(service.as_str())
        .expect("destination")
        .path("/StatusNotifierItem")
        .expect("path")
        .interface("org.kde.StatusNotifierItem")
        .expect("interface")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .expect("proxy");

    let id: String = item.get_property("Id").await.expect("Id");
    assert_eq!(id, "dev.ferro.ferrochat");
    let category: String = item.get_property("Category").await.expect("Category");
    assert_eq!(category, "Communications");
    let status: String = item.get_property("Status").await.expect("Status");
    assert_eq!(status, "Active");
    let icons: Vec<(i32, i32, Vec<u8>)> = item.get_property("IconPixmap").await.expect("pixmap");
    let sizes: Vec<i32> = icons.iter().map(|i| i.0).collect();
    assert_eq!(sizes, vec![22, 32, 48]);
    assert!(
        icons
            .iter()
            .all(|(w, h, d)| d.len() == (*w * *h * 4) as usize)
    );
    let tip: ToolTip = item.get_property("ToolTip").await.expect("ToolTip");
    assert_eq!(tip.2, "Ferrochat");

    // 3. Unread chats: NeedsAttention, tooltip with the count, attention icon.
    tray::publish(TrayModel {
        unread_chats: 3,
        window_visible: true,
        ..Default::default()
    });
    let mut status = String::new();
    for _ in 0..40 {
        status = item.get_property("Status").await.expect("Status");
        if status == "NeedsAttention" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, "NeedsAttention");
    let tip: ToolTip = item.get_property("ToolTip").await.expect("ToolTip");
    assert_eq!(tip.2, "Ferrochat \u{2014} 3 unread chats");
    let attn: Vec<(i32, i32, Vec<u8>)> = item
        .get_property("AttentionIconPixmap")
        .await
        .expect("attention pixmap");
    assert_eq!(attn.len(), 3);
    assert_ne!(attn[0].2, icons[0].2, "attention icon differs");

    // 4. Left click toggles the window.
    item.call_method("Activate", &(0i32, 0i32))
        .await
        .expect("Activate");
    let ev = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("event in time")
        .expect("open channel");
    assert_eq!(ev, External::Tray(TrayCmd::Toggle));
}
