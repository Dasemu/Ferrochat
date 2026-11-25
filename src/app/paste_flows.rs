//! Paste-to-send through the real widget tree: the composer is iced's
//! `text_editor`, focused by a click, and Ctrl+V reaches it the way the
//! runtime delivers it (the editor pastes the text flavour and captures the
//! key; `runtime_event` still turns it into `Ui::Paste`, whose focus check
//! is a widget operation run on the same tree).

use std::time::Duration;

use futures::StreamExt;
use iced::advanced::clipboard::{self, Clipboard};
use iced::advanced::renderer::Headless;
use iced::advanced::widget::operation::{Focusable, Operation, Outcome};
use iced::advanced::widget::{self, Id};
use iced::keyboard::{self, Key, Modifiers, key::Code, key::Physical};
use iced::mouse::{self, Cursor};
use iced::{Event, Pixels, Point, Rectangle, Size, Task};
use iced_runtime::Action;
use iced_runtime::user_interface::{Cache, UserInterface};

use super::paste::{self, RawImage};
use super::test_support::Harness;
use super::{COMPOSER_ID, Message, Ui};
use crate::mock::SHOWCASE_CHAT;

const URL: &str = "https://example.com/cat.png";
const WINDOW: Size = Size::new(1280.0, 800.0);

/// The system clipboard as the text editor sees it (text only).
struct TextClipboard(Option<String>);

impl Clipboard for TextClipboard {
    fn read(&self, kind: clipboard::Kind) -> Option<String> {
        (kind == clipboard::Kind::Standard)
            .then(|| self.0.clone())
            .flatten()
    }

    fn write(&mut self, _kind: clipboard::Kind, _contents: String) {}
}

/// What paste-to-send reads (arboard in the app): Firefox's "Copy image",
/// i.e. the image URL as text plus the pixels.
fn firefox_copy_image() -> paste::Clipboard {
    paste::classify(
        Vec::new(),
        Some(URL),
        |_| false,
        || {
            Some(RawImage {
                width: 4,
                height: 3,
                rgba: vec![200; 4 * 3 * 4],
            })
        },
    )
}

/// A screenshot tool: pixels only, no text flavour.
fn screenshot() -> paste::Clipboard {
    paste::classify(
        Vec::new(),
        None,
        |_| false,
        || {
            Some(RawImage {
                width: 4,
                height: 3,
                rgba: vec![90; 4 * 3 * 4],
            })
        },
    )
}

/// Finds where a focusable widget is laid out.
struct Locate {
    id: Id,
    found: Option<Rectangle>,
}

impl Operation<Rectangle> for Locate {
    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _state: &mut dyn Focusable) {
        if id == Some(&self.id) {
            self.found = Some(bounds);
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Rectangle>)) {
        operate(self);
    }

    fn finish(&self) -> Outcome<Rectangle> {
        self.found.map_or(Outcome::None, Outcome::Some)
    }
}

/// The harness plus a live widget tree of the main view, like a window.
struct Window {
    h: Harness,
    renderer: iced::Renderer,
    cache: Option<Cache>,
}

impl Window {
    async fn open_chat() -> Self {
        let mut h = Harness::new().await;
        h.ui(Ui::OpenChat(SHOWCASE_CHAT.into())).await;
        let renderer = futures::executor::block_on(<iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            Pixels(16.0),
            Some("tiny-skia"),
        ))
        .expect("tiny-skia renderer");
        Self {
            h,
            renderer,
            cache: Some(Cache::default()),
        }
    }

    /// Builds the tree from the current state, runs `f` on it, keeps the
    /// widget state (focus) for the next frame.
    fn with_ui<R>(
        &mut self,
        f: impl FnOnce(
            &mut UserInterface<'_, Message, iced::Theme, iced::Renderer>,
            &mut iced::Renderer,
        ) -> R,
    ) -> R {
        let cache = self.cache.take().unwrap_or_default();
        let mut ui = UserInterface::build(
            crate::ui::view(&self.h.app),
            WINDOW,
            cache,
            &mut self.renderer,
        );
        let r = f(&mut ui, &mut self.renderer);
        self.cache = Some(ui.into_cache());
        r
    }

    /// Delivers window events; returns their statuses and feeds the
    /// produced messages (plus what `runtime_event` makes of each event)
    /// into the app.
    async fn events(
        &mut self,
        events: &[Event],
        cursor: Cursor,
        cb: &mut dyn Clipboard,
    ) -> Vec<iced::event::Status> {
        let (msgs, statuses) = self.with_ui(|ui, r| {
            let mut msgs = Vec::new();
            let (_, statuses) = ui.update(events, cursor, r, cb, &mut msgs);
            (msgs, statuses)
        });
        let id = iced::window::Id::unique();
        let mut all = msgs;
        for (ev, status) in events.iter().zip(&statuses) {
            all.extend(super::runtime_event(ev.clone(), *status, id));
        }
        for m in all {
            let t = self.h.app.update(m);
            self.pump(t).await;
        }
        statuses
    }

    /// Like `Harness::pump`, but widget operations run on the live tree.
    async fn pump(&mut self, task: Task<Message>) {
        let mut pending = vec![task];
        let mut rounds = 0;
        while let Some(task) = pending.pop() {
            rounds += 1;
            assert!(rounds < 500, "task loop did not settle");
            let Some(stream) = iced_runtime::task::into_stream(task) else {
                continue;
            };
            futures::pin_mut!(stream);
            let mut msgs = Vec::new();
            while let Ok(Some(action)) =
                tokio::time::timeout(Duration::from_secs(5), stream.next()).await
            {
                match action {
                    Action::Output(m) => msgs.push(m),
                    Action::Widget(op) => self.with_ui(|ui, r| {
                        let mut op = Some(op);
                        while let Some(mut current) = op.take() {
                            ui.operate(r, current.as_mut());
                            if let Outcome::Chain(next) = current.finish() {
                                op = Some(next);
                            }
                        }
                    }),
                    _ => {}
                }
            }
            for m in msgs {
                pending.push(self.h.app.update(m));
            }
        }
    }

    fn composer_bounds(&mut self) -> Rectangle {
        self.with_ui(|ui, r| {
            let mut op = Locate {
                id: Id::new(COMPOSER_ID),
                found: None,
            };
            ui.operate(r, &mut widget::operation::black_box(&mut op));
            op.found.expect("the composer is laid out")
        })
    }

    async fn click_composer(&mut self) {
        let at = self.composer_bounds().center();
        let cursor = Cursor::Available(at);
        let click = [
            Event::Mouse(mouse::Event::CursorMoved { position: at }),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ];
        self.events(&click, cursor, &mut clipboard::Null).await;
    }

    /// Ctrl+V with the given text flavour on the clipboard.
    async fn ctrl_v(&mut self, text: Option<&str>) -> iced::event::Status {
        let press = Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Character("v".into()),
            modified_key: Key::Character("v".into()),
            physical_key: Physical::Code(Code::KeyV),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::CTRL,
            text: None,
            repeat: false,
        });
        let mut cb = TextClipboard(text.map(str::to_string));
        let cursor = Cursor::Available(Point::new(900.0, 400.0));
        self.events(&[press], cursor, &mut cb).await[0]
    }

    fn composer_focused(&mut self) -> bool {
        self.with_ui(|ui, r| {
            let mut op = widget::operation::focusable::is_focused(Id::new(COMPOSER_ID));
            ui.operate(r, &mut widget::operation::black_box(&mut op));
            matches!(op.finish(), Outcome::Some(true))
        })
    }
}

