//! App state and the single `update` mutation point.

pub mod message;
pub mod state;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use iced::widget::{image, operation};
use iced::{Subscription, Task, window};

pub use message::{Done, Message, Ui};
use state::{ChatList, Conversation, Filter};

use crate::auth::secrets::{KeyringStore, MemoryStore, SIGNAL_STORE_KEY, SecretStore};
use crate::auth::{AuthEvent, AuthState};
use crate::config::{Config, ScreenshotTarget};
use crate::media::ByteLru;
use crate::media::audio::{self, Pcm, Player, Recorder};
use crate::net::{
    self, ClientHandle, Connection, Content, Event, Message as ChatMessage, OutgoingKind,
    OutgoingMedia, ReceiptStatus,
};
use crate::store::Store;
use crate::ui::theme::FerroTheme;

/// Id of the conversation scrollable (for snapping to the newest message).
pub const CONVERSATION_SCROLL: &str = "conversation";
/// Messages loaded per page from the cache.
pub const PAGE: usize = 60;
/// Image cache budget (architecture §3).
pub const IMAGE_BUDGET: usize = 48 * 1024 * 1024;

/// Voice note playback state.
#[derive(Debug, Clone)]
pub struct VoiceState {
    /// Message id.
    pub id: String,
    /// Still downloading / decoding.
    pub loading: bool,
    /// Paused by the user.
    pub paused: bool,
    /// Position.
    pub position_ms: u64,
    /// Total length.
    pub duration_ms: u64,
}

/// Root state.
pub struct App {
    pub(crate) cfg: Config,
    pub(crate) theme: FerroTheme,
    pub(crate) store: Store,
    secrets: Arc<dyn SecretStore>,
    pub(crate) auth: AuthState,
    pub(crate) connection: Connection,
    client: Option<ClientHandle>,
    pub(crate) own_jid: Option<String>,
    net_generation: u32,
    pub(crate) chats: ChatList,
    pub(crate) search: String,
    pub(crate) filter: Filter,
    pub(crate) open: Option<Conversation>,
    pending_read: usize,
    pub(crate) composer: String,
    pub(crate) emoji_open: bool,
    pub(crate) menu_open: bool,
    pub(crate) qr: Option<image::Handle>,
    pub(crate) images: ByteLru<String, image::Handle>,
    pub(crate) thumbs: HashMap<String, image::Handle>,
    downloading: HashSet<String>,
    player: Option<Player>,
    pub(crate) voice: Option<VoiceState>,
    voice_cache: ByteLru<String, Arc<Pcm>>,
    recorder: Option<Recorder>,
    pub(crate) recording_since: Option<Instant>,
    pub(crate) toast: Option<String>,
    pub(crate) focused: bool,
    pub(crate) viewer: Option<String>,
    pub(crate) sync_progress: Option<u32>,
    started: Instant,
    first_frame_logged: bool,
}

/// Subscription key for the protocol stream. Changing `generation`
/// restarts the client (reload QR, after logout).
#[derive(Clone)]
struct NetKey {
    cfg: Config,
    generation: u32,
    secrets: Arc<dyn SecretStore>,
}

impl Hash for NetKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cfg.hash(state);
        self.generation.hash(state);
    }
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn net_stream(key: &NetKey) -> futures::stream::BoxStream<'static, Message> {
    let key = key.clone();
    futures::stream::once(async move {
        if key.cfg.mock {
            let paired = !matches!(key.cfg.screenshot, Some((ScreenshotTarget::Login, _)))
                && key.generation == 0;
            let opts = net::fake::FakeOptions {
                paired,
                auto_reply: true,
                ..Default::default()
            };
            let (_client, stream) = net::fake::start(opts, crate::mock::mock_data(now_secs()));
            stream.boxed()
        } else {
            net::whatsapp::start(key.cfg.data_dir.clone(), key.cfg.signal_db(), key.secrets).boxed()
        }
    })
    .flatten()
    .map(Message::Net)
    .boxed()
}

fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
    map: impl FnOnce(T) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move { tokio::task::spawn_blocking(f).await.ok() },
        move |r| match r {
            Some(v) => map(v),
            None => Message::done(Done::Nothing),
        },
    )
}

fn store_task(
    store: &Store,
    f: impl FnOnce(&Store) -> crate::error::Result<()> + Send + 'static,
) -> Task<Message> {
    let s = store.clone();
    blocking(
        move || f(&s).map_err(|e| e.to_string()),
        |r| Message::done(Done::Stored(r)),
    )
}

