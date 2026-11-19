//! Events that come from outside the iced runtime: tray clicks, a second
//! launch, a clicked notification. Producers (D-Bus callbacks, an accept
//! thread, notification waiters) push into one global channel; the app
//! drains it through a subscription.

use std::sync::{Mutex, OnceLock};

use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::tray::TrayCmd;

/// Something that happened outside the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum External {
    /// A tray menu entry or a click on the icon.
    Tray(TrayCmd),
    /// Show the window and focus this chat, if any (a second launch or a
    /// clicked notification).
    Show {
        /// Chat to open.
        chat: Option<String>,
    },
    /// The tray appeared (`true`) or disappeared (`false`).
    TrayUp(bool),
}

struct Bus {
    tx: UnboundedSender<External>,
    rx: Mutex<Option<UnboundedReceiver<External>>>,
}

fn bus() -> &'static Bus {
    static BUS: OnceLock<Bus> = OnceLock::new();
    BUS.get_or_init(|| {
        let (tx, rx) = unbounded_channel();
        Bus {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    })
}

/// Queues an event for the app. Never blocks; callable from any thread.
pub fn send(ev: External) {
    let _ = bus().tx.send(ev);
}

/// Takes the receiving end (once; later calls return `None`).
pub fn take_receiver() -> Option<UnboundedReceiver<External>> {
    bus().rx.lock().ok().and_then(|mut g| g.take())
}

/// The event stream for `Subscription::run`.
pub fn stream() -> futures::stream::BoxStream<'static, External> {
    match take_receiver() {
        Some(rx) => {
            futures::stream::unfold(
                rx,
                |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
            )
            .boxed()
        }
        None => futures::stream::pending().boxed(),
    }
}
