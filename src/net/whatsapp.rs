//! Live protocol client on top of `whatsapp-rust`.
//!
//! * The library's Signal/session state lives in a SQLCipher database
//!   (`session.sqlcipher`). Its 256-bit raw key is generated on first run and
//!   kept in the OS keyring (see [`crate::auth::secrets::signal_store_key`]),
//!   so only ciphertext touches the disk.
//! * Library events are forwarded through a channel and converted to
//!   [`super::Event`]s by the pure functions in [`super::convert`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use diesel::RunQueryDsl;
use futures::future::BoxFuture;
use futures::stream::Stream;
use tokio::sync::mpsc;
use whatsapp_rust::bot::{Bot, BotHandle};
use whatsapp_rust::download::{DownloadParams, MediaType};
use whatsapp_rust::prelude::{Event as WaEvent, EventHandler};
use whatsapp_rust::send::SendOptions;
use whatsapp_rust::upload::UploadOptions;
use whatsapp_rust::wacore::store::DevicePropsOverride;
use whatsapp_rust::{Client, Jid};
use whatsapp_rust_sqlite_storage::{SqliteStore, SqliteStoreConfig};

use super::convert::{self, Envelope, Uploaded};
use super::types::{Content, MediaKind, MediaRef, OutgoingKind, OutgoingMedia};
use super::{Connection, Event, WaClient};
use crate::auth::secrets::{SecretStore, signal_store_key};
use crate::error::{Error, MaskedJid, Result};

/// Live client handle.
pub struct RealClient {
    client: Arc<Client>,
    handle: Mutex<Option<BotHandle>>,
    signal_db: PathBuf,
}

fn parse_jid(s: &str) -> Result<Jid> {
    s.parse::<Jid>()
        .map_err(|e| Error::Protocol(format!("invalid jid {}: {e}", MaskedJid(s))))
}

fn media_type(kind: MediaKind) -> MediaType {
    match kind {
        MediaKind::Image => MediaType::Image,
        MediaKind::Document => MediaType::Document,
        MediaKind::Audio => MediaType::Audio,
    }
}

impl WaClient for RealClient {
    fn send_text<'a>(
        &'a self,
        chat: &'a str,
        id: &'a str,
        text: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let jid = parse_jid(chat)?;
            self.client
                .send_message_with_options(
                    jid,
                    convert::build_text(text),
                    SendOptions::default().with_message_id(id),
                )
                .await
                .map_err(|e| Error::Net(e.to_string()))?;
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
            let jid = parse_jid(chat)?;
            let mt = match media.kind {
                OutgoingKind::Image { .. } => MediaType::Image,
                OutgoingKind::Document => MediaType::Document,
                OutgoingKind::Voice { .. } => MediaType::Audio,
            };
            let up = self
                .client
                .upload(media.bytes.clone(), mt, UploadOptions::default())
                .await
                .map_err(|e| Error::Net(format!("upload failed: {e}")))?;
            let uploaded = Uploaded {
                url: up.url,
                direct_path: up.direct_path,
                media_key: up.media_key.to_vec(),
                file_sha256: up.file_sha256.to_vec(),
                file_enc_sha256: up.file_enc_sha256.to_vec(),
                file_length: up.file_length,
                media_key_timestamp: up.media_key_timestamp,
                streaming_sidecar: up.streaming_sidecar,
            };
            let (msg, content) = convert::build_media(&media, &uploaded);
            self.client
                .send_message_with_options(jid, msg, SendOptions::default().with_message_id(id))
                .await
                .map_err(|e| Error::Net(e.to_string()))?;
            Ok(content)
        })
    }

    fn download<'a>(&'a self, media: &'a MediaRef) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let params = DownloadParams::encrypted(
                media.direct_path.clone(),
                &media.media_key,
                &media.file_sha256,
                &media.file_enc_sha256,
                media.file_length,
                media_type(media.kind),
            );
            self.client
                .download_from_params(&params)
                .await
                .map_err(|e| Error::Net(format!("download failed: {e}")))
        })
    }

    fn mark_read<'a>(
        &'a self,
        chat: &'a str,
        sender: Option<&'a str>,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let chat = parse_jid(chat)?;
            let sender = sender.map(parse_jid).transpose()?;
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.client
                .mark_as_read(&chat, sender.as_ref(), &ids)
                .await
                .map_err(|e| Error::Net(e.to_string()))
        })
    }

    fn mark_played<'a>(
        &'a self,
        chat: &'a str,
        sender: Option<&'a str>,
        ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let chat = parse_jid(chat)?;
            let sender = sender.map(parse_jid).transpose()?;
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            self.client
                .mark_as_played(&chat, sender.as_ref(), &ids)
                .await
                .map_err(|e| Error::Net(e.to_string()))
        })
    }

    fn logout(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.client.logout().await;
            let handle = self.handle.lock().ok().and_then(|mut h| h.take());
            if let Some(h) = handle {
                h.shutdown().await;
            }
            remove_db(&self.signal_db);
            Ok(())
        })
    }
}