/// Encodes a QR payload into an image handle (white quiet zone, black modules).
pub fn qr_handle(payload: &str) -> Option<image::Handle> {
    let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
    let w = code.width();
    let scale = 8usize;
    let quiet = 4usize;
    let side = (w + quiet * 2) * scale;
    let colors = code.to_colors();
    let mut rgba = vec![255u8; side * side * 4];
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = (x + quiet) * scale + dx;
                        let py = (y + quiet) * scale + dy;
                        let i = (py * side + px) * 4;
                        rgba[i] = 0x11;
                        rgba[i + 1] = 0x1b;
                        rgba[i + 2] = 0x21;
                    }
                }
            }
        }
    }
    Some(image::Handle::from_rgba(side as u32, side as u32, rgba))
}

impl App {
    /// Boot: open the cache synchronously, render from it, start the client.
    pub fn boot(cfg: Config) -> (Self, Task<Message>) {
        let started = Instant::now();
        let store = if cfg.mock {
            Store::open_in_memory()
        } else {
            Store::open(&cfg.cache_db())
        }
        .or_else(|e| {
            tracing::error!(error = %e, "cache unreadable; using an in-memory cache");
            Store::open_in_memory()
        });
        let (store, chats) = match store {
            Ok(s) => {
                let rows = s.chat_rows().unwrap_or_default();
                let names = s.contact_names().unwrap_or_default();
                (s, ChatList::from_rows(rows, names))
            }
            Err(e) => {
                // In-memory SQLite only fails on allocation failure.
                eprintln!("ferrochat: cannot open any cache: {e}");
                std::process::exit(1);
            }
        };
        let own_jid = store.kv("own_jid").ok().flatten();
        let secrets: Arc<dyn SecretStore> = if cfg.mock {
            Arc::new(MemoryStore::new())
        } else {
            Arc::new(KeyringStore)
        };
        let auth = if !cfg.mock && cfg.signal_db().exists() {
            AuthState::Restoring
        } else {
            AuthState::WaitingForQr
        };
        let app = Self {
            theme: FerroTheme::new(!cfg.light_theme),
            cfg,
            store,
            secrets,
            auth,
            connection: Connection::Connecting,
            client: None,
            own_jid,
            net_generation: 0,
            chats,
            search: String::new(),
            filter: Filter::All,
            open: None,
            pending_read: 0,
            composer: String::new(),
            emoji_open: false,
            menu_open: false,
            qr: None,
            images: ByteLru::new(IMAGE_BUDGET),
            thumbs: HashMap::new(),
            downloading: HashSet::new(),
            player: None,
            voice: None,
            voice_cache: ByteLru::new(16 * 1024 * 1024),
            recorder: None,
            recording_since: None,
            toast: None,
            focused: true,
            viewer: None,
            sync_progress: None,
            started,
            first_frame_logged: false,
        };
        let mut tasks = vec![Task::done(Message::FirstFrame)];
        if app.cfg.screenshot.is_some() {
            tasks.push(Task::perform(
                tokio::time::sleep(Duration::from_millis(2500)),
                |_| Message::CaptureScreenshot,
            ));
        }
        (app, Task::batch(tasks))
    }

    /// Wrapper for `iced::application`'s title.
    pub fn title(&self) -> String {
        let unread: u32 = self.chats.entries.iter().map(|e| e.meta.unread).sum();
        if unread > 0 {
            format!("({unread}) Ferrochat")
        } else {
            "Ferrochat".into()
        }
    }

    /// iced theme.
    pub fn iced_theme(&self) -> iced::Theme {
        self.theme.iced()
    }

    /// Long-lived streams.
    pub fn subscription(&self) -> Subscription<Message> {
        let key = NetKey {
            cfg: self.cfg.clone(),
            generation: self.net_generation,
            secrets: self.secrets.clone(),
        };
        let mut subs = vec![
            Subscription::run_with(key, net_stream),
            iced::event::listen_with(|event, _status, _id| match event {
                iced::Event::Window(window::Event::Focused) => Some(Message::Focus(true)),
                iced::Event::Window(window::Event::Unfocused) => Some(Message::Focus(false)),
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::Ui(Ui::CloseChat)),
                _ => None,
            }),
        ];
        let playing = self.voice.as_ref().is_some_and(|v| !v.paused);
        if playing || self.recording_since.is_some() {
            subs.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    /// The only mutation point.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Ui(ui) => self.on_ui(ui),
            Message::Net(ev) => self.on_net(ev),
            Message::Done(done) => self.on_done(*done),
            Message::Tick => {
                self.on_tick();
                Task::none()
            }
            Message::Focus(f) => {
                self.focused = f;
                if f {
                    self.send_pending_reads()
                } else {
                    Task::none()
                }
            }
            Message::FirstFrame => {
                if !self.first_frame_logged {
                    self.first_frame_logged = true;
                    let ms = self.started.elapsed().as_secs_f64() * 1000.0;
                    if self.cfg.trace_startup {
                        tracing::info!(first_frame_ms = format!("{ms:.1}"), "startup");
                        eprintln!("first_frame_ms={ms:.1}");
                    }
                }
                Task::none()
            }
            Message::CaptureScreenshot => window::latest()
                .and_then(window::screenshot)
                .map(|s| Message::done(Done::Screenshot(Box::new(s)))),
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(text.into());
    }

