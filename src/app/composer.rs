//! The message box: a multi-line `text_editor` buffer with WhatsApp
//! Desktop's keys (Enter sends, Shift+Enter inserts a newline) and the
//! per-chat drafts.
//!
//! The composer used to be a single-line `text_input`, whose `on_submit`
//! fires on Enter whatever the modifiers: Shift+Enter sent the message and a
//! newline could not be typed at all.

use std::collections::HashMap;
use std::sync::Arc;

use iced::keyboard::{Key, key::Named};
use iced::widget::text_editor::{
    Action, Binding, Content, Cursor, Edit, KeyPress, Position, Status,
};

/// Drafts kept for chats that are not open (bounded).
pub const MAX_DRAFTS: usize = 64;

/// Editable text with a cursor.
#[derive(Debug, Default, Clone)]
pub struct Composer {
    content: Content,
}

impl Composer {
    /// An empty box.
    pub fn new() -> Self {
        Self::default()
    }

    /// A buffer holding `text`, cursor at the end. The text goes in as a
    /// paste so the editor lays it out like typed input.
    pub fn with_text(text: &str) -> Self {
        let mut c = Self::new();
        if !text.is_empty() {
            c.content
                .perform(Action::Edit(Edit::Paste(Arc::new(text.to_owned()))));
        }
        c
    }

    /// The editor content (for the view).
    pub fn content(&self) -> &Content {
        &self.content
    }

    /// The whole text, newlines included.
    pub fn text(&self) -> String {
        self.content.text()
    }

    /// True when there is nothing but whitespace.
    pub fn is_blank(&self) -> bool {
        self.text().trim().is_empty()
    }

    /// Empties the box.
    pub fn clear(&mut self) {
        self.content = Content::new();
    }

    /// Replaces the text (edit mode, drafts).
    pub fn set_text(&mut self, text: &str) {
        *self = Self::with_text(text);
    }

    /// Applies an editor action; true when it changed the text.
    pub fn perform(&mut self, action: Action) -> bool {
        let edit = action.is_edit();
        self.content.perform(action);
        edit
    }

    /// Inserts `text` at the cursor (emoji picker).
    pub fn insert(&mut self, text: &str) {
        self.content
            .perform(Action::Edit(Edit::Paste(Arc::new(text.to_owned()))));
    }

    /// Moves the cursor to the end of the text.
    pub fn move_to_end(&mut self) {
        let last = self.content.line_count().saturating_sub(1);
        // Columns are byte offsets into the line (a char count panics in
        // cosmic-text on non-ASCII lines).
        let column = self.content.line(last).map_or(0, |l| l.text.len());
        self.content.move_to(Cursor {
            position: Position { line: last, column },
            selection: None,
        });
    }
}

/// Key bindings of the composer: a plain Enter produces `submit`,
/// Shift+Enter breaks the line, everything else is the editor default.
pub fn key_binding<M: Clone>(kp: KeyPress, submit: M) -> Option<Binding<M>> {
    if !matches!(kp.status, Status::Focused { .. }) {
        return None;
    }
    if let Key::Named(Named::Enter) = kp.modified_key.as_ref() {
        return Some(if kp.modifiers.shift() {
            Binding::Enter
        } else {
            Binding::Custom(submit)
        });
    }
    Binding::from_key_press(kp)
}

/// Lines the box grows to before it scrolls (WhatsApp Desktop shows about
/// six lines).
pub const MAX_LINES: f32 = 6.0;
/// Height of one line of composer text.
pub const LINE_HEIGHT: f32 = 22.0;
/// Space above and below the text inside the box.
pub const PADDING_Y: f32 = 9.0;

/// Limits for the text area *inside* the padding. iced's `text_editor`
/// applies `min_height`/`max_height` to the text and then adds the
/// padding: a total height here would grow the box by the padding twice.
/// The box never takes more than a third of the window.
pub fn text_height_limits(window_height: f32) -> (f32, f32) {
    let by_window = (window_height / 3.0 - 2.0 * PADDING_Y).max(LINE_HEIGHT);
    (LINE_HEIGHT, (LINE_HEIGHT * MAX_LINES).min(by_window))
}