/// Removes the encrypted session database and its WAL/SHM side files.
pub fn remove_db(path: &Path) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}

/// Library event handler: forwards into our channel, conversion happens on
/// our own task so the dispatch path stays cheap.
struct Forwarder(mpsc::UnboundedSender<Arc<WaEvent>>);

impl EventHandler for Forwarder {
    fn handle_event(&self, event: Arc<WaEvent>) {
        let _ = self.0.send(event);
    }
}

async fn open_store(path: &Path, key_hex: String) -> Result<SqliteStore> {
    let url = path.to_string_lossy().to_string();
    let config = SqliteStoreConfig::default().with_connection_init(move |conn| {
        // Raw-key form: no PBKDF2 at every open, the key is already 256 random bits.
        diesel::sql_query(format!("PRAGMA key = \"x'{key_hex}'\";")).execute(conn)?;
        // Fails here (not later) when the key does not match the file.
        diesel::sql_query("SELECT count(*) FROM sqlite_master;").execute(conn)?;
        Ok(())
    });
    SqliteStore::with_config(&url, config)
        .await
        .map_err(|e| Error::Auth(format!("cannot open session store: {e}")))
}

/// Starts the live client and returns its event stream. The first event is
/// [`Event::SessionFound`] / [`Event::NoSession`], then [`Event::Ready`].
pub fn start(
    data_dir: PathBuf,
    signal_db: PathBuf,
    secrets: Arc<dyn SecretStore>,
) -> impl Stream<Item = Event> + Send + 'static {
    let (tx, rx) = mpsc::unbounded_channel::<Event>();
    tokio::spawn(async move {
        if let Err(e) = run(data_dir, signal_db, secrets, tx.clone()).await {
            tracing::error!(error = %e, "protocol client stopped");
            let _ = tx.send(Event::Fatal(e.user_message()));
        }
    });
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) })
}

