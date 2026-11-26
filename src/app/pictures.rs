//! Photos in the open chat: what to download first, the disk cache of
//! bubble previews, own sends shown at once, retries and placeholders.
//!
//! Order of a photo bubble: blurred inline thumbnail -> preview from the
//! disk cache or the network (decoded and scaled off the UI thread). Photos
//! on screen are fetched first, newest first (`Done::Visible`, found by a
//! widget operation after scrolling / loading), then the newest of the
//! chat in the background. A failed download is retried twice, then the
//! bubble offers "Retry".

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::advanced::widget::{self, Operation, operation::Outcome};
use iced::widget::image;
use iced::{Rectangle, Task, Vector};
use sha2::Digest;

use super::actions::row_id;
use super::message::{Done, Fetched, Message};
use super::{App, CONVERSATION_SCROLL, MAX_STICKER_BYTES, media_fetch, picker};
use crate::media::picture::{self, Preview, PreviewCache};
use crate::net::{Content, MediaKind, Message as ChatMessage};

/// Wait after a scroll before looking for visible photos (one check per
/// burst of scroll events).
const VISIBLE_DEBOUNCE: Duration = Duration::from_millis(120);
/// A visibility check that never reported back (no window) is forgotten
/// after this.
const VISIBLE_STALE: Duration = Duration::from_secs(2);
/// A spinner appears on a photo whose download takes longer than this.
pub const SPINNER_DELAY: Duration = Duration::from_millis(700);

/// Delay before automatic retry number `attempt` (1-based) of a failed
/// download; `None`: give up and show "Retry".
pub fn retry_delay(attempt: u32) -> Option<Duration> {
    let base = if cfg!(test) {
        Duration::from_millis(5)
    } else {
        Duration::from_secs(2)
    };
    match attempt {
        1 => Some(base),
        2 => Some(base * 4),
        _ => None,
    }
}

/// Message ids waiting for a download slot, most urgent first.
#[derive(Debug, Default)]
pub struct DownloadQueue {
    q: VecDeque<String>,
}

impl DownloadQueue {
    /// Appends `id` (background work) unless it is queued already.
    pub fn push_back(&mut self, id: &str) {
        if !self.q.iter().any(|q| q == id) {
            self.q.push_back(id.to_string());
        }
    }

    /// Puts `id` first (moving it if it was queued).
    pub fn push_front(&mut self, id: &str) {
        self.q.retain(|q| q != id);
        self.q.push_front(id.to_string());
    }

    /// Moves the queued ones of `ids` to the front, in the order given.
    pub fn promote(&mut self, ids: &[String]) {
        for id in ids.iter().rev() {
            if self.q.iter().any(|q| q == id) {
                self.push_front(id);
            }
        }
    }

    /// Next id.
    pub fn pop_front(&mut self) -> Option<String> {
        self.q.pop_front()
    }

    /// Drops everything (another chat was opened).
    pub fn clear(&mut self) {
        self.q.clear();
    }

    /// Queued ids, first first.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.q.iter().map(String::as_str)
    }
}

/// Download / cache state of photos.
#[derive(Default)]
pub struct Pictures {
    /// Downloads waiting for a slot.
    pub queue: DownloadQueue,
    /// Keys being downloaded (they hold a slot), with their start time.
    pub downloading: HashMap<String, Instant>,
    /// Keys being read from the disk cache (no slot).
    pub disk_loading: HashSet<String>,
    /// Keys known not to be on disk (this chat).
    pub disk_missed: HashSet<String>,
    /// Failed attempts per key.
    pub failures: HashMap<String, u32>,
    /// Keys waiting for an automatic retry.
    pub retrying: HashSet<String>,
    /// Keys that failed for good: the bubble offers "Retry".
    pub failed: HashSet<String>,
    /// Cache key of own photos still uploading (message id -> key): the
    /// bubble shows the picture before the upload returns its reference.
    pub local_keys: HashMap<String, String>,
    /// Previews on disk (`None` in `--mock`).
    pub cache: Option<PreviewCache>,
    /// A visibility check is scheduled (since when).
    pub visible_pending: Option<Instant>,
}

