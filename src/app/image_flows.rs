//! Photo bubbles end to end: own sends, download order, the preview disk
//! cache, retries and placeholders (real `App` + fake client).

use iced::widget::image::Handle;

use super::Ui;
use super::message::{Done, Message};
use super::paste::{RawImage, Staged};
use super::test_support::Harness;
use crate::media::picture::{PLACEHOLDER_SIDE, PREVIEW_SIDE, PreviewCache};
use crate::mock::{self, SHOWCASE_CHAT};
use crate::net::{Content, MediaKind, MediaRef, Message as ChatMessage, ReceiptStatus};

const PABLO: &str = "34655555555@s.whatsapp.net";
const SUNSET: &str = "/mock/sunset.png";

/// Width and height of a decoded (RGBA) handle; `None` for encoded bytes.
fn rgba_size(h: &Handle) -> Option<(u32, u32)> {
    match h {
        Handle::Rgba { width, height, .. } => Some((*width, *height)),
        _ => None,
    }
}

fn jpeg(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([shade, (x % 255) as u8, (y % 255) as u8])
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::Cursor::new(&mut out), 80)
        .encode_image(&img)
        .unwrap();
    out
}

/// An incoming photo served by the fake CDN at `/test/<id>`.
fn photo(h: &Harness, id: &str, ts: i64, shade: u8) -> ChatMessage {
    let bytes = jpeg(1600, 1200, shade);
    let path = format!("/test/{id}");
    let (_, _, thumb) = crate::media::image::probe_and_thumbnail(&bytes).unwrap();
    let media = MediaRef {
        direct_path: path.clone(),
        media_key: vec![7; 32],
        file_sha256: <sha2::Sha256 as sha2::Digest>::digest(&bytes).to_vec(),
        file_enc_sha256: vec![0; 32],
        file_length: bytes.len() as u64,
        kind: MediaKind::Image,
        mimetype: "image/jpeg".into(),
    };
    h.fake.put_media(&path, bytes);
    ChatMessage {
        id: id.into(),
        chat: PABLO.into(),
        sender: PABLO.into(),
        sender_name: None,
        from_me: false,
        timestamp: ts,
        content: Content::Image {
            caption: None,
            width: 1600,
            height: 1200,
            thumbnail: Some(thumb),
            media: Some(media),
        },
        status: ReceiptStatus::Read,
        expires_at: None,
        reply: None,
        edited: false,
        mentions: Vec::new(),
        reactions: Vec::new(),
        forwarded: None,
    }
}

fn key_of(h: &Harness, id: &str) -> String {
    let m = h.app.find_message(id).expect("message open");
    h.app.image_key(m).expect("photo key")
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ferrochat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[tokio::test]
async fn a_sent_photo_is_sharp_at_once_and_never_downloaded_back() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(PABLO.into())).await;
    let staged = Staged::from_image(&RawImage {
        width: 1200,
        height: 900,
        rgba: vec![140; 1200 * 900 * 4],
    });
    let t = h.app.update(Message::done(Done::Staged {
        chat: PABLO.into(),
        staged: Some(Box::new(staged)),
    }));
    h.pump(t).await;
    h.ui(Ui::AttachSend).await;
    let last = h
        .app
        .open
        .as_ref()
        .unwrap()
        .messages
        .back()
        .unwrap()
        .clone();
    assert!(last.from_me);
    let Content::Image { media: Some(r), .. } = &last.content else {
        panic!("not an uploaded photo: {:?}", last.content);
    };
    // The key made from the file is the one the upload reports.
    assert_eq!(h.app.image_key(&last), Some(r.cache_key()));
    let shown = h
        .app
        .images
        .peek(&r.cache_key())
        .expect("full picture shown");
    assert_eq!(
        rgba_size(shown),
        Some((PREVIEW_SIDE, 495)),
        "decoded, scaled"
    );
    assert!(h.fake.downloads().is_empty(), "nothing downloaded back");
    assert!(h.app.pictures.local_keys.is_empty(), "upload key took over");
}

#[tokio::test]
async fn a_photo_in_flight_is_shown_under_its_file_key() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(PABLO.into())).await;
    // Before the upload returns, the bubble has no media reference yet.
    let bytes = jpeg(900, 600, 30);
    let t = h.app.preview_own_photo("LOCAL1", bytes.clone());
    h.pump(t).await;
    let key = super::pictures::key_of(&bytes);
    assert_eq!(
        h.app.pictures.local_keys.get("LOCAL1"),
        Some(&key),
        "pending bubble finds its picture"
    );
    assert_eq!(
        h.app.images.peek(&key).and_then(rgba_size),
        Some((PREVIEW_SIDE, 440))
    );
}

