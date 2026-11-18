//! In-process fake of the protocol layer.
//!
//! Drives `--mock` mode, screenshot mode and the integration tests. It
//! mimics the live client's event order (session probe → `Ready` → QR or
//! `Connected` → history) and its receipt behaviour (sent → delivered →
//! read after configurable delays).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::Stream;
use tokio::sync::mpsc;

use super::types::{Content, MediaRef, Message, OutgoingMedia, ReceiptStatus};
use super::{Connection, Event, WaClient};
use crate::error::{Error, Result};
use crate::mock::MockData;

/// Knobs for the fake.
#[derive(Debug, Clone)]
pub struct FakeOptions {
    /// Start with a session (skip the QR screen).
    pub paired: bool,
    /// Number of QR codes to rotate through before `QrExhausted`.
    pub qr_codes: u32,
    /// Interval between QR codes.
    pub qr_interval: Duration,
    /// Emit delivered/read receipts for sent messages.
    pub auto_receipts: bool,
    /// Delay before each receipt step.
    pub receipt_delay: Duration,
    /// The peer answers text messages.
    pub auto_reply: bool,
    /// Make every send fail (tests the failed-tick path).
    pub fail_sends: bool,
}

impl Default for FakeOptions {
    fn default() -> Self {
        Self {
            paired: true,
            qr_codes: 3,
            qr_interval: Duration::from_secs(20),
            auto_receipts: true,
            receipt_delay: Duration::from_millis(800),
            auto_reply: false,
            fail_sends: false,
        }
    }
}

/// A recorded outgoing action.
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// Text.
    Text {
        /// Chat.
        chat: String,
        /// Id.
        id: String,
        /// Body.
        text: String,
    },
    /// Media.
    Media {
        /// Chat.
        chat: String,
        /// Id.
        id: String,
        /// Resulting content.
        content: Content,
    },
    /// Read receipt.
    Read {
        /// Chat.
        chat: String,
        /// Ids.
        ids: Vec<String>,
    },
    /// Played receipt.
    Played {
        /// Chat.
        chat: String,
        /// Ids.
        ids: Vec<String>,
    },
    /// Logout.
    Logout,
}

/// The fake client.
pub struct FakeClient {
    tx: mpsc::UnboundedSender<Event>,
    opts: FakeOptions,
    own_jid: String,
    log: Mutex<Vec<Sent>>,
    media: Mutex<HashMap<String, Vec<u8>>>,
    pending_pair: Mutex<Option<(Vec<Event>, Vec<Message>)>>,
}

impl FakeClient {
    /// Everything the app asked the fake to do, in order.
    pub fn log(&self) -> Vec<Sent> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }

    fn record(&self, s: Sent) {
        if let Ok(mut l) = self.log.lock() {
            l.push(s);
        }
    }

    /// Simulates the phone scanning the QR code (tests / mock login).
    pub fn scan(&self) {
        let _ = self.tx.send(Event::PairSuccess {
            jid: self.own_jid.clone(),
        });
        let _ = self.tx.send(Event::Connected {
            jid: Some(self.own_jid.clone()),
        });
        let pending = self.pending_pair.lock().ok().and_then(|mut p| p.take());
        if let Some((history, messages)) = pending {
            for e in history {
                let _ = self.tx.send(e);
            }
            let _ = self.tx.send(Event::Messages {
                messages,
                live: false,
            });
        }
    }

    /// Simulates an incoming live message.
    pub fn simulate_incoming(&self, msg: Message) {
        let _ = self.tx.send(Event::Messages {
            messages: vec![msg],
            live: true,
        });
    }

    /// Simulates the phone unlinking this device.
    pub fn simulate_unlink(&self) {
        let _ = self.tx.send(Event::LoggedOut {
            reason: "This device was unlinked from your phone.".into(),
        });
    }

    fn schedule_receipts(&self, chat: &str, id: &str) {
        let tx = self.tx.clone();
        let chat = chat.to_string();
        let ids = vec![id.to_string()];
        let delay = self.opts.receipt_delay;
        let auto = self.opts.auto_receipts;
        let _ = tx.send(Event::Receipt {
            chat: chat.clone(),
            ids: ids.clone(),
            status: ReceiptStatus::Sent,
        });
        if !auto {
            return;
        }
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Event::Receipt {
                chat: chat.clone(),
                ids: ids.clone(),
                status: ReceiptStatus::Delivered,
            });
            tokio::time::sleep(delay).await;
            let _ = tx.send(Event::Receipt {
                chat,
                ids,
                status: ReceiptStatus::Read,
            });
        });
    }
}