/// Which photo rows are on screen (or within half a viewport of it), most
/// urgent first: visible rows bottom-up (newest first), then the others by
/// distance. `rows` are `(id, top, bottom)` in screen coordinates.
pub fn visible_order(rows: &[(String, f32, f32)], view_top: f32, view_bottom: f32) -> Vec<String> {
    let margin = (view_bottom - view_top).max(0.0) / 2.0;
    let mut on: Vec<&(String, f32, f32)> = Vec::new();
    let mut near: Vec<(f32, &String)> = Vec::new();
    for r in rows {
        let (top, bottom) = (r.1, r.2);
        if bottom > view_top && top < view_bottom {
            on.push(r);
        } else if bottom > view_top - margin && top < view_bottom + margin {
            let dist = if bottom <= view_top {
                view_top - bottom
            } else {
                top - view_bottom
            };
            near.push((dist, &r.0));
        }
    }
    on.sort_by(|a, b| b.2.total_cmp(&a.2));
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    on.into_iter()
        .map(|r| r.0.clone())
        .chain(near.into_iter().map(|(_, id)| id.clone()))
        .collect()
}

/// Collects the screen position of the photo rows of the conversation.
#[derive(Clone)]
pub(super) struct VisibleRows {
    scroll: widget::Id,
    rows: HashMap<widget::Id, String>,
    found: Vec<(String, Rectangle)>,
    area: Option<(Rectangle, Vector)>,
}

impl Operation<Vec<String>> for VisibleRows {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Vec<String>>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
        if let Some(m) = id.and_then(|id| self.rows.get(id)) {
            self.found.push((m.clone(), bounds));
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn widget::operation::Scrollable,
    ) {
        if id == Some(&self.scroll) {
            self.area = Some((bounds, translation));
        }
    }

    fn finish(&self) -> Outcome<Vec<String>> {
        let Some((viewport, translation)) = self.area else {
            return Outcome::Some(Vec::new());
        };
        let rows: Vec<(String, f32, f32)> = self
            .found
            .iter()
            .map(|(id, b)| {
                let top = b.y - translation.y;
                (id.clone(), top, top + b.height)
            })
            .collect();
        Outcome::Some(visible_order(
            &rows,
            viewport.y,
            viewport.y + viewport.height,
        ))
    }
}