    // ---------------------------------------------------------------- UI

    fn on_ui(&mut self, ui: Ui) -> Task<Message> {
        match ui {
            Ui::Search(q) => self.search = q,
            Ui::Filter(f) => self.filter = f,
            Ui::OpenChat(chat) => return self.open_chat(chat),
            Ui::CloseChat => {
                if self.viewer.take().is_none() && !self.emoji_open && !self.menu_open {
                    self.open = None;
                }
                self.emoji_open = false;
                self.menu_open = false;
            }
            Ui::Composer(t) => self.composer = t,
            Ui::Send => return self.send_text(),
            Ui::Attach => {
                self.emoji_open = false;
                return Task::perform(
                    async {
                        let file = rfd::AsyncFileDialog::new()
                            .set_title("Send a photo or document")
                            .pick_file()
                            .await?;
                        let path = file.path().to_path_buf();
                        let res = tokio::task::spawn_blocking(move || {
                            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                            if bytes.len() > 100 * 1024 * 1024 {
                                return Err("Files over 100 MB cannot be sent.".to_string());
                            }
                            Ok(Box::new(crate::media::image::outgoing_from_file(
                                &path, bytes, None,
                            )))
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                        Some(res)
                    },
                    |r| Message::done(Done::Prepared(r)),
                );
            }
            Ui::ToggleEmoji => self.emoji_open = !self.emoji_open,
            Ui::Emoji(e) => self.composer.push_str(e),
            Ui::StartRecording => match Recorder::start() {
                Ok(r) => {
                    self.recorder = Some(r);
                    self.recording_since = Some(Instant::now());
                }
                Err(e) => self.toast(e.user_message()),
            },
            Ui::StopRecording { send } => {
                self.recording_since = None;
                if let Some(rec) = self.recorder.take() {
                    let pcm = rec.finish();
                    if send && pcm.len() > audio::OPUS_RATE as usize / 2 {
                        return blocking(
                            move || {
                                let ogg =
                                    audio::encode_ogg_opus(&pcm).map_err(|e| e.to_string())?;
                                Ok(Box::new(OutgoingMedia {
                                    kind: OutgoingKind::Voice {
                                        seconds: (pcm.len() as u32 / audio::OPUS_RATE).max(1),
                                        waveform: audio::waveform(&pcm),
                                    },
                                    bytes: ogg,
                                    file_name: "voice-note.ogg".into(),
                                    mimetype: "audio/ogg; codecs=opus".into(),
                                    caption: None,
                                }))
                            },
                            |r| Message::done(Done::Prepared(Some(r))),
                        );
                    }
                }
            }
            Ui::PlayVoice(id) => return self.play_voice(id),
            Ui::PauseVoice => {
                if let (Some(v), Some(p)) = (&mut self.voice, &self.player) {
                    p.pause();
                    v.paused = true;
                }
            }
            Ui::OpenDocument(id) => return self.open_document(&id),
            Ui::OpenImage(id) => {
                let task = self.ensure_media(&id);
                self.viewer = Some(id);
                return task;
            }
            Ui::CloseViewer => self.viewer = None,
            Ui::Scrolled { relative_y } => {
                if relative_y > 0.98
                    && let Some(c) = &self.open
                    && c.has_more
                    && let Some((ts, id)) = c.oldest_cursor()
                {
                    let chat = c.chat.clone();
                    let s = self.store.clone();
                    return blocking(
                        move || {
                            s.load_messages(&chat, Some((ts, &id)), PAGE)
                                .map(|m| (chat, m))
                        },
                        |r| match r {
                            Ok((chat, msgs)) => Message::done(Done::Loaded {
                                has_more: msgs.len() == PAGE,
                                chat,
                                msgs,
                                older: true,
                            }),
                            Err(e) => Message::done(Done::Stored(Err(e.to_string()))),
                        },
                    );
                }
            }
            Ui::ReloadQr => {
                self.auth = std::mem::take(&mut self.auth).next(AuthEvent::ReloadQr);
                self.qr = None;
                self.net_generation += 1;
            }
            Ui::Logout => {
                self.menu_open = false;
                if let Some(c) = self.client.clone() {
                    return Task::perform(async move { c.logout().await }, |_| {
                        Message::done(Done::Nothing)
                    })
                    .chain(self.cleanup_session());
                }
                return self.cleanup_session();
            }
            Ui::ToggleMenu => self.menu_open = !self.menu_open,
            Ui::DismissToast => self.toast = None,
            Ui::SimulateScan => {
                if let Some(c) = &self.client {
                    c.simulate_scan();
                }
            }
            Ui::Retry(id) => return self.retry(&id),
        }
        Task::none()
    }

    fn open_chat(&mut self, chat: String) -> Task<Message> {
        self.emoji_open = false;
        self.menu_open = false;
        if self.open.as_ref().is_some_and(|c| c.chat == chat) {
            return Task::none();
        }
        self.pending_read = self
            .chats
            .get(&chat)
            .map(|e| e.meta.unread as usize)
            .unwrap_or(0);
        self.chats.mark_read(&chat);
        self.open = Some(Conversation::new(chat.clone(), Vec::new(), false));
        self.composer.clear();
        let s = self.store.clone();
        let c2 = chat.clone();
        let load = blocking(
            move || s.load_messages(&c2, None, PAGE).map(|m| (c2, m)),
            |r| match r {
                Ok((chat, msgs)) => Message::done(Done::Loaded {
                    has_more: msgs.len() == PAGE,
                    chat,
                    msgs,
                    older: false,
                }),
                Err(e) => Message::done(Done::Stored(Err(e.to_string()))),
            },
        );
        let persist = store_task(&self.store, {
            let chat = chat.clone();
            move |s| s.set_unread(&chat, 0)
        });
        Task::batch([load, persist])
    }

    fn local_message(&self, chat: &str, id: String, content: Content) -> ChatMessage {
        ChatMessage {
            id,
            chat: chat.to_string(),
            sender: self.own_jid.clone().unwrap_or_default(),
            sender_name: None,
            from_me: true,
            timestamp: now_secs(),
            content,
            status: ReceiptStatus::Pending,
        }
    }

    fn add_local(&mut self, msg: ChatMessage) -> Task<Message> {
        self.chats
            .apply_messages(std::slice::from_ref(&msg), Some(&msg.chat), true, false);
        self.index_thumbs(std::slice::from_ref(&msg));
        if let Some(c) = &mut self.open {
            c.merge(vec![msg.clone()]);
        }
        Task::batch([
            store_task(&self.store, move |s| s.insert_messages(&[msg]).map(|_| ())),
            operation::snap_to(CONVERSATION_SCROLL, operation::RelativeOffset::START),
        ])
    }

    fn send_text(&mut self) -> Task<Message> {
        let text = self.composer.trim().to_string();
        let Some(chat) = self.open.as_ref().map(|c| c.chat.clone()) else {
            return Task::none();
        };
        if text.is_empty() {
            return Task::none();
        }
        self.composer.clear();
        self.emoji_open = false;
        let id = net::new_message_id();
        let local = self.local_message(&chat, id.clone(), Content::Text(text.clone()));
        let t1 = self.add_local(local);
        let t2 = self.dispatch_text(chat, id, text);
        Task::batch([t1, t2])
    }

    fn dispatch_text(&self, chat: String, id: String, text: String) -> Task<Message> {
        let Some(client) = self.client.clone() else {
            return Task::done(Message::done(Done::Sent {
                chat,
                id,
                result: Err("not connected".into()),
            }));
        };
        Task::perform(
            async move {
                let r = client
                    .send_text(&chat, &id, &text)
                    .await
                    .map_err(|e| e.to_string());
                (chat, id, r)
            },
            |(chat, id, result)| Message::done(Done::Sent { chat, id, result }),
        )
    }

    fn send_media(&mut self, media: OutgoingMedia) -> Task<Message> {
        let Some(chat) = self.open.as_ref().map(|c| c.chat.clone()) else {
            return Task::none();
        };
        let id = net::new_message_id();
        let local = self.local_message(&chat, id.clone(), media.local_content(None));
        let t1 = self.add_local(local);
        let Some(client) = self.client.clone() else {
            return t1;
        };
        let t2 = Task::perform(
            async move {
                let r = client
                    .send_media(&chat, &id, media)
                    .await
                    .map_err(|e| e.to_string());
                (chat, id, r)
            },
            |(chat, id, result)| Message::done(Done::MediaSent { chat, id, result }),
        );
        Task::batch([t1, t2])
    }

    fn retry(&mut self, id: &str) -> Task<Message> {
        let Some(m) = self.open.as_ref().and_then(|c| c.get(id)).cloned() else {
            return Task::none();
        };
        if m.status != ReceiptStatus::Failed {
            return Task::none();
        }
        match m.content {
            Content::Text(t) => {
                if let Some(c) = &mut self.open
                    && let Some(x) = c.messages.iter_mut().find(|x| x.id == m.id)
                {
                    x.status = ReceiptStatus::Pending;
                }
                self.dispatch_text(m.chat, m.id, t)
            }
            _ => {
                self.toast("Media can't be retried; attach the file again.");
                Task::none()
            }
        }
    }

    fn find_message(&self, id: &str) -> Option<&ChatMessage> {
        self.open.as_ref().and_then(|c| c.get(id))
    }

    /// Starts downloading the media of message `id` if not cached yet.
    fn ensure_media(&mut self, id: &str) -> Task<Message> {
        let Some(mref) = self
            .find_message(id)
            .and_then(|m| m.content.media())
            .cloned()
        else {
            return Task::none();
        };
        let key = mref.cache_key();
        if self.images.contains(&key) || !self.downloading.insert(key.clone()) {
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            self.downloading.remove(&key);
            return Task::none();
        };
        Task::perform(
            async move {
                let r = client
                    .download(&mref)
                    .await
                    .map(Arc::new)
                    .map_err(|e| e.to_string());
                (key, r)
            },
            |(key, result)| Message::done(Done::Downloaded { key, result }),
        )
    }

    fn play_voice(&mut self, id: String) -> Task<Message> {
        if self.player.is_none() {
            match Player::spawn() {
                Ok(p) => self.player = Some(p),
                Err(e) => {
                    self.toast(e.user_message());
                    return Task::none();
                }
            }
        }
        if let (Some(v), Some(p)) = (&mut self.voice, &self.player)
            && v.id == id
            && v.paused
            && !v.loading
        {
            p.resume();
            v.paused = false;
            return Task::none();
        }
        if let Some(p) = &self.player {
            p.stop();
        }
        let Some(msg) = self.find_message(&id).cloned() else {
            return Task::none();
        };
        let Some(mref) = msg.content.media().cloned() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        if !msg.from_me
            && let Some(client) = self.client.clone()
        {
            let chat = msg.chat.clone();
            let sender = net::is_group_jid(&chat).then(|| msg.sender.clone());
            let ids = vec![msg.id.clone()];
            tasks.push(Task::perform(
                async move { client.mark_played(&chat, sender.as_deref(), &ids).await },
                |_| Message::done(Done::Nothing),
            ));
        }
        let key = mref.cache_key();
        if let Some(pcm) = self.voice_cache.get(&key).cloned() {
            self.start_playback(id, pcm);
            return Task::batch(tasks);
        }
        self.voice = Some(VoiceState {
            id: id.clone(),
            loading: true,
            paused: false,
            position_ms: 0,
            duration_ms: 0,
        });
        let Some(client) = self.client.clone() else {
            return Task::batch(tasks);
        };
        tasks.push(Task::perform(
            async move {
                let r = async {
                    let bytes = client.download(&mref).await.map_err(|e| e.to_string())?;
                    tokio::task::spawn_blocking(move || audio::decode_ogg_opus(&bytes))
                        .await
                        .map_err(|e| e.to_string())?
                        .map_err(|e| e.to_string())
                }
                .await;
                (id, r.map(Arc::new))
            },
            |(id, result)| Message::done(Done::VoiceReady { id, result }),
        ));
        Task::batch(tasks)
    }

    fn start_playback(&mut self, id: String, pcm: Arc<Pcm>) {
        let duration_ms = pcm.duration_ms();
        if let Some(p) = &self.player {
            p.play((*pcm).clone(), Duration::ZERO);
        }
        self.voice = Some(VoiceState {
            id,
            loading: false,
            paused: false,
            position_ms: 0,
            duration_ms,
        });
    }

    fn on_tick(&mut self) {
        if let (Some(v), Some(p)) = (&mut self.voice, &self.player)
            && !v.loading
        {
            v.position_ms = p.state().position_ms().min(v.duration_ms);
            if p.state().is_finished() {
                self.voice = None;
            }
        }
    }

    fn open_document(&mut self, id: &str) -> Task<Message> {
        let Some(m) = self.find_message(id).cloned() else {
            return Task::none();
        };
        let Content::Document {
            file_name,
            media: Some(mref),
            ..
        } = m.content
        else {
            return Task::none();
        };
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let dir = self.cfg.downloads_dir.clone();
        let open_after = self.cfg.screenshot.is_none();
        self.toast("Downloading document…");
        Task::perform(
            async move {
                let bytes = client.download(&mref).await.map_err(|e| e.to_string())?;
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let path = unique_path(&dir, &crate::notify::sanitize_file_name(&file_name));
                    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                    if open_after {
                        crate::notify::open_path(&path).map_err(|e| e.to_string())?;
                    }
                    Ok(path)
                })
                .await
                .map_err(|e| e.to_string())?
            },
            |r| Message::done(Done::DocumentOpened(r)),
        )
    }