#[tokio::test]
async fn photos_on_screen_jump_the_queue_newest_first() {
    let mut h = Harness::new().await;
    // 60 messages: two old photos outside the newest 30, two new ones.
    let mut msgs = mock::long_history(PABLO, 60, 1_700_000_000);
    for m in &mut msgs {
        m.chat = PABLO.into();
    }
    let old1 = photo(&h, "OLD1", msgs[5].timestamp + 1, 10);
    let old2 = photo(&h, "OLD2", msgs[8].timestamp + 1, 20);
    let new1 = photo(&h, "NEW1", msgs[55].timestamp + 1, 30);
    let new2 = photo(&h, "NEW2", msgs[58].timestamp + 1, 40);
    msgs.extend([old1, old2, new1, new2]);
    h.app.store.insert_messages(&msgs).unwrap();
    // Every slot busy (slow downloads elsewhere): requests queue up.
    for i in 0..crate::media::MAX_CONCURRENT_DOWNLOADS {
        h.app
            .pictures
            .downloading
            .insert(format!("busy{i}"), std::time::Instant::now());
    }
    h.ui(Ui::OpenChat(PABLO.into())).await;
    let queued: Vec<&str> = h.app.pictures.queue.ids().collect();
    assert_eq!(queued, ["NEW2", "NEW1"], "background pass: newest first");
    // The old photos were scrolled into view.
    let t = h.app.update(Message::done(Done::Visible(vec![
        "OLD2".into(),
        "OLD1".into(),
    ])));
    h.pump(t).await;
    let queued: Vec<&str> = h.app.pictures.queue.ids().collect();
    assert_eq!(queued, ["OLD2", "OLD1", "NEW2", "NEW1"]);
    // Slots free up: everything arrives, on-screen photos first.
    h.app.pictures.downloading.clear();
    let t = h.app.pump_downloads();
    h.pump(t).await;
    let order = h.fake.downloads();
    assert_eq!(&order[..2], ["/test/OLD2", "/test/OLD1"]);
    for id in ["OLD1", "OLD2", "NEW1", "NEW2"] {
        let k = key_of(&h, id);
        assert!(h.app.images.contains(&k), "{id} shown");
    }
}

#[tokio::test]
async fn an_older_page_scrolled_into_view_gets_its_photos() {
    let mut h = Harness::new().await;
    let mut msgs = mock::long_history(PABLO, 40, 1_700_000_000);
    for m in &mut msgs {
        m.chat = PABLO.into();
    }
    msgs.push(photo(&h, "OLDPIC", msgs[2].timestamp + 1, 50));
    h.app.store.insert_messages(&msgs).unwrap();
    h.ui(Ui::OpenChat(PABLO.into())).await;
    // Not among the newest 30: only fetched once it is on screen.
    assert!(!h.app.images.contains(&key_of(&h, "OLDPIC")));
    assert!(h.fake.downloads().is_empty());
    let t = h
        .app
        .update(Message::done(Done::Visible(vec!["OLDPIC".into()])));
    h.pump(t).await;
    assert!(h.app.images.contains(&key_of(&h, "OLDPIC")));
    assert_eq!(h.fake.downloads(), ["/test/OLDPIC"]);
}

#[tokio::test]
async fn previews_come_back_from_disk_after_a_restart() {
    let dir = temp_dir("previews-restart");
    let mut h = Harness::new().await;
    h.app.pictures.cache = Some(PreviewCache::new(dir.clone()));
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let id = h.app.viewer_ids().first().cloned().expect("showcase photo");
    let key = key_of(&h, &id);
    assert!(h.app.images.contains(&key));
    assert!(h.fake.downloads().iter().any(|p| p == SUNSET));

    // A new process: empty memory, same disk cache.
    let mut h = Harness::new().await;
    h.app.pictures.cache = Some(PreviewCache::new(dir.clone()));
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    assert!(h.app.images.contains(&key), "shown from the disk cache");
    assert!(
        !h.fake.downloads().iter().any(|p| p == SUNSET),
        "not downloaded again"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_failing_photo_is_retried_then_offers_retry() {
    let mut h = Harness::new().await;
    h.fake.fail_next_downloads(SUNSET, 3);
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let id = h.app.viewer_ids().first().cloned().unwrap();
    let key = key_of(&h, &id);
    let tries = |h: &Harness| h.fake.downloads().iter().filter(|p| *p == SUNSET).count();
    assert_eq!(tries(&h), 3, "first try + two automatic retries");
    assert!(h.app.download_failed(&key), "Retry offered");
    assert!(!h.app.images.contains(&key));
    // Scrolling or new messages do not hammer a failed download.
    h.ui(Ui::OpenChat(PABLO.into())).await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    assert_eq!(tries(&h), 3);
    h.ui(Ui::RetryImage(id)).await;
    assert!(h.app.images.contains(&key));
    assert!(!h.app.download_failed(&key));
    assert_eq!(tries(&h), 4);
}

#[tokio::test]
async fn placeholders_are_small_blurred_pixels_made_off_the_ui_thread() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let id = h.app.viewer_ids().first().cloned().unwrap();
    let (w, hgt) = rgba_size(h.app.thumbs.get(&id).expect("placeholder")).expect("decoded");
    assert_eq!(w.max(hgt), PLACEHOLDER_SIDE);
}

#[tokio::test]
async fn the_viewer_shows_the_preview_then_the_original() {
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let id = h.app.viewer_ids().first().cloned().unwrap();
    assert!(h.app.images.contains(&key_of(&h, &id)));
    h.ui(Ui::OpenImage(id)).await;
    // The original bytes (for "Save as"), not the bubble's RGBA preview.
    let full = h.app.viewer_handle().expect("original loaded");
    assert!(matches!(full, Handle::Bytes(..)));
}

#[tokio::test]
async fn the_visibility_probe_finds_photo_rows_in_the_real_layout() {
    use iced::advanced::renderer::Headless;
    use iced::advanced::widget::Operation;
    use iced::advanced::widget::operation::{self, Outcome};
    use iced_runtime::user_interface::{Cache, UserInterface};

    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    // As if nothing had loaded yet.
    h.app.images.clear();
    let photo = h.app.viewer_ids().last().cloned().expect("showcase photo");
    let mut probe = h.app.visible_probe().expect("photo rows to look for");
    let mut renderer = <iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(16.0),
        Some("tiny-skia"),
    )
    .await
    .expect("tiny-skia renderer");
    let mut look = |height: f32, probe: &mut super::pictures::VisibleRows| {
        let el = crate::ui::view(&h.app);
        let size = iced::Size::new(1280.0, height);
        let mut ui = UserInterface::build(el, size, Cache::default(), &mut renderer);
        ui.operate(&renderer, &mut operation::black_box(probe));
        match probe.finish() {
            Outcome::Some(ids) => ids,
            _ => panic!("no outcome"),
        }
    };
    // 800 px tall: the newest messages are text, the photos are further up.
    assert!(look(800.0, &mut probe.clone()).is_empty());
    // A window tall enough to show the whole chat shows the photo.
    let ids = look(4000.0, &mut probe);
    assert!(ids.contains(&photo), "{photo} not found in {ids:?}");
}