/// Hex SHA-256 of a file: the cache key its `MediaRef` will have once
/// uploaded (`file_sha256` is the hash of the plaintext).
pub fn key_of(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

fn handle_of(p: Preview) -> (image::Handle, usize) {
    let size = p.bytes();
    (image::Handle::from_rgba(p.width, p.height, p.rgba), size)
}

impl App {
    /// The key a photo bubble's picture is cached under.
    pub fn image_key(&self, msg: &ChatMessage) -> Option<String> {
        msg.content
            .media()
            .map(|m| m.cache_key())
            .or_else(|| self.pictures.local_keys.get(&msg.id).cloned())
    }

    /// When the download of `key` started, while it runs.
    pub fn download_since(&self, key: &str) -> Option<Instant> {
        self.pictures.downloading.get(key).copied()
    }

    /// The download of `key` failed for good ("Retry" is offered).
    pub fn download_failed(&self, key: &str) -> bool {
        self.pictures.failed.contains(key)
    }

    /// Another chat was opened: its queue and disk misses start afresh.
    pub(super) fn reset_pictures_for_chat(&mut self) {
        self.pictures.queue.clear();
        self.pictures.disk_missed.clear();
        self.pictures.visible_pending = None;
    }

    /// Fetches a photo / sticker of the open chat: the disk cache first
    /// (no slot needed), then the network, at most
    /// [`crate::media::MAX_CONCURRENT_DOWNLOADS`] at a time; `urgent`
    /// requests jump the queue.
    pub(super) fn request_media(
        &mut self,
        id: &str,
        max_pixels: u64,
        urgent: bool,
    ) -> Task<Message> {
        let Some((mref, origin)) = self
            .find_message(id)
            .and_then(|m| m.content.media().map(|r| (r.clone(), self.media_origin(m))))
        else {
            return Task::none();
        };
        let key = mref.cache_key();
        let p = &mut self.pictures;
        if self.images.contains(&key)
            || self.anims.contains(&key)
            || p.downloading.contains_key(&key)
            || p.disk_loading.contains(&key)
            || p.retrying.contains(&key)
            || p.failed.contains(&key)
        {
            return Task::none();
        }
        if mref.kind == MediaKind::Image
            && let Some(cache) = p.cache.clone()
            && !p.disk_missed.contains(&key)
        {
            p.disk_loading.insert(key.clone());
            let id = id.to_string();
            let k = key.clone();
            return Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || cache.load(&k))
                        .await
                        .ok()
                        .flatten()
                },
                move |preview| {
                    Message::done(Done::DiskPreview {
                        id: id.clone(),
                        key: key.clone(),
                        max_pixels,
                        preview,
                    })
                },
            );
        }
        if p.downloading.len() >= crate::media::MAX_CONCURRENT_DOWNLOADS {
            if urgent {
                p.queue.push_front(id);
            } else {
                p.queue.push_back(id);
            }
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        if mref.kind == MediaKind::Sticker && mref.file_length > MAX_STICKER_BYTES {
            // Not a sticker anyone sends: show the placeholder.
            return Task::none();
        }
        self.pictures
            .downloading
            .insert(key.clone(), Instant::now());
        let id = id.to_string();
        if mref.kind == MediaKind::Sticker {
            // WebP: decoded (every frame of an animated one) off the UI
            // thread and kept on the sticker shelf for the picker.
            let shelf = self.shelf();
            return Task::perform(
                async move {
                    let r = picker::fetch_sticker(client, mref, shelf, Some(origin)).await;
                    (id, key, r)
                },
                |(id, key, result)| Message::done(Done::Downloaded { id, key, result }),
            );
        }
        let cache = self.pictures.cache.clone();
        Task::perform(
            async move {
                let r = async {
                    let bytes = media_fetch::download(&client, &mref, Some(&origin)).await?;
                    let k = mref.cache_key();
                    // Header probe (decompression bombs), decode, scale and
                    // store, all off the UI thread.
                    tokio::task::spawn_blocking(move || {
                        let p =
                            picture::preview_of(&bytes, max_pixels).map_err(|e| e.to_string())?;
                        if let Some(c) = cache
                            && let Err(e) = c.store(&k, &p)
                        {
                            tracing::debug!(error = %e, "preview not cached");
                        }
                        Ok(Fetched::Picture(p))
                    })
                    .await
                    .map_err(|e| e.to_string())?
                }
                .await;
                (id, key, r)
            },
            |(id, key, result)| Message::done(Done::Downloaded { id, key, result }),
        )
    }

    /// Starts queued downloads while slots are free.
    pub(super) fn pump_downloads(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        while self.pictures.downloading.len() < crate::media::MAX_CONCURRENT_DOWNLOADS {
            let Some(id) = self.pictures.queue.pop_front() else {
                break;
            };
            let cap = if self.viewer.as_ref().is_some_and(|v| v.id == id) {
                crate::media::MAX_IMAGE_PIXELS
            } else {
                crate::media::MAX_AUTO_IMAGE_PIXELS
            };
            tasks.push(self.request_media(&id, cap, false));
        }
        Task::batch(tasks)
    }

    /// A download finished.
    pub(super) fn on_downloaded(
        &mut self,
        id: String,
        key: String,
        result: Result<Fetched, String>,
    ) -> Task<Message> {
        self.pictures.downloading.remove(&key);
        let mut tasks = Vec::new();
        match result {
            Ok(Fetched::Picture(p)) => {
                self.pictures.failures.remove(&key);
                let (h, size) = handle_of(p);
                self.images.put(key, h, size);
            }
            Ok(Fetched::Sticker { still, anim, seen }) => {
                tasks.push(self.on_sticker_fetched(key, still, anim, seen));
            }
            Err(e) => {
                tracing::warn!(error = %e, "media download failed");
                let n = self.pictures.failures.entry(key.clone()).or_insert(0);
                *n += 1;
                match retry_delay(*n) {
                    Some(d) => {
                        self.pictures.retrying.insert(key.clone());
                        tasks.push(Task::perform(
                            async move { tokio::time::sleep(d).await },
                            move |()| {
                                Message::done(Done::RetryMedia {
                                    id: id.clone(),
                                    key: key.clone(),
                                })
                            },
                        ));
                    }
                    None => {
                        self.pictures.failed.insert(key);
                    }
                }
            }
        }
        tasks.push(self.pump_downloads());
        Task::batch(tasks)
    }

    /// The disk cache answered.
    pub(super) fn on_disk_preview(
        &mut self,
        id: String,
        key: String,
        max_pixels: u64,
        preview: Option<Preview>,
    ) -> Task<Message> {
        self.pictures.disk_loading.remove(&key);
        match preview {
            Some(p) => {
                let (h, size) = handle_of(p);
                self.images.put(key, h, size);
                Task::none()
            }
            None => {
                self.pictures.disk_missed.insert(key);
                self.request_media(&id, max_pixels, true)
            }
        }
    }

    /// An automatic retry is due.
    pub(super) fn on_retry_media(&mut self, id: String, key: String) -> Task<Message> {
        self.pictures.retrying.remove(&key);
        self.request_media(&id, crate::media::MAX_AUTO_IMAGE_PIXELS, true)
    }

    /// "Retry" on a photo whose download failed.
    pub(super) fn retry_image(&mut self, id: &str) -> Task<Message> {
        let Some(key) = self.find_message(id).and_then(|m| self.image_key(m)) else {
            return Task::none();
        };
        self.pictures.failed.remove(&key);
        self.pictures.failures.remove(&key);
        self.pictures.retrying.remove(&key);
        self.request_media(id, crate::media::MAX_AUTO_IMAGE_PIXELS, true)
    }

    /// Photos on screen: fetch them first, newest first.
    pub(super) fn on_visible(&mut self, ids: Vec<String>) -> Task<Message> {
        self.pictures.visible_pending = None;
        let tasks: Vec<_> = ids
            .iter()
            .map(|id| self.request_media(id, crate::media::MAX_AUTO_IMAGE_PIXELS, false))
            .collect();
        self.pictures.queue.promote(&ids);
        Task::batch(tasks)
    }

    /// Schedules a look at which photo rows are on screen (debounced: one
    /// per burst of scroll events; nothing when every photo is loaded).
    pub(super) fn check_visible(&mut self) -> Task<Message> {
        if self
            .pictures
            .visible_pending
            .is_some_and(|t| t.elapsed() < VISIBLE_STALE)
        {
            return Task::none();
        }
        let Some(probe) = self.visible_probe() else {
            return Task::none();
        };
        self.pictures.visible_pending = Some(Instant::now());
        let probe = Arc::new(probe);
        Task::future(async { tokio::time::sleep(VISIBLE_DEBOUNCE).await })
            .then(move |()| widget::operate((*probe).clone()))
            .map(|ids| Message::done(Done::Visible(ids)))
    }

    /// The operation that finds the open chat's photo rows still waiting
    /// for their picture; `None` when there are none.
    pub(super) fn visible_probe(&self) -> Option<VisibleRows> {
        let rows: HashMap<widget::Id, String> = self
            .open
            .as_ref()?
            .messages
            .iter()
            .filter_map(|m| match &m.content {
                Content::Image { media: Some(r), .. } | Content::Sticker { media: Some(r), .. }
                    if !self.images.contains(&r.cache_key())
                        && !self.anims.contains(&r.cache_key()) =>
                {
                    Some((row_id(&m.id), m.id.clone()))
                }
                _ => None,
            })
            .collect();
        (!rows.is_empty()).then(|| VisibleRows {
            scroll: widget::Id::new(CONVERSATION_SCROLL),
            rows,
            found: Vec::new(),
            area: None,
        })
    }

    /// Own photo being sent: its preview is made from the file we have, so
    /// the bubble is sharp at once (and nothing is downloaded back later).
    pub(super) fn preview_own_photo(&mut self, id: &str, bytes: Vec<u8>) -> Task<Message> {
        let key = key_of(&bytes);
        self.pictures.local_keys.insert(id.to_string(), key.clone());
        if self.images.contains(&key) {
            return Task::none();
        }
        let cache = self.pictures.cache.clone();
        let id = id.to_string();
        Task::perform(
            async move {
                let k = key.clone();
                let r = tokio::task::spawn_blocking(move || {
                    let p = picture::preview_of(&bytes, crate::media::MAX_IMAGE_PIXELS)
                        .map_err(|e| e.to_string())?;
                    if let Some(c) = cache {
                        let _ = c.store(&k, &p);
                    }
                    Ok(Fetched::Picture(p))
                })
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
                (id, key, r)
            },
            |(id, key, result)| match result {
                Ok(f) => Message::done(Done::Downloaded {
                    id,
                    key,
                    result: Ok(f),
                }),
                // Not decodable here: the bubble keeps its thumbnail.
                Err(_) => Message::done(Done::Nothing),
            },
        )
    }

    /// Blurred placeholders for photo bubbles, made off the UI thread from
    /// their inline thumbnails.
    pub(super) fn make_placeholders(&mut self, msgs: &[ChatMessage]) -> Task<Message> {
        let todo: Vec<(String, Vec<u8>)> = msgs
            .iter()
            .filter(|m| !self.thumbs.contains_key(&m.id))
            .filter_map(|m| match &m.content {
                Content::Image {
                    thumbnail: Some(t), ..
                } => Some((m.id.clone(), t.clone())),
                _ => None,
            })
            .collect();
        if todo.is_empty() {
            return Task::none();
        }
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    todo.into_iter()
                        .filter_map(|(id, t)| picture::blurred_placeholder(&t).map(|p| (id, p)))
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default()
            },
            |list| Message::done(Done::Placeholders(list)),
        )
    }

    pub(super) fn on_placeholders(&mut self, list: Vec<(String, Preview)>) {
        for (id, p) in list {
            let (h, _) = handle_of(p);
            self.thumbs.insert(id, h);
        }
    }
}