    fn cleanup_session(&mut self) -> Task<Message> {
        self.client = None;
        self.open = None;
        self.chats = ChatList::default();
        self.images.clear();
        self.thumbs.clear();
        self.own_jid = None;
        if let Some(p) = &self.player {
            p.stop();
        }
        self.voice = None;
        let store = self.store.clone();
        let secrets = self.secrets.clone();
        let signal_db = self.cfg.signal_db();
        let mock = self.cfg.mock;
        blocking(
            move || {
                let _ = store.clear();
                if !mock {
                    net::whatsapp::remove_db(&signal_db);
                    if let Err(e) = secrets.delete(SIGNAL_STORE_KEY) {
                        tracing::warn!(error = %e, "could not delete session key");
                    }
                }
            },
            |_| Message::done(Done::CleanedUp),
        )
    }

    fn send_pending_reads(&mut self) -> Task<Message> {
        if !self.focused || self.pending_read == 0 {
            return Task::none();
        }
        let Some(conv) = &self.open else {
            return Task::none();
        };
        let incoming: Vec<ChatMessage> = conv
            .messages
            .iter()
            .filter(|m| !m.from_me)
            .rev()
            .take(self.pending_read)
            .cloned()
            .collect();
        self.pending_read = 0;
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let chat = conv.chat.clone();
        let batches = state::read_receipt_batches(&chat, &incoming);
        Task::batch(batches.into_iter().map(|(sender, ids)| {
            let client = client.clone();
            let chat = chat.clone();
            Task::perform(
                async move { client.mark_read(&chat, sender.as_deref(), &ids).await },
                |_| Message::done(Done::Nothing),
            )
        }))
    }