#[tokio::test]
async fn ctrl_v_of_a_browser_image_in_the_focused_editor_opens_the_preview() {
    let mut w = Window::open_chat().await;
    w.h.app.read_clipboard = firefox_copy_image;
    w.click_composer().await;
    assert!(w.composer_focused(), "a click focuses the text_editor");

    let status = w.ctrl_v(Some(URL)).await;
    // The editor takes the key (and pastes the URL) ...
    assert_eq!(status, iced::event::Status::Captured);
    // ... and the image still opens the preview, without the URL in the box.
    let preview = w.h.app.attach.as_ref().expect("attachment preview");
    assert_eq!(preview.chat, SHOWCASE_CHAT);
    assert_eq!(preview.items.len(), 1);
    assert!(preview.handle.is_some());
    assert_eq!(w.h.app.composer.text().trim(), "");
}

#[tokio::test]
async fn a_typed_draft_survives_pasting_an_image_next_to_its_url() {
    let mut w = Window::open_chat().await;
    w.h.app.read_clipboard = firefox_copy_image;
    w.click_composer().await;
    for c in "mira".chars() {
        let s = c.to_string();
        let ev = Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Character(s.as_str().into()),
            modified_key: Key::Character(s.as_str().into()),
            physical_key: Physical::Code(Code::KeyA),
            location: keyboard::Location::Standard,
            modifiers: Modifiers::empty(),
            text: Some(s.as_str().into()),
            repeat: false,
        });
        w.events(&[ev], Cursor::Unavailable, &mut clipboard::Null)
            .await;
    }
    assert_eq!(w.h.app.composer.text().trim(), "mira");
    w.ctrl_v(Some(URL)).await;
    assert!(w.h.app.attach.is_some());
    assert_eq!(w.h.app.composer.text().trim(), "mira");
}

#[tokio::test]
async fn a_screenshot_pastes_as_an_image_and_text_stays_text() {
    let mut w = Window::open_chat().await;
    w.click_composer().await;

    // Plain text: the editor pastes it, no preview.
    w.h.app.read_clipboard = || paste::classify(Vec::new(), Some("hola"), |_| false, || None);
    w.ctrl_v(Some("hola")).await;
    assert!(w.h.app.attach.is_none());
    assert_eq!(w.h.app.composer.text().trim(), "hola");

    // Pixels only (Spectacle, Flameshot): the preview opens.
    w.h.app.read_clipboard = screenshot;
    w.ctrl_v(None).await;
    assert!(w.h.app.attach.is_some());
    assert_eq!(w.h.app.composer.text().trim(), "hola");
}

#[tokio::test]
async fn paste_is_ignored_when_the_composer_is_not_focused() {
    let mut w = Window::open_chat().await;
    w.h.app.read_clipboard = screenshot;
    // Focus moved elsewhere (no widget has this id: everything unfocuses).
    w.with_ui(|ui, r| {
        let mut op = widget::operation::focusable::focus::<()>(Id::new("elsewhere"));
        ui.operate(r, &mut op);
    });
    assert!(!w.composer_focused());
    w.ctrl_v(None).await;
    assert!(w.h.app.attach.is_none());
}
