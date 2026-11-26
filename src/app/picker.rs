//! Stickers and GIFs: the picker's "GIF" and "Stickers" tabs, sending them,
//! and playing received ones inline.
//!
//! * **Stickers tab**: favourites (local to this computer) and recent
//!   stickers (sent and received, kept on the bounded disk shelf
//!   `media::sticker::Shelf`), plus "Create" (a picture -> 512x512 WebP).
//!   `whatsapp-rust` 0.7 drops the phone's `favoriteSticker` app-state
//!   mutations (no event, no read API), so favourites are not synced and
//!   nothing is ever written to the phone's app state.
//! * **GIF tab**: recent GIFs (sent and received), "send a GIF from a file"
//!   (`.gif` converted to MP4 by the helper, `.mp4` checked by it) and,
//!   with the user's own GIPHY key, search (`net::giphy`).
//! * **Inline playback**: animated stickers are decoded in the download
//!   task, GIFs by the helper's frames mode; both become an [`Anim`] in
//!   `App::anims` drawn by the self-playing `ui::anim_image`.
//!
//! Everything slow runs off the UI thread: disk on the blocking pool (short
//! and bounded), network and helper processes on detached threads
//! (`media::detached`). Nothing is loaded until the drawer opens on a tab,
//! and the tiles are dropped when it closes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use iced::Task;
use iced::widget::image::Handle;

use super::anim::{self, Anim};
use super::message::{Done, Fetched, Message, Ui};
use super::{App, blocking, store_task};
use crate::auth::secrets::Secret;
use crate::media::gifv;
use crate::media::sticker::{self as shelf, Kind, Shelf, ShelfItem};
use crate::media::video::{self, Failure};
use crate::net::giphy::{self, Endpoint, GifHit, SearchError};
use crate::net::{self, Content, GifSource, MediaRef, OutgoingKind, OutgoingMedia};

/// Side of a picker tile.
pub const TILE: f32 = 86.0;
/// Height of a GIF tile.
pub const GIF_TILE_H: f32 = 96.0;
/// Space between tiles.
pub const TILE_GAP: f32 = 6.0;
/// GIF tiles downloading at once.
const MAX_TILE_FETCHES: usize = 4;
/// GIFs decoded for bubbles at once (one helper process each).
const MAX_GIF_DECODES: usize = 2;
/// Decoded animations kept for bubbles.
pub const ANIM_BUDGET: usize = 48 * 1024 * 1024;
/// Wait after the last keystroke before searching.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(450);
/// Mock-mode GIF "key" (mock search never leaves the process).
const MOCK_KEY: &str = "mock";

/// Drawer tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Emoji grid.
    #[default]
    Emoji,
    /// GIF search, recent GIFs, GIF from a file.
    Gifs,
    /// Favourite and recent stickers.
    Stickers,
}

/// Whether a GIPHY key is configured.
#[derive(Clone, Default)]
pub enum KeyState {
    /// Not read yet (the keyring is only asked when needed).
    #[default]
    Unknown,
    /// Being read.
    Loading,
    /// No key.
    Missing,
    /// A key.
    Present(Secret),
    /// The keyring could not be read.
    Error(String),
}

impl std::fmt::Debug for KeyState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            KeyState::Unknown => "Unknown",
            KeyState::Loading => "Loading",
            KeyState::Missing => "Missing",
            KeyState::Present(_) => "Present",
            KeyState::Error(_) => "Error",
        })
    }
}

/// A sticker made from a picture, waiting for "Send".
#[derive(Debug, Clone)]
pub struct Draft {
    /// The WebP.
    pub webp: Arc<Vec<u8>>,
    /// Preview.
    pub handle: Handle,
}

/// Picker state for the GIF and Stickers tabs.
#[derive(Debug, Default)]
pub struct MediaPicker {
    /// Active tab.
    pub tab: Tab,
    /// Recent stickers, newest first.
    pub recent_stickers: Vec<ShelfItem>,
    /// Favourite stickers (this computer only).
    pub favourites: Vec<ShelfItem>,
    /// Recent GIFs, newest first.
    pub recent_gifs: Vec<ShelfItem>,
    /// Snapshots shown while the drawer is open (sending must not reflow
    /// the grid under the pointer).
    pub shown_stickers: Vec<ShelfItem>,
    /// See `shown_stickers`.
    pub shown_favourites: Vec<ShelfItem>,
    /// See `shown_stickers`.
    pub shown_gifs: Vec<ShelfItem>,
    /// Still tiles by shelf key (sticker first frames, GIF posters).
    pub tiles: HashMap<String, Handle>,
    tiles_loading: HashSet<String>,
    /// GIF search box.
    pub query: String,
    generation: u32,
    /// Orders list saves: they run on the blocking pool, so an older snapshot
    /// could otherwise land after a newer one and undo it.
    saves: Arc<SaveOrder>,
    /// GIPHY key.
    pub key: KeyState,
    /// Search results of the current query.
    pub results: Option<Result<Vec<GifHit>, String>>,
    /// A search is running.
    pub searching: bool,
    /// Animated tiles of the results, by GIPHY id.
    pub hit_tiles: HashMap<String, Arc<Anim>>,
    tile_queue: VecDeque<GifHit>,
    tiles_inflight: usize,
    /// Sticker made from a picture, waiting for "Send".
    pub draft: Option<Draft>,
    /// Status line ("Converting GIF…"), cleared when done.
    pub busy: Option<String>,
    /// Settings: the key being typed.
    pub key_input: String,
}

/// Sequence numbers of list saves; only a newer snapshot may overwrite.
#[derive(Debug, Default)]
struct SaveOrder {
    next: std::sync::atomic::AtomicU64,
    written: std::sync::Mutex<u64>,
}

impl SaveOrder {
    fn ticket(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1
    }

