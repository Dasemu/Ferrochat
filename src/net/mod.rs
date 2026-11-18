//! Protocol client. Owns sockets, never touches UI types.
//!
//! The app talks to WhatsApp only through the [`WaClient`] trait and the
//! [`Event`] stream. Two implementations exist:
//!
//! * [`whatsapp::RealClient`]: `whatsapp-rust` (pure Rust multi-device
//!   protocol), with its Signal store in a SQLCipher database.
//! * [`fake::FakeClient`]: an in-process scripted fake used by `--mock`,
//!   screenshot mode and the integration tests.

pub mod convert;
pub mod fake;
pub mod types;
pub mod whatsapp;

use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::error::Result;
pub use types::*;

/// Shared handle to the active protocol client.
pub type ClientHandle = Arc<dyn WaClient>;

/// Operations the app can ask of the protocol layer.
pub trait WaClient: Send + Sync {
    /// Sends a text message with a caller-chosen id.
    fn send_text<'a>(
        &'a self,
        chat: &'a str,
        id: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
    /// Uploads media and sends it. Returns the content as sent (with the CDN
    /// reference filled in).
    fn send_media<'a>(
        &'a self,
        chat: &'a str,
        id: &'a str,
        media: OutgoingMedia,
    ) -> BoxFuture<'a, Result<Content>>;
    /// Downloads and decrypts a media blob.
    fn download<'a>(&'a self, media: &'a MediaRef) -> BoxFuture<'a, Result<Vec<u8>>>;
    /// Sends read receipts. `sender` is required for group messages.
    fn mark_read<'a>(
        &'a self,
        chat: &'a str,
        sender: Option<&'a str>,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>>;
    /// Sends a "played" receipt for a voice note.
    fn mark_played<'a>(
        &'a self,
        chat: &'a str,
        sender: Option<&'a str>,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>>;
    /// Unlinks this device, disconnects and deletes the session store.
    fn logout(&self) -> BoxFuture<'_, Result<()>>;
    /// `--mock` only: pretend the phone scanned the QR code. The live client
    /// ignores it.
    fn simulate_scan(&self) {}
}

/// Connection state shown in the banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    /// Not connected, not trying.
    Disconnected,
    /// First connection attempt.
    Connecting,
    /// Online.
    Connected,
    /// Lost the connection, retrying with backoff.
    Reconnecting {
        /// Attempt number, from 1.
        attempt: u32,
    },
}

/// Events pushed by the protocol client.
#[derive(Clone)]
pub enum Event {
    /// The client is constructed; keep the handle to issue commands.
    Ready(ClientHandle),
    /// A stored session was found and is being restored.
    SessionFound,
    /// Not paired; QR codes will follow.
    NoSession,
    /// New QR payload to display.
    Qr {
        /// Payload.
        code: String,
        /// Validity in seconds.
        valid_for_secs: u64,
    },
    /// No more QR codes will come; the app restarts the client to get new ones.
    QrExhausted,
    /// The phone accepted the link.
    PairSuccess {
        /// Own JID.
        jid: String,
    },
    /// Pairing failed.
    PairError(String),
    /// Connection state changed.
    Connection(Connection),
    /// Connected with a valid session.
    Connected {
        /// Own JID.
        jid: Option<String>,
    },
    /// Session revoked / device unlinked.
    LoggedOut {
        /// Reason, user-facing.
        reason: String,
    },
    /// New or historical messages.
    Messages {
        /// Messages in arrival order.
        messages: Vec<Message>,
        /// True for live traffic (may notify), false for history/offline.
        live: bool,
    },
    /// History sync chunk: chat metadata and names.
    History {
        /// Chats.
        chats: Vec<ChatMeta>,
        /// Contact names (`jid`, `name`).
        names: Vec<(String, String)>,
        /// Sync progress 0..=100 if known.
        progress: Option<u32>,
    },
    /// Delivery / read receipt for outgoing messages.
    Receipt {
        /// Chat.
        chat: ChatId,
        /// Affected message ids.
        ids: Vec<String>,
        /// New status.
        status: ReceiptStatus,
    },
    /// A contact's display name changed / became known.
    ContactName {
        /// JID.
        jid: String,
        /// Name.
        name: String,
    },
    /// Chat marked read on another device.
    ChatRead {
        /// Chat.
        chat: ChatId,
    },
    /// Unrecoverable failure (keyring missing, database unreadable).
    Fatal(String),
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redacting Debug: no QR payloads, bodies or names.
        match self {
            Event::Ready(_) => write!(f, "Ready"),
            Event::SessionFound => write!(f, "SessionFound"),
            Event::NoSession => write!(f, "NoSession"),
            Event::Qr { valid_for_secs, .. } => write!(f, "Qr(valid={valid_for_secs}s)"),
            Event::QrExhausted => write!(f, "QrExhausted"),
            Event::PairSuccess { .. } => write!(f, "PairSuccess"),
            Event::PairError(_) => write!(f, "PairError"),
            Event::Connection(c) => write!(f, "Connection({c:?})"),
            Event::Connected { .. } => write!(f, "Connected"),
            Event::LoggedOut { .. } => write!(f, "LoggedOut"),
            Event::Messages { messages, live } => {
                write!(f, "Messages(n={}, live={live})", messages.len())
            }
            Event::History {
                chats,
                names,
                progress,
            } => write!(
                f,
                "History(chats={}, names={}, progress={progress:?})",
                chats.len(),
                names.len()
            ),
            Event::Receipt { ids, status, .. } => {
                write!(f, "Receipt(n={}, {status:?})", ids.len())
            }
            Event::ContactName { .. } => write!(f, "ContactName"),
            Event::ChatRead { .. } => write!(f, "ChatRead"),
            Event::Fatal(_) => write!(f, "Fatal"),
        }
    }
}

/// Generates a message id in the same shape WhatsApp Web uses
/// (`3EB0` + 18 upper-case hex chars).
pub fn new_message_id() -> String {
    let mut raw = [0u8; 9];
    if getrandom::fill(&mut raw).is_err() {
        // Extremely unlikely; fall back to the clock so ids stay unique.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        raw.copy_from_slice(&nanos.to_be_bytes()[7..16]);
    }
    format!("3EB0{}", hex::encode_upper(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ids_look_like_web_ids() {
        let a = new_message_id();
        let b = new_message_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 22);
        assert!(a.starts_with("3EB0"));
    }

    #[test]
    fn event_debug_is_redacted() {
        let e = Event::Qr {
            code: "2@secretpayload".into(),
            valid_for_secs: 20,
        };
        assert!(!format!("{e:?}").contains("secret"));
    }
}