    fn index_thumbs(&mut self, msgs: &[ChatMessage]) {
        for m in msgs {
            if let Content::Image {
                thumbnail: Some(t), ..
            } = &m.content
                && !self.thumbs.contains_key(&m.id)
            {
                self.thumbs
                    .insert(m.id.clone(), image::Handle::from_bytes(t.clone()));
            }
        }
        // Bounded: thumbnails only for what is in memory.
        if self.thumbs.len() > 2000 {
            let keep: HashSet<String> = self
                .open
                .iter()
                .flat_map(|c| c.messages.iter().map(|m| m.id.clone()))
                .collect();
            self.thumbs.retain(|k, _| keep.contains(k));
        }
    }

    fn auto_download_images(&mut self) -> Task<Message> {
        let ids: Vec<String> = self
            .open
            .iter()
            .flat_map(|c| c.messages.iter().rev().take(30))
            .filter(|m| matches!(m.content, Content::Image { media: Some(_), .. }))
            .map(|m| m.id.clone())
            .collect();
        Task::batch(
            ids.iter()
                .map(|id| self.ensure_media(id))
                .collect::<Vec<_>>(),
        )
    }

    // ---------------------------------------------------------------- Net

    fn advance_auth(&mut self, ev: AuthEvent) {
        self.auth = std::mem::take(&mut self.auth).next(ev);
    }

