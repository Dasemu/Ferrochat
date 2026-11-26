//! App-level tests of stickers and GIFs (`app/picker.rs`): the real `App`
//! and `Store` against the fake client, mock GIF search, no network.

use super::Ui;
use super::picker::{KeyState, PickerUi, Tab};
use super::test_support::Harness;
use crate::media::sticker as shelf;
use crate::mock::SHOWCASE_CHAT;
use crate::net::fake::Sent;
use crate::net::{Content, GifSource};

fn picker(p: PickerUi) -> Ui {
    Ui::Picker(p)
}

fn helper_built() -> bool {
    let ok = crate::media::video::helper_path().is_some();
    if !ok {
        eprintln!("skipped: the ferrochat-player helper is not built");
    }
    ok
}

/// Media cache key of the showcase's animated sticker.
fn animated_sticker_key(h: &Harness) -> String {
    h.app
        .open
        .as_ref()
        .and_then(|c| {
            c.messages.iter().find_map(|m| match &m.content {
                Content::Sticker {
                    animated: true,
                    media: Some(r),
                    ..
                } => Some(r.cache_key()),
                _ => None,
            })
        })
        .expect("showcase animated sticker")
}

fn sent_media(h: &Harness) -> Vec<Content> {
    h.fake
        .log()
        .into_iter()
        .filter_map(|s| match s {
            Sent::Media { content, .. } => Some(content),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn received_animated_stickers_play_inline_and_become_recent() {
    let mut h = Harness::new().await;
    // Mock mode seeds the same sticker; start from an empty list.
    h.app.picker.recent_stickers.clear();
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let key = animated_sticker_key(&h);
    let anim = h.app.anims.peek(&key).expect("decoded every frame");
    assert!(anim.frames.len() >= 2);
    assert!(!h.app.images.contains(&key), "no still for an animated one");
    assert!(
        h.app.picker.recent_stickers.iter().any(|i| i.key == key),
        "seen stickers join the recent list"
    );
    // Persisted for the next start.
    let stored = h.app.store.kv(shelf::RECENT_STICKERS_KEY).unwrap().unwrap();
    assert!(stored.contains(&key));
}

#[tokio::test]
async fn a_recent_sticker_is_sent_as_a_sticker_message() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    h.ui(Ui::ToggleEmoji).await;
    h.ui(picker(PickerUi::Tab(Tab::Stickers))).await;
    let item = h.app.picker.shown_stickers[0].clone();
    assert!(
        h.app.picker.tiles.contains_key(&item.key),
        "tile loaded when the tab opened"
    );
    h.ui(picker(PickerUi::SendSticker(item.key.clone()))).await;
    let sent = sent_media(&h);
    let Some(Content::Sticker {
        animated,
        media: Some(m),
        ..
    }) = sent.last()
    else {
        panic!("no sticker sent: {sent:?}");
    };
    assert_eq!(*animated, item.animated);
    assert_eq!(m.kind, crate::net::MediaKind::Sticker);
    assert_eq!(m.mimetype, "image/webp");
    // The bubble shows at once (same key as the upload).
    assert_eq!(m.cache_key(), item.key);
    assert!(h.app.anims.contains(&item.key) || h.app.images.contains(&item.key));
    assert_eq!(h.app.picker.recent_stickers[0].key, item.key);
    // The open grid did not move under the pointer.
    assert_eq!(h.app.picker.shown_stickers[0].key, item.key);
    // Closing the drawer frees every tile.
    h.ui(Ui::ToggleEmoji).await;
    assert!(h.app.picker.tiles.is_empty());
    assert_eq!(h.app.picker.tab, Tab::Emoji);
}

#[tokio::test]
async fn stickers_are_favourited_from_the_message_menu_locally() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let key = animated_sticker_key(&h);
    let id = h
        .app
        .open
        .as_ref()
        .and_then(|c| {
            c.messages
                .iter()
                .find(|m| matches!(&m.content, Content::Sticker { animated: true, .. }))
        })
        .map(|m| m.id.clone())
        .unwrap();
    // Mock mode seeds this sticker as a favourite; start from none.
    h.app.picker.favourites.clear();
    let before = 0;
    h.ui(picker(PickerUi::FavouriteMessage(id.clone()))).await;
    assert!(h.app.picker.is_favourite(&key));
    assert_eq!(h.app.picker.favourites.len(), before + 1);
    let stored = h
        .app
        .store
        .kv(shelf::FAVOURITE_STICKERS_KEY)
        .unwrap()
        .unwrap();
    assert!(stored.contains(&key));
    // Nothing went to the phone: favourites are local only.
    assert!(
        h.fake
            .log()
            .iter()
            .all(|s| !matches!(s, Sent::Media { .. }))
    );
    h.ui(picker(PickerUi::FavouriteMessage(id))).await;
    assert!(!h.app.picker.is_favourite(&key));
}

#[tokio::test]
async fn received_gifs_play_inline_from_the_helper() {
    if !helper_built() {
        return;
    }
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let key = h
        .app
        .open
        .as_ref()
        .and_then(|c| {
            c.messages.iter().find_map(|m| match &m.content {
                Content::Video {
                    gif: true,
                    media: Some(r),
                    ..
                } => Some(r.cache_key()),
                _ => None,
            })
        })
        .expect("showcase GIF");
    let anim = h.app.anims.peek(&key).expect("GIF decoded for its bubble");
    assert!(anim.frames.len() >= 10, "{}", anim.frames.len());
    assert!(anim.size.0 <= super::anim::GIF.target);
    assert!(h.app.picker.recent_gifs.iter().any(|i| i.key == key));
}

#[tokio::test]
async fn a_gif_search_result_is_sent_as_a_gif_video() {
    if !helper_built() {
        return;
    }
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    h.ui(Ui::ToggleEmoji).await;
    h.ui(picker(PickerUi::Tab(Tab::Gifs))).await;
    assert!(matches!(h.app.picker.key, KeyState::Present(_)), "mock key");
    // The debounce timer fires through the task pump.
    h.ui(picker(PickerUi::Query("party".into()))).await;
    let hits = match &h.app.picker.results {
        Some(Ok(hits)) => hits.clone(),
        other => panic!("no results: {other:?}"),
    };
    assert!(!hits.is_empty());
    assert!(
        hits.iter()
            .all(|hit| h.app.picker.hit_tiles.contains_key(&hit.id)),
        "animated tiles"
    );
    h.ui(picker(PickerUi::SendHit(0))).await;
    let sent = sent_media(&h);
    let Some(Content::Video {
        gif: true,
        media: Some(m),
        thumbnail: Some(_),
        width,
        height,
        ..
    }) = sent.last()
    else {
        panic!("no GIF sent: {sent:?}");
    };
    assert_eq!(m.kind, crate::net::MediaKind::Video);
    assert_eq!(m.mimetype, "video/mp4");
    assert_eq!((*width, *height), (320, 180));
    let recent = &h.app.picker.recent_gifs[0];
    assert_eq!(recent.source, GifSource::Giphy);
    assert_eq!(recent.key, m.cache_key());
    // Typing a new query drops the old results' tiles.
    h.ui(picker(PickerUi::Query(String::new()))).await;
    assert!(h.app.picker.results.is_none());
    assert!(h.app.picker.hit_tiles.is_empty());
}

#[tokio::test]
async fn a_stale_search_answer_is_ignored() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    h.ui(Ui::ToggleEmoji).await;
    h.ui(picker(PickerUi::Tab(Tab::Gifs))).await;
    // An old generation's timer: no search runs.
    h.ui(picker(PickerUi::SearchDue(9999))).await;
    assert!(h.app.picker.results.is_none());
    assert!(!h.app.picker.searching);
}

#[tokio::test]
async fn the_giphy_key_is_saved_and_removed_through_the_secret_store() {
    let mut h = Harness::new().await;
    h.ui(picker(PickerUi::KeyInput("not a key!".into()))).await;
    h.ui(picker(PickerUi::SaveKey)).await;
    assert!(h.app.toast.is_some(), "a malformed key is refused");
    h.ui(picker(PickerUi::KeyInput("AbCdEf0123456789".into())))
        .await;
    h.ui(picker(PickerUi::SaveKey)).await;
    assert!(h.app.picker.has_key());
    assert!(h.app.picker.key_input.is_empty(), "the field is cleared");
    let stored = h
        .app
        .secrets
        .get(crate::net::giphy::KEY_ACCOUNT)
        .unwrap()
        .unwrap();
    assert_eq!(stored.expose(), "AbCdEf0123456789");
    h.ui(picker(PickerUi::RemoveKey)).await;
    assert!(matches!(h.app.picker.key, KeyState::Missing));
    assert!(
        h.app
            .secrets
            .get(crate::net::giphy::KEY_ACCOUNT)
            .unwrap()
            .is_none()
    );
}
