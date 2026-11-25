//! Quitting must not wait for open notifications.
//!
//! KDE Plasma keeps notifications with actions in its history and sends
//! `NotificationClosed` only when the user dismisses them, so waiting for a
//! click can last forever. That wait used to run on tokio's blocking pool,
//! and iced drops its tokio runtime at quit, which waits for every blocking
//! task: the app hung on "Quit" with its window frozen.
//!
//! Needs a bus: `dbus-run-session -- cargo test --test notify_quit`
//! (skipped without `DBUS_SESSION_BUS_ADDRESS`).
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use zbus::zvariant::OwnedValue;

/// A notification server that, like Plasma's history, never closes
/// anything by itself.
struct NeverCloses {
    shown: Arc<AtomicU32>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NeverCloses {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: &str,
        _replaces_id: u32,
        _app_icon: &str,
        _summary: &str,
        _body: &str,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        self.shown.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn close_notification(&self, _id: u32) {}

    fn get_capabilities(&self) -> Vec<String> {
        vec!["actions".into(), "body".into(), "persistence".into()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("Plasma".into(), "KDE".into(), "5.27".into(), "1.2".into())
    }
}

#[test]
fn quitting_does_not_wait_for_open_notifications() {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        eprintln!("skipped: no D-Bus session bus (use dbus-run-session)");
        return;
    }
    let shown = Arc::new(AtomicU32::new(0));
    let server = zbus::blocking::connection::Builder::session()
        .unwrap()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            NeverCloses {
                shown: shown.clone(),
            },
        )
        .unwrap()
        .build()
        .unwrap();

    // The app's runtime, used the way the app uses it: `notify::show` with a
    // click action runs on the blocking pool. Then quit: iced drops the
    // runtime. All of it runs on a helper thread so a regression fails the
    // test instead of hanging it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let shown = rt.block_on(async {
            let task = tokio::task::spawn_blocking(|| {
                ferrochat::notify::show("Ferrochat", "hi", Some(Box::new(|| {})));
            });
            tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .is_ok()
        });
        let _ = tx.send(("show returned", shown));
        let start = Instant::now();
        drop(rt);
        let _ = tx.send(("runtime dropped", start.elapsed() < Duration::from_secs(1)));
    });
    let (step, ok) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(step, "show returned");
    assert!(ok, "notify::show must return once the notification is up");
    assert_eq!(
        shown.load(Ordering::SeqCst),
        1,
        "the notification was shown"
    );
    let (step, ok) = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the runtime shuts down while a notification is still open");
    assert_eq!(step, "runtime dropped");
    assert!(ok, "dropping the runtime took too long");
    drop(server);
}