    fn on_net(&mut self, ev: Event) -> Task<Message> {
        tracing::debug!(event = ?ev, "net event");
        match ev {
            Event::Ready(h) => self.client = Some(h),
            Event::SessionFound => self.advance_auth(AuthEvent::SessionFound),
            Event::NoSession => self.advance_auth(AuthEvent::NoSession),
            Event::Qr {
                code,
                valid_for_secs,
            } => {
                self.qr = qr_handle(&code);
                self.advance_auth(AuthEvent::Qr {
                    code,
                    valid_for_secs,
                });
            }
            Event::QrExhausted => self.advance_auth(AuthEvent::QrExhausted),
            Event::PairSuccess { jid } => {
                self.own_jid = Some(jid.clone());
                self.qr = None;
                self.advance_auth(AuthEvent::PairSuccess { jid: jid.clone() });
                return store_task(&self.store, move |s| s.set_kv("own_jid", &jid));
            }
            Event::PairError(e) => self.advance_auth(AuthEvent::PairError(e)),
            Event::Connection(c) => self.connection = c,
            Event::Connected { jid } => {
                self.connection = Connection::Connected;
                if let Some(j) = &jid {
                    self.own_jid = Some(j.clone());
                }
                self.advance_auth(AuthEvent::Connected { jid: jid.clone() });
                if let Some(j) = jid {
                    return store_task(&self.store, move |s| s.set_kv("own_jid", &j));
                }
            }
            Event::LoggedOut { reason } => {
                self.toast(reason.clone());
                self.advance_auth(AuthEvent::LoggedOut(reason));
                return self.cleanup_session();
            }
            Event::Messages { messages, live } => return self.on_messages(messages, live),
            Event::History {
                chats,
                names,
                progress,
            } => {
                self.sync_progress = progress.filter(|p| *p < 100);
                self.chats.apply_history(&chats, &names);
                let mut tasks = vec![store_task(&self.store, move |s| {
                    s.upsert_chats(&chats)?;
                    s.set_contact_names(&names)
                })];
                if matches!(self.cfg.screenshot, Some((ScreenshotTarget::Chat, _)))
                    && self.open.is_none()
                {
                    tasks.push(Task::done(Message::Ui(Ui::OpenChat(
                        crate::mock::SHOWCASE_CHAT.into(),
                    ))));
                }
                return Task::batch(tasks);
            }
            Event::Receipt { chat, ids, status } => {
                self.chats.apply_receipt(&chat, &ids, status);
                if let Some(c) = &mut self.open
                    && c.chat == chat
                {
                    c.apply_receipt(&ids, status);
                }
                return store_task(&self.store, move |s| {
                    s.apply_receipt(&chat, &ids, status).map(|_| ())
                });
            }
            Event::ContactName { jid, name } => {
                self.chats.names.insert(jid.clone(), name.clone());
                return store_task(&self.store, move |s| s.set_contact_names(&[(jid, name)]));
            }
            Event::ChatRead { chat } => {
                self.chats.mark_read(&chat);
                return store_task(&self.store, move |s| s.set_unread(&chat, 0));
            }
            Event::Fatal(msg) => self.advance_auth(AuthEvent::Fatal(msg)),
        }
        Task::none()
    }