    /// Runs `write` unless a newer snapshot was already written; the lock
    /// keeps concurrent saves from interleaving.
    fn write_if_newest(
        &self,
        ticket: u64,
        write: impl FnOnce() -> crate::error::Result<()>,
    ) -> crate::error::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        if ticket < *written {
            return Ok(());
        }
        write()?;
        *written = ticket;
        Ok(())
    }
}

impl MediaPicker {
    /// Lists loaded from the cache at boot.
    pub fn load(store: &crate::store::Store) -> Self {
        let list = |k: &str| {
            store
                .kv(k)
                .ok()
                .flatten()
                .map(|j| shelf::parse_list(&j))
                .unwrap_or_default()
        };
        Self {
            recent_stickers: list(shelf::RECENT_STICKERS_KEY),
            favourites: list(shelf::FAVOURITE_STICKERS_KEY),
            recent_gifs: list(shelf::RECENT_GIFS_KEY),
            ..Default::default()
        }
    }

    /// The drawer opened: take the snapshots the grid shows.
    pub fn opened(&mut self) {
        self.shown_stickers = self.recent_stickers.clone();
        self.shown_favourites = self.favourites.clone();
        self.shown_gifs = self.recent_gifs.clone();
    }

    /// The drawer closed: free every tile (zero cost while closed).
    pub fn closed(&mut self) {
        self.tiles = HashMap::new();
        self.tiles_loading.clear();
        self.hit_tiles = HashMap::new();
        self.tile_queue.clear();
        self.results = None;
        self.query.clear();
        self.searching = false;
        self.draft = None;
        self.generation = self.generation.wrapping_add(1);
        self.tab = Tab::Emoji;
    }

    /// Whether anything is held for the drawer.
    pub fn holds_tiles(&self) -> bool {
        !self.tiles.is_empty() || !self.hit_tiles.is_empty() || self.results.is_some()
    }

    /// Whether `key` is a favourite.
    pub fn is_favourite(&self, key: &str) -> bool {
        self.favourites.iter().any(|i| i.key == key)
    }

    /// Whether GIF search can run.
    pub fn has_key(&self) -> bool {
        matches!(self.key, KeyState::Present(_))
    }

    /// Every key a list mentions (the shelf keeps exactly these).
    fn keep(list: &[&[ShelfItem]]) -> Vec<String> {
        list.iter()
            .flat_map(|l| l.iter().map(|i| i.key.clone()))
            .collect()
    }
}

/// The picker's UI messages.
#[derive(Debug, Clone)]
pub enum PickerUi {
    /// Drawer tab.
    Tab(Tab),
    /// Send a sticker from the shelf.
    SendSticker(String),
    /// Right click on a sticker tile: favourite / unfavourite.
    ToggleFavourite(String),
    /// Message menu: favourite / unfavourite the sticker of this message.
    FavouriteMessage(String),
    /// "Create": pick a picture to make a sticker.
    CreateSticker,
    /// Send the made sticker.
    DraftSend,
    /// Drop the made sticker.
    DraftCancel,
    /// GIF search box edited.
    Query(String),
    /// Debounce timer of a query.
    SearchDue(u32),
    /// A search result clicked (index).
    SendHit(usize),
    /// A recent GIF clicked (shelf key).
    SendRecentGif(String),
    /// "Send a GIF from a file".
    SendGifFile,
    /// Settings: key field edited.
    KeyInput(String),
    /// Settings: save the key.
    SaveKey,
    /// Settings: remove the key.
    RemoveKey,
    /// Open the settings page from the GIF tab.
    OpenSettings,
}

/// A ready-to-send sticker or GIF.
#[derive(Clone)]
pub struct Prepared {
    /// Chat it was picked in.
    pub chat: String,
    /// What is uploaded.
    pub media: OutgoingMedia,
    /// Its shelf entry (recent list).
    pub item: ShelfItem,
    /// Sticker or GIF.
    pub kind: Kind,
    /// Bubble picture of a static sticker.
    pub still: Option<(u32, u32, Vec<u8>)>,
    /// Bubble animation.
    pub anim: Option<Arc<Anim>>,
}

/// Results of the picker's tasks.
#[derive(Clone)]
pub enum PickerDone {
    /// A still tile is ready (`None`: the file is gone or broken).
    Tile {
        /// Shelf key.
        key: String,
        /// RGBA still (stickers) or JPEG bytes (GIF posters).
        picture: Option<TilePicture>,
    },
    /// The keyring answered.
    Key(Result<Option<Secret>, String>),
    /// The key was saved (`true`) or removed.
    KeySaved(Result<Option<Secret>, String>),
    /// Search results.
    Results {
        /// Query generation.
        generation: u32,
        /// Hits or a user-facing error.
        result: Result<Vec<GifHit>, String>,
    },
    /// An animated result tile.
    HitTile {
        /// Query generation.
        generation: u32,
        /// GIPHY id.
        id: String,
        /// The animation (`None`: failed, the tile stays blank).
        anim: Option<Arc<Anim>>,
    },
    /// A sticker / GIF is ready to send, or why not.
    Ready(Result<Box<Prepared>, String>),
    /// A made sticker is ready for its preview.
    Draft(Result<(Vec<u8>, u32, u32, Vec<u8>), String>),
    /// A received GIF was decoded for its bubble.
    GifAnim {
        /// Media cache key.
        key: String,
        /// The animation and the recent-list entry.
        result: Result<(Arc<Anim>, ShelfItem), String>,
    },
}

/// Pixels of a still tile.
#[derive(Clone)]
pub enum TilePicture {
    /// RGBA.
    Rgba(u32, u32, Vec<u8>),
    /// Encoded (JPEG).
    Encoded(Vec<u8>),
}

impl std::fmt::Debug for PickerDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            PickerDone::Tile { .. } => "Tile",
            PickerDone::Key(_) => "Key",
            PickerDone::KeySaved(_) => "KeySaved",
            PickerDone::Results { .. } => "Results",
            PickerDone::HitTile { .. } => "HitTile",
            PickerDone::Ready(_) => "Ready",
            PickerDone::Draft(_) => "Draft",
            PickerDone::GifAnim { .. } => "GifAnim",
        };
        f.write_str(name)
    }
}