impl WaClient for FakeClient {
    fn send_text<'a>(
        &'a self,
        chat: &'a str,
        id: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.opts.fail_sends {
                return Err(Error::Net("fake send failure".into()));
            }
            self.record(Sent::Text {
                chat: chat.into(),
                id: id.into(),
                text: text.into(),
            });
            self.schedule_receipts(chat, id);
            if self.opts.auto_reply {
                let tx = self.tx.clone();
                let chat = chat.to_string();
                let delay = self.opts.receipt_delay * 3;
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = tx.send(Event::Messages {
                        messages: vec![Message {
                            id: super::new_message_id(),
                            chat: chat.clone(),
                            sender: chat,
                            sender_name: None,
                            from_me: false,
                            timestamp: chrono::Utc::now().timestamp(),
                            content: Content::Text("👍".into()),
                            status: ReceiptStatus::Read,
                        }],
                        live: true,
                    });
                });
            }
            Ok(())
        })
    }

    fn send_media<'a>(
        &'a self,
        chat: &'a str,
        id: &'a str,
        media: OutgoingMedia,
    ) -> BoxFuture<'a, Result<Content>> {
        Box::pin(async move {
            if self.opts.fail_sends {
                return Err(Error::Net("fake upload failure".into()));
            }
            let path = format!("/fake/{id}");
            let kind = match media.kind {
                super::OutgoingKind::Image { .. } => super::MediaKind::Image,
                super::OutgoingKind::Document => super::MediaKind::Document,
                super::OutgoingKind::Voice { .. } => super::MediaKind::Audio,
            };
            let mref = MediaRef {
                direct_path: path.clone(),
                media_key: vec![1; 32],
                file_sha256: id.as_bytes().to_vec(),
                file_enc_sha256: vec![2; 32],
                file_length: media.bytes.len() as u64,
                kind,
                mimetype: media.mimetype.clone(),
            };
            if let Ok(mut m) = self.media.lock() {
                m.insert(path, media.bytes.clone());
            }
            let content = media.local_content(Some(mref));
            self.record(Sent::Media {
                chat: chat.into(),
                id: id.into(),
                content: content.clone(),
            });
            self.schedule_receipts(chat, id);
            Ok(content)
        })
    }

    fn download<'a>(&'a self, media: &'a MediaRef) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.media
                .lock()
                .ok()
                .and_then(|m| m.get(&media.direct_path).cloned())
                .ok_or_else(|| Error::Net("media not found (404)".into()))
        })
    }

    fn mark_read<'a>(
        &'a self,
        chat: &'a str,
        _sender: Option<&'a str>,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.record(Sent::Read {
                chat: chat.into(),
                ids: ids.to_vec(),
            });
            Ok(())
        })
    }

    fn mark_played<'a>(
        &'a self,
        chat: &'a str,
        _sender: Option<&'a str>,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.record(Sent::Played {
                chat: chat.into(),
                ids: ids.to_vec(),
            });
            Ok(())
        })
    }

    fn simulate_scan(&self) {
        self.scan();
    }

    fn logout(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.record(Sent::Logout);
            let _ = self.tx.send(Event::LoggedOut {
                reason: "You logged out.".into(),
            });
            Ok(())
        })
    }
}

/// Starts the fake. Returns the concrete handle (for test hooks) and the
/// event stream the app subscribes to.
pub fn start(
    opts: FakeOptions,
    data: MockData,
) -> (Arc<FakeClient>, impl Stream<Item = Event> + Send + 'static) {
    let (tx, rx) = mpsc::unbounded_channel();
    let history = vec![Event::History {
        chats: data.chats.clone(),
        names: data.names.clone(),
        progress: Some(100),
    }];
    let client = Arc::new(FakeClient {
        tx: tx.clone(),
        opts: opts.clone(),
        own_jid: data.own_jid.clone(),
        log: Mutex::new(Vec::new()),
        media: Mutex::new(data.media),
        pending_pair: Mutex::new(None),
    });
    let _ = tx.send(if opts.paired {
        Event::SessionFound
    } else {
        Event::NoSession
    });
    let _ = tx.send(Event::Ready(client.clone()));
    let _ = tx.send(Event::Connection(Connection::Connected));
    if opts.paired {
        let _ = tx.send(Event::Connected {
            jid: Some(data.own_jid.clone()),
        });
        for e in history {
            let _ = tx.send(e);
        }
        let _ = tx.send(Event::Messages {
            messages: data.messages,
            live: false,
        });
    } else {
        if let Ok(mut p) = client.pending_pair.lock() {
            *p = Some((history, data.messages));
        }
        let txq = tx.clone();
        let (n, interval) = (opts.qr_codes, opts.qr_interval);
        tokio::spawn(async move {
            for i in 0..n {
                let code = format!(
                    "2@FERROCHATMOCK{i:02}QRPAYLOAD,bW9ja19ub2lzZV9rZXk=,bW9ja19pZGVudGl0eQ==,bW9ja19hZHY="
                );
                if txq
                    .send(Event::Qr {
                        code,
                        valid_for_secs: interval.as_secs(),
                    })
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(interval).await;
            }
            let _ = txq.send(Event::QrExhausted);
        });
    }
    let stream =
        futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) });
    (client, stream)
}