    fn on_messages(&mut self, messages: Vec<ChatMessage>, live: bool) -> Task<Message> {
        let open_chat = self.open.as_ref().map(|c| c.chat.clone());
        self.chats
            .apply_messages(&messages, open_chat.as_deref(), self.focused, live);
        self.index_thumbs(&messages);
        let mut tasks = Vec::new();
        if let Some(c) = &mut self.open {
            let mine: Vec<_> = messages
                .iter()
                .filter(|m| m.chat == c.chat)
                .cloned()
                .collect();
            if !mine.is_empty() {
                c.merge(mine.clone());
                if live {
                    let incoming = mine.iter().filter(|m| !m.from_me).count();
                    if self.focused {
                        self.pending_read += incoming;
                    }
                    tasks.push(operation::snap_to(
                        CONVERSATION_SCROLL,
                        operation::RelativeOffset::START,
                    ));
                }
            }
        }
        tasks.push(self.auto_download_images());
        tasks.push(self.send_pending_reads());
        if live && self.cfg.screenshot.is_none() {
            let mode = self.cfg.notification_preview;
            for m in messages.iter().filter(|m| {
                let muted = self.chats.get(&m.chat).is_some_and(|e| e.meta.muted);
                state::should_notify(m, open_chat.as_deref(), self.focused, muted)
            }) {
                let sender = m
                    .sender_name
                    .clone()
                    .unwrap_or_else(|| self.chats.display_name(&m.sender));
                let group = net::is_group_jid(&m.chat).then(|| self.chats.display_name(&m.chat));
                let (summary, body) = crate::notify::notification_text(
                    &sender,
                    group.as_deref(),
                    &m.content.preview(),
                    mode,
                );
                tasks.push(blocking(
                    move || crate::notify::show(&summary, &body),
                    |_| Message::done(Done::Nothing),
                ));
            }
        }
        tasks.push(store_task(&self.store, move |s| {
            s.insert_messages(&messages).map(|_| ())
        }));
        Task::batch(tasks)
    }

    // ---------------------------------------------------------------- Done