/// Media kinds `request_media` fetches automatically.
pub fn auto_fetched(c: &Content) -> bool {
    matches!(
        c,
        Content::Image { media: Some(_), .. } | Content::Sticker { media: Some(_), .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_twice_then_gives_up() {
        assert!(retry_delay(1).is_some());
        assert!(retry_delay(2) > retry_delay(1));
        assert_eq!(retry_delay(3), None);
    }

    #[test]
    fn the_queue_promotes_visible_photos_in_order() {
        let mut q = DownloadQueue::default();
        for id in ["a", "b", "c", "d"] {
            q.push_back(id);
        }
        q.push_back("a");
        assert_eq!(q.ids().collect::<Vec<_>>(), ["a", "b", "c", "d"]);
        q.promote(&["d".into(), "x".into(), "c".into()]);
        assert_eq!(q.ids().collect::<Vec<_>>(), ["d", "c", "a", "b"]);
        q.push_front("b");
        assert_eq!(q.pop_front().as_deref(), Some("b"));
    }

    #[test]
    fn visible_rows_come_first_bottom_up_then_the_nearest() {
        let rows = vec![
            ("far".to_string(), -2000.0, -1800.0),
            ("above".to_string(), -150.0, -50.0),
            ("top".to_string(), -20.0, 120.0),
            ("bottom".to_string(), 500.0, 700.0),
            ("below".to_string(), 900.0, 1000.0),
        ];
        assert_eq!(
            visible_order(&rows, 0.0, 800.0),
            ["bottom", "top", "above", "below"]
        );
    }
}