fn wrap(d: PickerDone) -> Message {
    Message::done(Done::Picker(Box::new(d)))
}

/// Mock GIF search: hits served from the process (no network).
fn mock_hits(query: &str) -> Vec<GifHit> {
    let n = 8 + query.len() % 5;
    (0..n as u32)
        .map(|i| GifHit {
            id: format!("mock-{i}"),
            title: format!("{query} {i}"),
            tile_url: format!("mock://tile/{i}"),
            tile_size: (120, 90),
            mp4_url: "mock://clip.mp4".into(),
            size: (320, 180),
        })
        .collect()
}

/// Bytes of a GIPHY (or mock) URL. Blocking.
fn fetch(endpoint: &Endpoint, url: &str, max: u64) -> Result<Vec<u8>, SearchError> {
    if let Some(rest) = url.strip_prefix("mock://") {
        return Ok(match rest.strip_prefix("tile/") {
            Some(i) => crate::mock::gif_tile_webp(i.parse().unwrap_or(0)),
            None => crate::mock::gif_clip(),
        });
    }
    giphy::fetch(endpoint, url, max)
}

/// Writes `mp4` to the GIF shelf, checks it with the helper (H.264, size,
/// duration, poster) and decodes its bubble animation. Blocking (helper
/// processes): run it on a detached thread.
fn prepare_gif(
    shelf: &Shelf,
    chat: String,
    mp4: &[u8],
    source: GifSource,
) -> Result<Box<Prepared>, String> {
    if mp4.len() as u64 > shelf::MAX_GIF_BYTES {
        return Err("This GIF is larger than 16 MB.".into());
    }
    let helper = video::helper_path().ok_or_else(|| {
        "The ferrochat-player helper is missing, so GIFs cannot be prepared.".to_string()
    })?;
    let key = shelf::key_of(mp4);
    let path = shelf.path(Kind::Gif, &key);
    shelf.put(&path, mp4).map_err(|e| e.to_string())?;
    let probe = gifv::probe_with(&helper, &path).map_err(|f| gif_failure(&f))?;
    let (fw, fh) = probe.frame_size;
    let poster = crate::media::image::poster_jpeg(fw, fh, probe.rgba).map_err(|e| e.to_string())?;
    let _ = shelf.put(&shelf.poster_path(&key), &poster);
    let thumb = small_thumb(&poster).unwrap_or_else(|| poster.clone());
    let anim = decode_gif(&helper, &path).ok();
    let seconds = (probe.duration_ms / 1000).max(1) as u32;
    let item = ShelfItem {
        key,
        width: probe.width,
        height: probe.height,
        animated: true,
        seconds,
        source,
    };
    Ok(Box::new(Prepared {
        chat,
        media: OutgoingMedia {
            kind: OutgoingKind::Gif {
                width: probe.width,
                height: probe.height,
                seconds,
                thumbnail: thumb,
                source,
            },
            bytes: mp4.to_vec(),
            file_name: "gif.mp4".into(),
            mimetype: "video/mp4".into(),
            caption: None,
        },
        item,
        kind: Kind::Gif,
        still: None,
        anim,
    }))
}

/// The 96 px inline thumbnail sent with a GIF.
fn small_thumb(jpeg: &[u8]) -> Option<Vec<u8>> {
    crate::media::image::probe_and_thumbnail(jpeg)
        .ok()
        .map(|(_, _, t)| t)
}

fn gif_failure(f: &Failure) -> String {
    match f {
        Failure::MissingCodec(c) => format!("This video uses {c}; GIFs must be H.264 MP4s."),
        Failure::Unavailable(d) => format!("The video helper could not start ({d})."),
        Failure::Broken(d) => format!("This file cannot be sent as a GIF ({d})."),
    }
}

/// A GIF's bubble animation via the helper's frames mode. Blocking.
fn decode_gif(helper: &std::path::Path, path: &std::path::Path) -> Result<Arc<Anim>, String> {
    let lim = anim::GIF;
    let spec = gifv::FrameSpec {
        max_w: lim.target,
        max_h: lim.target,
        max_frames: lim.max_frames as u32,
        max_bytes: lim.max_bytes as u64,
    };
    let frames = gifv::decode_with(helper, path, spec).map_err(|f| f.detail())?;
    Ok(Arc::new(anim::from_rgba_frames(frames)))
}

/// A sticker's shelf entry, its bubble still (static) or animation.
type StickerParts = (ShelfItem, Option<(u32, u32, Vec<u8>)>, Option<Arc<Anim>>);

/// A sticker's shelf entry, bubble still and animation. Blocking.
fn sticker_parts(bytes: &[u8], key: String) -> Result<StickerParts, String> {
    let (w, h) = shelf::webp_size(bytes).ok_or("This sticker is not a WebP image.")?;
    match anim::decode(bytes, &anim::STICKER) {
        Ok(a) => Ok((ShelfItem::sticker(key, w, h, true), None, Some(Arc::new(a)))),
        Err(anim::AnimError::NotAnimated) => {
            let still =
                shelf::still_rgba(bytes, anim::STICKER.target).map_err(|e| e.to_string())?;
            Ok((ShelfItem::sticker(key, w, h, false), Some(still), None))
        }
        Err(e) => Err(format!("This sticker cannot be shown ({e:?}).")),
    }
}