    fn on_done(&mut self, done: Done) -> Task<Message> {
        match done {
            Done::Loaded {
                chat,
                msgs,
                older,
                has_more,
            } => {
                self.index_thumbs(&msgs);
                if let Some(c) = &mut self.open
                    && c.chat == chat
                {
                    if older {
                        c.prepend(msgs, has_more);
                    } else {
                        c.merge(msgs);
                        c.has_more = has_more;
                        return Task::batch([
                            operation::snap_to(
                                CONVERSATION_SCROLL,
                                operation::RelativeOffset::START,
                            ),
                            self.auto_download_images(),
                            self.send_pending_reads(),
                        ]);
                    }
                }
            }
            Done::Sent { chat, id, result } => {
                let status = match &result {
                    Ok(()) => ReceiptStatus::Sent,
                    Err(e) => {
                        tracing::warn!(error = %e, "send failed");
                        ReceiptStatus::Failed
                    }
                };
                return self.on_net(Event::Receipt {
                    chat,
                    ids: vec![id],
                    status,
                });
            }
            Done::MediaSent { chat, id, result } => match result {
                Ok(content) => {
                    if let Some(c) = &mut self.open
                        && c.chat == chat
                    {
                        c.set_content(&id, content.clone());
                    }
                    let (c2, i2) = (chat.clone(), id.clone());
                    let t = store_task(&self.store, move |s| s.update_message(&c2, &i2, &content));
                    return Task::batch([
                        t,
                        self.on_net(Event::Receipt {
                            chat,
                            ids: vec![id],
                            status: ReceiptStatus::Sent,
                        }),
                    ]);
                }
                Err(e) => {
                    self.toast(format!("Could not send media: {e}"));
                    return self.on_net(Event::Receipt {
                        chat,
                        ids: vec![id],
                        status: ReceiptStatus::Failed,
                    });
                }
            },
            Done::Downloaded { key, result } => {
                self.downloading.remove(&key);
                match result {
                    Ok(bytes) => {
                        let size = bytes.len();
                        self.images
                            .put(key, image::Handle::from_bytes((*bytes).clone()), size);
                    }
                    Err(e) => tracing::warn!(error = %e, "media download failed"),
                }
            }
            Done::VoiceReady { id, result } => match result {
                Ok(pcm) => {
                    if let Some(key) = self
                        .find_message(&id)
                        .and_then(|m| m.content.media())
                        .map(|m| m.cache_key())
                    {
                        let size = pcm.samples.len() * 4;
                        self.voice_cache.put(key, pcm.clone(), size);
                    }
                    if self.voice.as_ref().is_some_and(|v| v.id == id) {
                        self.start_playback(id, pcm);
                    }
                }
                Err(e) => {
                    self.voice = None;
                    self.toast(format!("Could not play voice note: {e}"));
                }
            },
            Done::Prepared(Some(Ok(media))) => return self.send_media(*media),
            Done::Prepared(Some(Err(e))) => self.toast(e),
            Done::Prepared(None) => {}
            Done::DocumentOpened(Ok(path)) => {
                self.toast(format!("Saved to {}", path.display()));
            }
            Done::DocumentOpened(Err(e)) => self.toast(format!("Could not open document: {e}")),
            Done::Stored(Err(e)) => tracing::warn!(error = %e, "cache write failed"),
            Done::Stored(Ok(())) | Done::Nothing => {}
            Done::CleanedUp => {
                self.net_generation += 1;
                self.qr = None;
            }
            Done::Screenshot(shot) => {
                let Some((_, path)) = self.cfg.screenshot.clone() else {
                    return Task::none();
                };
                return blocking(
                    move || save_png(&path, &shot).map(|_| path),
                    |r| Message::done(Done::ScreenshotSaved(r)),
                );
            }
            Done::ScreenshotSaved(r) => {
                match r {
                    Ok(p) => eprintln!("screenshot written to {}", p.display()),
                    Err(e) => eprintln!("screenshot failed: {e}"),
                }
                return iced::exit();
            }
        }
        Task::none()
    }

    /// Current playback / recording status (for tests).
    pub fn is_recording(&self) -> bool {
        self.recording_since.is_some()
    }
}

fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (1..1000)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(candidate)
}

fn save_png(path: &std::path::Path, shot: &window::Screenshot) -> Result<(), String> {
    let (w, h) = (shot.size.width, shot.size.height);
    let img = ::image::RgbaImage::from_raw(w, h, shot.rgba.to_vec())
        .ok_or_else(|| "bad screenshot buffer".to_string())?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    img.save(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_handle_renders() {
        assert!(qr_handle("2@abc,def,ghi").is_some());
    }

    #[test]
    fn unique_paths_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("ferrochat-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.pdf"), dir.join("a (1).pdf"));
        assert_eq!(unique_path(&dir, "b.pdf"), dir.join("b.pdf"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