async fn run(
    data_dir: PathBuf,
    signal_db: PathBuf,
    secrets: Arc<dyn SecretStore>,
    tx: mpsc::UnboundedSender<Event>,
) -> Result<()> {
    std::fs::create_dir_all(&data_dir)?;
    let (key, created) = tokio::task::spawn_blocking(move || signal_store_key(secrets.as_ref()))
        .await
        .map_err(|_| Error::Cancelled)??;
    if created && signal_db.exists() {
        // The old key is gone, so the old database is undecryptable.
        tracing::warn!("new session key generated; discarding unreadable session store");
        remove_db(&signal_db);
    }
    let had_session = signal_db.exists();
    let _ = tx.send(if had_session {
        Event::SessionFound
    } else {
        Event::NoSession
    });
    let _ = tx.send(Event::Connection(Connection::Connecting));

    let store = match open_store(&signal_db, key.expose().to_string()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "session store unreadable; starting a fresh pairing");
            remove_db(&signal_db);
            let _ = tx.send(Event::NoSession);
            open_store(&signal_db, key.expose().to_string()).await?
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&signal_db, std::fs::Permissions::from_mode(0o600));
    }

    let (wa_tx, mut wa_rx) = mpsc::unbounded_channel::<Arc<WaEvent>>();
    let bot = Bot::builder()
        .with_backend(store)
        .with_device_props(DevicePropsOverride::new().with_os("Ferrochat"))
        .with_event_handler(Forwarder(wa_tx))
        .build()
        .await
        .map_err(|e| Error::Protocol(format!("client setup failed: {e}")))?;
    let client = bot.client();
    let handle = bot.spawn();
    let real = Arc::new(RealClient {
        client: client.clone(),
        handle: Mutex::new(Some(handle)),
        signal_db: signal_db.clone(),
    });
    let _ = tx.send(Event::Ready(real.clone()));

    let mut attempt = 0u32;
    loop {
        tokio::select! {
            _ = tx.closed() => {
                // The UI dropped the subscription (logout / reload / exit).
                let handle = real.handle.lock().ok().and_then(|mut h| h.take());
                if let Some(h) = handle {
                    h.shutdown().await;
                }
                return Ok(());
            }
            ev = wa_rx.recv() => {
                let Some(ev) = ev else { return Ok(()) };
                for out in translate(&ev, &client, &mut attempt) {
                    if tx.send(out).is_err() {
                        break;
                    }
                }
            }
        }
    }
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Library event → zero or more app events.
fn translate(ev: &WaEvent, client: &Client, attempt: &mut u32) -> Vec<Event> {
    match ev {
        WaEvent::PairingQrCode(q) => vec![Event::Qr {
            code: q.code.clone(),
            valid_for_secs: q.timeout.as_secs(),
        }],
        WaEvent::PairingQrCodesExhausted(_) => vec![Event::QrExhausted],
        WaEvent::PairSuccess(p) => vec![Event::PairSuccess {
            jid: convert::bare_jid(&p.id.to_string()),
        }],
        WaEvent::PairError(p) => vec![Event::PairError(p.error.clone())],
        WaEvent::Connected(_) => {
            *attempt = 0;
            vec![
                Event::Connection(Connection::Connected),
                Event::Connected {
                    jid: client.pn().map(|j| convert::bare_jid(&j.to_string())),
                },
            ]
        }
        WaEvent::Disconnected(_) | WaEvent::ConnectFailure(_) | WaEvent::StreamError(_) => {
            *attempt += 1;
            vec![Event::Connection(Connection::Reconnecting {
                attempt: *attempt,
            })]
        }
        WaEvent::LoggedOut(l) => vec![Event::LoggedOut {
            reason: format!("This device was unlinked ({:?}).", l.reason),
        }],
        WaEvent::StreamReplaced(_) => vec![Event::LoggedOut {
            reason: "WhatsApp was opened with this session somewhere else.".into(),
        }],
        WaEvent::TemporaryBan(_) => vec![Event::Fatal(
            "WhatsApp temporarily banned this account. Third-party clients break WhatsApp's \
             terms of service."
                .into(),
        )],
        WaEvent::ClientOutdated(_) => vec![Event::Fatal(
            "WhatsApp rejected this client version. Update Ferrochat.".into(),
        )],
        WaEvent::Messages(batch) => {
            let live = matches!(batch.origin, whatsapp_rust::prelude::BatchOrigin::Live);
            let mut out = Vec::new();
            for m in batch.messages.iter() {
                let info = &m.info;
                let env = Envelope {
                    id: info.id.to_string(),
                    chat: info.source.chat.to_string(),
                    sender: info.source.sender.to_string(),
                    from_me: info.source.is_from_me,
                    push_name: info.push_name.to_string(),
                    timestamp: info.timestamp.timestamp(),
                };
                if let Some(target) = convert::revoked_target(&m.message) {
                    out.push(super::Message {
                        id: target,
                        content: Content::Deleted,
                        ..match convert::message_from_envelope(&env, &m.message) {
                            Some(msg) => msg,
                            None => continue,
                        }
                    });
                } else if let Some(msg) = convert::message_from_envelope(&env, &m.message) {
                    out.push(msg);
                }
            }
            if out.is_empty() {
                vec![]
            } else {
                vec![Event::Messages {
                    messages: out,
                    live: live && batch.messages.iter().all(|m| !m.info.is_offline),
                }]
            }
        }
        WaEvent::Receipt(r) => {
            let chat = convert::bare_jid(&r.source.chat.to_string());
            if convert::is_self_read(&r.r#type) {
                return vec![Event::ChatRead { chat }];
            }
            match convert::receipt_status(&r.r#type) {
                Some(status) => vec![Event::Receipt {
                    chat,
                    ids: r.message_ids.iter().map(|i| i.to_string()).collect(),
                    status,
                }],
                None => vec![],
            }
        }
        WaEvent::HistorySync(lazy) => match lazy.get() {
            Some(hs) => {
                let (chats, messages, names) = convert::from_history(hs, now_secs());
                let mut v = vec![Event::History {
                    chats,
                    names,
                    progress: lazy.progress(),
                }];
                if !messages.is_empty() {
                    v.push(Event::Messages {
                        messages,
                        live: false,
                    });
                }
                v
            }
            None => vec![],
        },
        WaEvent::PushNameUpdate(p) => vec![Event::ContactName {
            jid: convert::bare_jid(&p.jid.to_string()),
            name: p.new_push_name.clone(),
        }],
        WaEvent::ContactUpdate(c) => match c
            .action
            .full_name
            .clone()
            .or_else(|| c.action.first_name.clone())
        {
            Some(name) if !name.is_empty() => vec![Event::ContactName {
                jid: convert::bare_jid(&c.jid.to_string()),
                name,
            }],
            _ => vec![],
        },
        WaEvent::MarkChatAsReadUpdate(m) if m.action.read == Some(true) => {
            vec![Event::ChatRead {
                chat: convert::bare_jid(&m.jid.to_string()),
            }]
        }
        _ => vec![],
    }
}