/// Downloads a received sticker, keeps it on the shelf and decodes it
/// (animated ones fully, within [`anim::STICKER`]).
pub(super) async fn fetch_sticker(
    client: net::ClientHandle,
    mref: MediaRef,
    shelf: Shelf,
) -> Result<Fetched, String> {
    let bytes = client.download(&mref).await.map_err(|e| e.to_string())?;
    let key = mref.cache_key();
    tokio::task::spawn_blocking(move || {
        let (item, still, anim) = sticker_parts(&bytes, key)?;
        // A hex digest key only: never a path from the network.
        let seen = if item.valid() {
            shelf
                .put(&shelf.path(Kind::Sticker, &item.key), &bytes)
                .ok()
                .map(|()| item)
        } else {
            None
        };
        Ok(Fetched::Sticker {
            still: still.map(|(w, h, px)| (w, h, Arc::new(px))),
            anim,
            seen,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

impl App {
    /// The shelf root: the cache directory, or a private temporary
    /// directory in `--mock` (removed on exit).
    pub(super) fn shelf(&self) -> Shelf {
        if self.cfg.mock {
            Shelf::new(mock_shelf_root())
        } else {
            Shelf::new(self.cfg.cache_dir.clone())
        }
    }

    /// Handles [`PickerUi`].
    pub(super) fn on_picker(&mut self, ui: PickerUi) -> Task<Message> {
        match ui {
            PickerUi::Tab(tab) => {
                self.picker.tab = tab;
                self.picker_tab_opened()
            }
            PickerUi::SendSticker(key) => self.send_shelf_sticker(key),
            PickerUi::ToggleFavourite(key) => self.toggle_favourite_key(&key),
            PickerUi::FavouriteMessage(id) => {
                self.popup = None;
                self.favourite_message(&id)
            }
            PickerUi::CreateSticker => self.pick_sticker_picture(),
            PickerUi::DraftSend => self.send_draft(),
            PickerUi::DraftCancel => {
                self.picker.draft = None;
                Task::none()
            }
            PickerUi::Query(q) => {
                self.picker.query = q;
                self.picker.generation = self.picker.generation.wrapping_add(1);
                let generation = self.picker.generation;
                if self.picker.query.trim().is_empty() {
                    self.picker.results = None;
                    self.picker.hit_tiles = HashMap::new();
                    self.picker.tile_queue.clear();
                    self.picker.searching = false;
                    return Task::none();
                }
                Task::perform(
                    async move {
                        tokio::time::sleep(SEARCH_DEBOUNCE).await;
                        generation
                    },
                    |g| Message::Ui(Ui::Picker(PickerUi::SearchDue(g))),
                )
            }
            PickerUi::SearchDue(g) => self.run_search(g),
            PickerUi::SendHit(i) => self.send_hit(i),
            PickerUi::SendRecentGif(key) => self.send_recent_gif(key),
            PickerUi::SendGifFile => self.pick_gif_file(),
            PickerUi::KeyInput(k) => {
                self.picker.key_input = k;
                Task::none()
            }
            PickerUi::SaveKey => self.save_gif_key(),
            PickerUi::RemoveKey => self.remove_gif_key(),
            PickerUi::OpenSettings => {
                self.emoji_open = false;
                self.settings_open = true;
                self.load_gif_key()
            }
        }
    }

    /// The emoji button toggled the drawer.
    pub(super) fn picker_toggled(&mut self) -> Task<Message> {
        if self.emoji_open {
            self.picker.opened();
            self.picker_tab_opened()
        } else {
            self.picker.closed();
            Task::none()
        }
    }

    /// Frees the drawer's tiles once it is closed (any path closes it).
    pub(super) fn release_picker(&mut self) {
        if !self.emoji_open && (self.picker.holds_tiles() || self.picker.tab != Tab::Emoji) {
            self.picker.closed();
        }
    }

    /// A tab became visible: load what it shows.
    fn picker_tab_opened(&mut self) -> Task<Message> {
        match self.picker.tab {
            Tab::Emoji => Task::none(),
            Tab::Stickers => {
                let keys: Vec<String> = self
                    .picker
                    .shown_favourites
                    .iter()
                    .chain(&self.picker.shown_stickers)
                    .map(|i| i.key.clone())
                    .collect();
                self.load_tiles(Kind::Sticker, keys)
            }
            Tab::Gifs => {
                let keys = self
                    .picker
                    .shown_gifs
                    .iter()
                    .map(|i| i.key.clone())
                    .collect();
                Task::batch([self.load_tiles(Kind::Gif, keys), self.load_gif_key()])
            }
        }
    }

    fn load_tiles(&mut self, kind: Kind, keys: Vec<String>) -> Task<Message> {
        let shelf = self.shelf();
        let mut tasks = Vec::new();
        for key in keys {
            if self.picker.tiles.contains_key(&key)
                || !self.picker.tiles_loading.insert(key.clone())
            {
                continue;
            }
            let shelf = shelf.clone();
            tasks.push(blocking(
                move || {
                    let picture = match kind {
                        Kind::Sticker => shelf
                            .read(&shelf.path(kind, &key), crate::app::MAX_STICKER_BYTES)
                            .ok()
                            .and_then(|b| shelf::still_rgba(&b, (TILE * 1.5) as u32).ok())
                            .map(|(w, h, px)| TilePicture::Rgba(w, h, px)),
                        Kind::Gif => shelf
                            .read(&shelf.poster_path(&key), 1 << 20)
                            .ok()
                            .map(TilePicture::Encoded),
                    };
                    (key, picture)
                },
                |(key, picture)| wrap(PickerDone::Tile { key, picture }),
            ));
        }
        Task::batch(tasks)
    }

    /// Reads the GIPHY key once (mock mode has a built-in one).
    pub(super) fn load_gif_key(&mut self) -> Task<Message> {
        if !matches!(self.picker.key, KeyState::Unknown) {
            return Task::none();
        }
        if self.cfg.mock {
            self.picker.key = KeyState::Present(Secret::new(MOCK_KEY));
            return Task::none();
        }
        self.picker.key = KeyState::Loading;
        let secrets = self.secrets.clone();
        blocking(
            move || secrets.get(giphy::KEY_ACCOUNT).map_err(|e| e.to_string()),
            |r| wrap(PickerDone::Key(r)),
        )
    }

    fn save_gif_key(&mut self) -> Task<Message> {
        let key = self.picker.key_input.trim().to_string();
        if key.is_empty() || key.len() > 200 || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
            self.toast("That does not look like a GIPHY API key.");
            return Task::none();
        }
        self.picker.key_input.clear();
        let secrets = self.secrets.clone();
        blocking(
            move || {
                let s = Secret::new(key);
                secrets
                    .set(giphy::KEY_ACCOUNT, &s)
                    .map(|()| Some(s))
                    .map_err(|e| e.to_string())
            },
            |r| wrap(PickerDone::KeySaved(r)),
        )
    }

    fn remove_gif_key(&mut self) -> Task<Message> {
        let secrets = self.secrets.clone();
        blocking(
            move || {
                secrets
                    .delete(giphy::KEY_ACCOUNT)
                    .map(|()| None)
                    .map_err(|e| e.to_string())
            },
            |r| wrap(PickerDone::KeySaved(r)),
        )
    }

    fn run_search(&mut self, generation: u32) -> Task<Message> {
        if generation != self.picker.generation || !self.emoji_open {
            return Task::none();
        }
        let KeyState::Present(key) = &self.picker.key else {
            return Task::none();
        };
        let key = key.clone();
        let query = self.picker.query.trim().to_string();
        if query.is_empty() {
            return Task::none();
        }
        self.picker.searching = true;
        let mock = self.cfg.mock;
        Task::perform(
            crate::media::detached("gif-search", move || {
                if mock {
                    return Ok(mock_hits(&query));
                }
                giphy::search(&Endpoint::from_env(), key.expose(), &query)
                    .map_err(|e| e.user_message())
            }),
            move |r| {
                wrap(PickerDone::Results {
                    generation,
                    result: r.unwrap_or_else(|| Err("GIF search failed.".into())),
                })
            },
        )
    }

    fn pump_hit_tiles(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        while self.picker.tiles_inflight < MAX_TILE_FETCHES {
            let Some(hit) = self.picker.tile_queue.pop_front() else {
                break;
            };
            self.picker.tiles_inflight += 1;
            let generation = self.picker.generation;
            let id = hit.id.clone();
            tasks.push(Task::perform(
                crate::media::detached("gif-tile", move || {
                    let bytes =
                        fetch(&Endpoint::from_env(), &hit.tile_url, giphy::MAX_TILE).ok()?;
                    anim::decode(&bytes, &anim::TILE).ok().map(Arc::new)
                }),
                move |anim| {
                    wrap(PickerDone::HitTile {
                        generation,
                        id,
                        anim: anim.flatten(),
                    })
                },
            ));
        }
        Task::batch(tasks)
    }

    fn send_shelf_sticker(&mut self, key: String) -> Task<Message> {
        let Some(chat) = self.open_chat_id() else {
            return Task::none();
        };
        let shelf = self.shelf();
        blocking(
            move || {
                let bytes = shelf
                    .read(
                        &shelf.path(Kind::Sticker, &key),
                        crate::app::MAX_STICKER_BYTES,
                    )
                    .map_err(|_| "This sticker is no longer on this computer.".to_string())?;
                prepare_sticker(chat, bytes)
            },
            |r| wrap(PickerDone::Ready(r)),
        )
    }

    fn send_draft(&mut self) -> Task<Message> {
        let Some(draft) = self.picker.draft.take() else {
            return Task::none();
        };
        let Some(chat) = self.open_chat_id() else {
            return Task::none();
        };
        let shelf = self.shelf();
        blocking(
            move || {
                let bytes = (*draft.webp).clone();
                let key = shelf::key_of(&bytes);
                shelf
                    .put(&shelf.path(Kind::Sticker, &key), &bytes)
                    .map_err(|e| e.to_string())?;
                prepare_sticker(chat, bytes)
            },
            |r| wrap(PickerDone::Ready(r)),
        )
    }

    fn pick_sticker_picture(&mut self) -> Task<Message> {
        Task::perform(
            async {
                let file = rfd::AsyncFileDialog::new()
                    .set_title("Create a sticker from a picture")
                    .add_filter("Pictures", &["png", "jpg", "jpeg", "webp", "gif"])
                    .pick_file()
                    .await?;
                let path = file.path().to_path_buf();
                tokio::task::spawn_blocking(move || make_draft(&path))
                    .await
                    .ok()
            },
            |r| match r {
                Some(r) => wrap(PickerDone::Draft(r)),
                None => Message::done(Done::Nothing),
            },
        )
    }

    fn send_hit(&mut self, i: usize) -> Task<Message> {
        let Some(hit) = self
            .picker
            .results
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|h| h.get(i))
            .cloned()
        else {
            return Task::none();
        };
        let Some(chat) = self.open_chat_id() else {
            return Task::none();
        };
        self.picker.busy = Some("Sending GIF…".into());
        let shelf = self.shelf();
        Task::perform(
            crate::media::detached("gif-send", move || {
                let mp4 = fetch(&Endpoint::from_env(), &hit.mp4_url, giphy::MAX_MP4)
                    .map_err(|e| e.user_message())?;
                prepare_gif(&shelf, chat, &mp4, GifSource::Giphy)
            }),
            |r| {
                wrap(PickerDone::Ready(
                    r.unwrap_or_else(|| Err("GIF failed.".into())),
                ))
            },
        )
    }

    fn send_recent_gif(&mut self, key: String) -> Task<Message> {
        let Some(chat) = self.open_chat_id() else {
            return Task::none();
        };
        let source = self
            .picker
            .recent_gifs
            .iter()
            .find(|i| i.key == key)
            .map(|i| i.source)
            .unwrap_or_default();
        let shelf = self.shelf();
        Task::perform(
            crate::media::detached("gif-send", move || {
                let mp4 = shelf
                    .read(&shelf.path(Kind::Gif, &key), shelf::MAX_GIF_BYTES)
                    .map_err(|_| "This GIF is no longer on this computer.".to_string())?;
                prepare_gif(&shelf, chat, &mp4, source)
            }),
            |r| {
                wrap(PickerDone::Ready(
                    r.unwrap_or_else(|| Err("GIF failed.".into())),
                ))
            },
        )
    }

    fn pick_gif_file(&mut self) -> Task<Message> {
        let Some(chat) = self.open_chat_id() else {
            return Task::none();
        };
        let shelf = self.shelf();
        Task::perform(
            async move {
                let file = rfd::AsyncFileDialog::new()
                    .set_title("Send a GIF")
                    .add_filter("GIF or MP4", &["gif", "mp4"])
                    .pick_file()
                    .await?;
                let path = file.path().to_path_buf();
                crate::media::detached("gif-file", move || gif_from_file(&shelf, chat, &path)).await
            },
            |r| match r {
                Some(r) => wrap(PickerDone::Ready(r)),
                None => Message::done(Done::Nothing),
            },
        )
    }

    fn toggle_favourite_key(&mut self, key: &str) -> Task<Message> {
        let item = self
            .picker
            .favourites
            .iter()
            .chain(&self.picker.recent_stickers)
            .find(|i| i.key == key)
            .cloned();
        match item {
            Some(item) => self.toggle_sticker_favourite(item),
            None => Task::none(),
        }
    }

    fn favourite_message(&mut self, id: &str) -> Task<Message> {
        let Some(Content::Sticker {
            width,
            height,
            animated,
            media: Some(m),
        }) = self.find_message(id).map(|m| m.content.clone())
        else {
            return Task::none();
        };
        let item = ShelfItem::sticker(m.cache_key(), width, height, animated);
        if !item.valid() {
            return Task::none();
        }
        let shelf = self.shelf();
        let client = self.client.clone();
        let fav = self.toggle_sticker_favourite(item.clone());
        // Make sure the file is on the shelf (it is once the bubble showed).
        let ensure = Task::perform(
            async move {
                let path = shelf.path(Kind::Sticker, &item.key);
                if path.is_file() {
                    return;
                }
                if let Some(c) = client
                    && let Ok(bytes) = c.download(&m).await
                {
                    let _ = tokio::task::spawn_blocking(move || shelf.put(&path, &bytes)).await;
                }
            },
            |()| Message::done(Done::Nothing),
        );
        Task::batch([fav, ensure])
    }

    fn toggle_sticker_favourite(&mut self, item: ShelfItem) -> Task<Message> {
        let now = shelf::toggle_favourite(&mut self.picker.favourites, item);
        self.toast(if now {
            "Added to favourite stickers"
        } else {
            "Removed from favourite stickers"
        });
        self.persist_lists()
    }

    /// Writes the three lists and prunes the shelves to them.
    fn persist_lists(&self) -> Task<Message> {
        let p = &self.picker;
        let rs = shelf::encode_list(&p.recent_stickers);
        let fs = shelf::encode_list(&p.favourites);
        let rg = shelf::encode_list(&p.recent_gifs);
        let keep_stickers = MediaPicker::keep(&[&p.recent_stickers, &p.favourites]);
        let keep_gifs = MediaPicker::keep(&[&p.recent_gifs]);
        let shelf = self.shelf();
        let order = Arc::clone(&p.saves);
        let ticket = order.ticket();
        let save = store_task(&self.store, move |s| {
            order.write_if_newest(ticket, || {
                s.set_kv(shelf::RECENT_STICKERS_KEY, &rs)?;
                s.set_kv(shelf::FAVOURITE_STICKERS_KEY, &fs)?;
                s.set_kv(shelf::RECENT_GIFS_KEY, &rg)
            })
        });
        let prune = blocking(
            move || {
                let ks: Vec<&str> = keep_stickers.iter().map(String::as_str).collect();
                shelf.prune(Kind::Sticker, &ks, shelf::STICKER_SHELF_BYTES);
                let kg: Vec<&str> = keep_gifs.iter().map(String::as_str).collect();
                shelf.prune(Kind::Gif, &kg, shelf::GIF_SHELF_BYTES);
            },
            |()| Message::done(Done::Nothing),
        );
        Task::batch([save, prune])
    }

    /// A received sticker was downloaded: show it and remember it.
    pub(super) fn on_sticker_fetched(
        &mut self,
        key: String,
        still: Option<(u32, u32, Arc<Vec<u8>>)>,
        anim: Option<Arc<Anim>>,
        seen: Option<ShelfItem>,
    ) -> Task<Message> {
        if let Some(a) = anim {
            let size = a.bytes;
            self.anims.put(key.clone(), a, size);
        }
        if let Some((w, h, px)) = still {
            let size = px.len();
            let owned = Arc::try_unwrap(px).unwrap_or_else(|a| (*a).clone());
            self.images.put(key, Handle::from_rgba(w, h, owned), size);
        }
        let changed = seen.is_some_and(|item| {
            shelf::note_seen(
                &mut self.picker.recent_stickers,
                item,
                shelf::MAX_RECENT_STICKERS,
            )
        });
        if changed {
            self.persist_lists()
        } else {
            Task::none()
        }
    }

    /// Received GIFs in the open chat get their inline animation: the file
    /// is downloaded to the GIF shelf and the helper's frames mode decodes
    /// it (at most [`MAX_GIF_DECODES`] at once, each tried once per run).
    pub(super) fn want_gif_anims(&mut self) -> Task<Message> {
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let Some(helper) = video::helper_path() else {
            return Task::none();
        };
        // (media, inline thumbnail, width, height, seconds)
        type Wanted = (MediaRef, Option<Vec<u8>>, u32, u32, u32);
        let wanted: Vec<Wanted> = self
            .open
            .iter()
            .flat_map(|c| c.messages.iter().rev().take(30))
            .filter_map(|m| match &m.content {
                Content::Video {
                    gif: true,
                    media: Some(r),
                    thumbnail,
                    width,
                    height,
                    seconds,
                    ..
                } if r.file_length <= shelf::MAX_GIF_BYTES => {
                    Some((r.clone(), thumbnail.clone(), *width, *height, *seconds))
                }
                _ => None,
            })
            .collect();
        let shelf = self.shelf();
        let mut tasks = Vec::new();
        for (mref, thumb, w, h, secs) in wanted {
            let key = mref.cache_key();
            if self.gif_inflight >= MAX_GIF_DECODES {
                break;
            }
            if self.anims.contains(&key) || !self.gif_tried.insert(key.clone()) {
                continue;
            }
            self.gif_inflight += 1;
            let (client, shelf, helper) = (client.clone(), shelf.clone(), helper.clone());
            let k = key.clone();
            tasks.push(Task::perform(
                async move {
                    let bytes = client.download(&mref).await.map_err(|e| e.to_string())?;
                    crate::media::detached("gif-decode", move || {
                        let valid = k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit());
                        let name = if valid {
                            k.clone()
                        } else {
                            shelf::key_of(&bytes)
                        };
                        let path = shelf.path(Kind::Gif, &name);
                        shelf.put(&path, &bytes).map_err(|e| e.to_string())?;
                        let anim = decode_gif(&helper, &path)?;
                        // The inline thumbnail is the recent-list tile.
                        if let Some(p) = &thumb {
                            let _ = shelf.put(&shelf.poster_path(&name), p);
                        }
                        let item = ShelfItem {
                            key: name,
                            width: w,
                            height: h,
                            animated: true,
                            seconds: secs,
                            source: GifSource::None,
                        };
                        Ok((anim, item))
                    })
                    .await
                    .unwrap_or_else(|| Err("GIF decoder stopped".into()))
                },
                move |result| wrap(PickerDone::GifAnim { key, result }),
            ));
        }
        Task::batch(tasks)
    }

    /// Handles [`PickerDone`].
    pub(super) fn on_picker_done(&mut self, d: PickerDone) -> Task<Message> {
        match d {
            PickerDone::Tile { key, picture } => {
                self.picker.tiles_loading.remove(&key);
                if !self.emoji_open {
                    return Task::none();
                }
                if let Some(p) = picture {
                    let handle = match p {
                        TilePicture::Rgba(w, h, px) => Handle::from_rgba(w, h, px),
                        TilePicture::Encoded(b) => Handle::from_bytes(b),
                    };
                    self.picker.tiles.insert(key, handle);
                }
            }
            PickerDone::Key(r) | PickerDone::KeySaved(r) => {
                self.picker.key = match r {
                    Ok(Some(k)) => KeyState::Present(k),
                    Ok(None) => KeyState::Missing,
                    Err(e) => {
                        tracing::warn!(error = %e, "GIPHY key not readable");
                        KeyState::Error(e)
                    }
                };
            }
            PickerDone::Results { generation, result } => {
                if generation != self.picker.generation || !self.emoji_open {
                    return Task::none();
                }
                self.picker.searching = false;
                self.picker.hit_tiles = HashMap::new();
                self.picker.tile_queue = result.iter().flatten().cloned().collect();
                self.picker.results = Some(result);
                return self.pump_hit_tiles();
            }
            PickerDone::HitTile {
                generation,
                id,
                anim,
            } => {
                self.picker.tiles_inflight = self.picker.tiles_inflight.saturating_sub(1);
                if generation == self.picker.generation
                    && self.emoji_open
                    && let Some(a) = anim
                {
                    self.picker.hit_tiles.insert(id, a);
                }
                return self.pump_hit_tiles();
            }
            PickerDone::Draft(r) => match r {
                Ok((webp, w, h, px)) => {
                    if self.emoji_open {
                        self.picker.draft = Some(Draft {
                            webp: Arc::new(webp),
                            handle: Handle::from_rgba(w, h, px),
                        });
                    }
                }
                Err(e) => self.toast(e),
            },
            PickerDone::Ready(r) => {
                self.picker.busy = None;
                return match r {
                    Ok(p) => self.send_prepared(*p),
                    Err(e) => {
                        self.toast(e);
                        Task::none()
                    }
                };
            }
            PickerDone::GifAnim { key, result } => {
                self.gif_inflight = self.gif_inflight.saturating_sub(1);
                match result {
                    Ok((a, item)) => {
                        let size = a.bytes;
                        self.anims.put(key, a, size);
                        if shelf::note_seen(
                            &mut self.picker.recent_gifs,
                            item,
                            shelf::MAX_RECENT_GIFS,
                        ) {
                            return Task::batch([self.persist_lists(), self.want_gif_anims()]);
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "GIF bubble could not be decoded"),
                }
                return self.want_gif_anims();
            }
        }
        Task::none()
    }

    fn send_prepared(&mut self, p: Prepared) -> Task<Message> {
        if self.open.as_ref().is_none_or(|c| c.chat != p.chat) {
            return Task::none();
        }
        let key = p.item.key.clone();
        if let Some(a) = p.anim {
            let size = a.bytes;
            self.anims.put(key.clone(), a, size);
        }
        if let Some((w, h, px)) = p.still {
            let size = px.len();
            self.images.put(key, Handle::from_rgba(w, h, px), size);
        }
        match p.kind {
            Kind::Sticker => shelf::bump(
                &mut self.picker.recent_stickers,
                p.item,
                shelf::MAX_RECENT_STICKERS,
            ),
            Kind::Gif => shelf::bump(&mut self.picker.recent_gifs, p.item, shelf::MAX_RECENT_GIFS),
        }
        Task::batch([self.persist_lists(), self.send_media(p.media)])
    }
}

/// A sticker from the shelf (or just made) as outgoing media. Blocking.
fn prepare_sticker(chat: String, bytes: Vec<u8>) -> Result<Box<Prepared>, String> {
    let key = shelf::key_of(&bytes);
    let (item, still, anim) = sticker_parts(&bytes, key)?;
    Ok(Box::new(Prepared {
        chat,
        media: OutgoingMedia {
            kind: OutgoingKind::Sticker {
                width: item.width,
                height: item.height,
                animated: item.animated,
            },
            bytes,
            file_name: "sticker.webp".into(),
            mimetype: "image/webp".into(),
            caption: None,
        },
        item,
        kind: Kind::Sticker,
        still,
        anim,
    }))
}

/// A picked picture -> sticker WebP plus its preview. Blocking.
fn make_draft(path: &std::path::Path) -> Result<(Vec<u8>, u32, u32, Vec<u8>), String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > 40 * 1024 * 1024 {
        return Err("This picture is too large.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let webp = shelf::make_sticker(&bytes).map_err(|e| format!("Cannot make a sticker: {e}"))?;
    let (w, h, px) = shelf::still_rgba(&webp, 160).map_err(|e| e.to_string())?;
    Ok((webp, w, h, px))
}

/// A picked `.gif` (converted by the helper) or `.mp4` -> outgoing GIF.
/// Blocking (helper processes).
fn gif_from_file(
    shelf: &Shelf,
    chat: String,
    path: &std::path::Path,
) -> Result<Box<Prepared>, String> {
    let is_gif = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    let mp4 = if is_gif {
        let helper = video::helper_path()
            .ok_or_else(|| "The ferrochat-player helper is missing.".to_string())?;
        let dir = shelf.dir(Kind::Gif);
        crate::config::secure_dir(&dir).map_err(|e| e.to_string())?;
        let out = dir.join(format!(".convert-{}.mp4", std::process::id()));
        let r = gifv::encode_gif_with(&helper, path, &out).map_err(|f| gif_failure(&f));
        let bytes = r.and_then(|()| std::fs::read(&out).map_err(|e| e.to_string()));
        let _ = std::fs::remove_file(&out);
        bytes?
    } else {
        let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if len > shelf::MAX_GIF_BYTES {
            return Err("This video is larger than 16 MB.".into());
        }
        std::fs::read(path).map_err(|e| e.to_string())?
    };
    prepare_gif(shelf, chat, &mp4, GifSource::None)
}

/// `--mock` keeps its shelf in a private temporary directory.
pub fn mock_shelf_root() -> PathBuf {
    std::env::temp_dir().join(format!("ferrochat-mock-shelf-{}", std::process::id()))
}

/// Removes the `--mock` shelf (nothing else of mock mode lands on disk).
pub fn remove_mock_shelf(cfg: &crate::config::Config) {
    if cfg.mock {
        let _ = std::fs::remove_dir_all(mock_shelf_root());
    }
}

/// Mock mode: recent stickers / GIFs to show (written to the mock shelf).
pub fn seed_mock(shelf: &Shelf) -> (Vec<ShelfItem>, Vec<ShelfItem>) {
    let mut stickers = Vec::new();
    for bytes in [
        crate::mock::animated_sticker_webp(),
        crate::mock::sticker_webp(),
    ] {
        let key = shelf::key_of(&bytes);
        let (w, h) = shelf::webp_size(&bytes).unwrap_or((512, 512));
        if shelf.put(&shelf.path(Kind::Sticker, &key), &bytes).is_ok() {
            let animated = whatsapp_rust::webp::is_animated(&bytes);
            stickers.push(ShelfItem::sticker(key, w, h, animated));
        }
    }
    let mut gifs = Vec::new();
    let clip = crate::mock::gif_clip();
    let key = shelf::key_of(&clip);
    let poster = crate::media::image::probe_and_thumbnail(&crate::mock::sunset_png(320, 180))
        .ok()
        .map(|(_, _, t)| t);
    if shelf.put(&shelf.path(Kind::Gif, &key), &clip).is_ok() {
        if let Some(p) = poster {
            let _ = shelf.put(&shelf.poster_path(&key), &p);
        }
        gifs.push(ShelfItem {
            key,
            width: 320,
            height: 180,
            animated: true,
            seconds: 3,
            source: GifSource::None,
        });
    }
    (stickers, gifs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_list_save_never_overwrites_a_newer_one() {
        let order = SaveOrder::default();
        let (old, new) = (order.ticket(), order.ticket());
        let mut writes = Vec::new();
        order
            .write_if_newest(new, || {
                writes.push("new");
                Ok(())
            })
            .unwrap();
        order
            .write_if_newest(old, || {
                writes.push("old");
                Ok(())
            })
            .unwrap();
        assert_eq!(writes, ["new"]);
    }

    #[test]
    fn closing_the_drawer_frees_the_tiles() {
        let mut p = MediaPicker {
            tab: Tab::Gifs,
            ..Default::default()
        };
        p.tiles
            .insert("k".into(), Handle::from_rgba(1, 1, vec![0; 4]));
        p.results = Some(Ok(mock_hits("cat")));
        p.query = "cat".into();
        assert!(p.holds_tiles());
        p.closed();
        assert!(!p.holds_tiles());
        assert_eq!(p.tab, Tab::Emoji);
        assert!(p.query.is_empty());
    }

    #[test]
    fn the_key_state_never_prints_the_key() {
        let k = KeyState::Present(Secret::new("SECRETKEY"));
        assert!(!format!("{k:?}").contains("SECRETKEY"));
    }

    #[test]
    fn mock_search_serves_tiles_and_a_clip_without_network() {
        let hits = mock_hits("cats");
        assert!(hits.len() >= 8);
        let ep = Endpoint::giphy();
        let tile = fetch(&ep, &hits[0].tile_url, giphy::MAX_TILE).unwrap();
        assert!(anim::decode(&tile, &anim::TILE).is_ok());
        assert!(fetch(&ep, &hits[0].mp4_url, giphy::MAX_MP4).unwrap().len() > 1000);
    }
}