/// Renders the showcase photo bubble in each loading state to PNGs (for
/// the PR): `SHOTS=<dir> cargo test --lib bubble_states -- --ignored`.
#[tokio::test]
#[ignore = "writes screenshots"]
async fn bubble_states() {
    use iced::advanced::renderer::{Headless, Style};
    use iced::mouse::Cursor;
    use iced_runtime::user_interface::{Cache, UserInterface};

    let dir = std::path::PathBuf::from(std::env::var("SHOTS").unwrap_or_else(|_| ".".into()));
    let mut h = Harness::new().await;
    h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
    let id = h.app.viewer_ids().last().cloned().unwrap();
    let key = key_of(&h, &id);
    let full = h.app.images.peek(&key).cloned().unwrap();
    let blurred = h.app.thumbs.get(&id).cloned().unwrap();
    let raw = match &h.app.find_message(&id).unwrap().content {
        Content::Image { thumbnail, .. } => Handle::from_bytes(thumbnail.clone().unwrap()),
        _ => unreachable!(),
    };
    let mut renderer = <iced::Renderer as Headless>::new(
        iced::Font::DEFAULT,
        iced::Pixels(16.0),
        Some("tiny-skia"),
    )
    .await
    .unwrap();
    let theme = h.app.iced_theme(iced::window::Id::unique());
    let size = iced::Size::new(1280.0, 2400.0);
    let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(5);
    // (file, picture, placeholder, downloading, failed)
    type State = (&'static str, Option<Handle>, Option<Handle>, bool, bool);
    let states: [State; 4] = [
        ("before-thumbnail", None, Some(raw), false, false),
        ("after-loading", None, Some(blurred.clone()), true, false),
        ("after-failed", None, Some(blurred), false, true),
        ("after-full", Some(full), None, false, false),
    ];
    for (name, img, thumb, loading, failed) in states {
        h.app.images.clear();
        h.app.pictures.downloading.clear();
        h.app.pictures.failed.clear();
        if let Some(i) = img {
            h.app.images.put(key.clone(), i, 1);
        }
        if let Some(t) = thumb {
            h.app.thumbs.insert(id.clone(), t);
        }
        if loading {
            h.app.pictures.downloading.insert(key.clone(), long_ago);
        }
        if failed {
            h.app.pictures.failed.insert(key.clone());
        }
        let el = crate::ui::view(&h.app);
        let mut ui = UserInterface::build(el, size, Cache::default(), &mut renderer);
        let redraw = iced::Event::Window(iced::window::Event::RedrawRequested(
            std::time::Instant::now(),
        ));
        let mut msgs: Vec<Message> = Vec::new();
        let _ = ui.update(
            &[redraw],
            Cursor::Unavailable,
            &mut renderer,
            &mut iced::advanced::clipboard::Null,
            &mut msgs,
        );
        ui.draw(
            &mut renderer,
            &theme,
            &Style::default(),
            Cursor::Unavailable,
        );
        let px = renderer.screenshot(
            iced::Size::new(size.width as u32, size.height as u32),
            1.0,
            iced::Color::BLACK,
        );
        image::RgbaImage::from_raw(size.width as u32, size.height as u32, px)
            .unwrap()
            .save(dir.join(format!("{name}.png")))
            .unwrap();
    }
}