/// Drafts of the chats that are not open.
#[derive(Debug, Default, Clone)]
pub struct Drafts(HashMap<String, String>);

impl Drafts {
    /// Keeps `text` for `chat` (blank text forgets it).
    pub fn save(&mut self, chat: &str, text: String) {
        if text.trim().is_empty() {
            self.0.remove(chat);
            return;
        }
        if self.0.len() >= MAX_DRAFTS
            && !self.0.contains_key(chat)
            && let Some(k) = self.0.keys().next().cloned()
        {
            self.0.remove(&k);
        }
        self.0.insert(chat.to_string(), text);
    }

    /// Takes the draft of `chat`.
    pub fn take(&mut self, chat: &str) -> Option<String> {
        self.0.remove(chat)
    }

    /// The draft of `chat`, if any (chat list "Draft:" preview).
    pub fn get(&self, chat: &str) -> Option<&str> {
        self.0.get(chat).map(String::as_str)
    }

    /// Number of drafts.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Modifiers, key::Physical};

    fn press(key: Named, modifiers: Modifiers) -> KeyPress {
        KeyPress {
            key: Key::Named(key),
            modified_key: Key::Named(key),
            physical_key: Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        }
    }

    #[test]
    fn enter_sends_shift_enter_breaks_the_line() {
        let b = key_binding(press(Named::Enter, Modifiers::empty()), 1u8);
        assert!(matches!(b, Some(Binding::Custom(1))));
        let b = key_binding(press(Named::Enter, Modifiers::SHIFT), 1u8);
        assert!(matches!(b, Some(Binding::Enter)));
    }

    #[test]
    fn unfocused_editor_ignores_keys() {
        let mut k = press(Named::Enter, Modifiers::empty());
        k.status = Status::Active;
        assert!(key_binding(k, 1u8).is_none());
    }

    #[test]
    fn shift_enter_action_puts_a_newline_in_the_text() {
        let mut c = Composer::new();
        c.perform(Action::Edit(Edit::Insert('a')));
        c.perform(Action::Edit(Edit::Enter));
        c.perform(Action::Edit(Edit::Insert('b')));
        assert_eq!(c.text(), "a\nb");
        assert!(!c.is_blank());
        c.clear();
        assert!(c.is_blank());
        assert_eq!(c.text(), "");
    }

    #[test]
    fn buffer_keeps_newlines_and_inserts_at_the_end() {
        let mut c = Composer::with_text("a\nb");
        assert_eq!(c.text(), "a\nb");
        c.move_to_end();
        c.insert("\u{1F355} ");
        assert_eq!(c.text(), "a\nb\u{1F355} ");
        // Non-ASCII lines: cursor columns are byte offsets.
        let mut u = Composer::with_text("\u{E9}\n\u{FC}\u{1F355}");
        u.move_to_end();
        u.insert("\u{E9}");
        assert_eq!(u.text(), "\u{E9}\n\u{FC}\u{1F355}\u{E9}");
        assert_eq!(u.clone().text(), u.text());
    }

    #[test]
    fn height_limits_exclude_the_padding() {
        let (min, max) = text_height_limits(800.0);
        assert_eq!(min, LINE_HEIGHT);
        assert_eq!(max, LINE_HEIGHT * MAX_LINES);
        // A short window caps the box at a third of it, padding included.
        let (_, max) = text_height_limits(300.0);
        assert!(max + 2.0 * PADDING_Y <= 100.0 + 0.01, "{max}");
        assert!(text_height_limits(10.0).1 >= LINE_HEIGHT);
    }

    #[test]
    fn drafts_are_per_chat_and_bounded() {
        let mut d = Drafts::default();
        for i in 0..100 {
            d.save(&format!("{i}@s.whatsapp.net"), format!("t{i}"));
        }
        assert!(d.len() <= MAX_DRAFTS);
        d.save("a@s.whatsapp.net", "keep\nme".into());
        assert_eq!(d.get("a@s.whatsapp.net"), Some("keep\nme"));
        assert_eq!(d.take("a@s.whatsapp.net").as_deref(), Some("keep\nme"));
        d.save("b@s.whatsapp.net", "  ".into());
        assert!(d.take("b@s.whatsapp.net").is_none());
    }
}
