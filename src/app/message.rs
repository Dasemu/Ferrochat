//! The `Message` enum: UI events, protocol events and task completions.

use std::path::PathBuf;
use std::sync::Arc;

use crate::app::state::Filter;
use crate::media::audio::Pcm;
use crate::media::avatar::Tile;
use crate::net::{self, ChatId, Content, Message as ChatMessage, OutgoingMedia};
use crate::store::{SearchHit, Window};

/// Every state change goes through one of these.
#[derive(Debug, Clone)]
pub enum Message {
    /// User input.
    Ui(Ui),
    /// Pushed by the protocol client.
    Net(net::Event),
    /// A `Task` finished (boxed: some results are large).
    Done(Box<Done>),
    /// Fast tick while a voice note plays / records.
    Tick,
    /// One-second tick while typing indicators, our own composing state or
    /// the jump highlight are live.
    LiveTick,
    /// Window focus changed.
    Focus(bool),
    /// Screenshot mode: capture now.
    CaptureScreenshot,
    /// First frame has been presented (startup trace).
    FirstFrame,
    /// Tray click / menu entry, a second launch or a clicked notification.
    External(crate::ext::External),
    /// The user asked to close a window (X button, Alt+F4).
    CloseRequested(iced::window::Id),
    /// A window is gone.
    WindowClosed(iced::window::Id),
    /// A window was resized (remembered for the next open).
    WindowResized(iced::window::Id, iced::Size),
    /// A window moved (not reported on Wayland).
    WindowMoved(iced::window::Id, iced::Point),
    /// Quit the app (tray menu).
    Quit,
}

impl Message {
    /// Wraps a task result.
    pub fn done(d: Done) -> Self {
        Message::Done(Box::new(d))
    }
}

/// User input.
#[derive(Debug, Clone)]
pub enum Ui {
    /// Search box edited.
    Search(String),
    /// Filter chip clicked.
    Filter(Filter),
    /// Chat row clicked.
    OpenChat(ChatId),
    /// Escape / back.
    CloseChat,
    /// Composer edited.
    Composer(String),
    /// Enter / send button.
    Send,
    /// Paperclip.
    Attach,
    /// Emoji panel toggle.
    ToggleEmoji,
    /// Emoji picked.
    Emoji(&'static str),
    /// Mic pressed: start recording.
    StartRecording,
    /// Stop recording; send or discard.
    StopRecording {
        /// Send the note (false = discard).
        send: bool,
    },
    /// Play / resume a voice note (message id).
    PlayVoice(String),
    /// Pause playback.
    PauseVoice,
    /// Open (download + launch) a document.
    OpenDocument(String),
    /// Show an image full size.
    OpenImage(String),
    /// Close the image viewer.
    CloseViewer,
    /// Conversation scrolled; load older when near the top.
    Scrolled {
        /// Relative offset from the bottom anchor: 0 = newest, 1 = oldest.
        relative_y: f32,
        /// Absolute distance scrolled from the newest end, in pixels.
        offset_y: f32,
    },
    /// "Click to reload QR code".
    ReloadQr,
    /// Logout / unlink this device.
    Logout,
    /// Header menu toggle.
    ToggleMenu,
    /// Hide the toast.
    DismissToast,
    /// Mock mode: pretend the phone scanned the QR code.
    SimulateScan,
    /// Retry a failed message.
    Retry(String),
    /// Reconnect after "opened elsewhere" / offline.
    Reconnect,
    /// Backoff timer fired for this client generation.
    AutoReconnect {
        /// Generation the timer was armed for.
        generation: u32,
    },
    /// Discard an undecryptable session and pair again (user confirmed).
    ResetSession,
    /// The pointer entered a message row (hover actions).
    HoverMessage(String),
    /// The pointer left a message row.
    UnhoverMessage(String),
    /// Smiley button: open / close the quick-reaction picker of a message.
    ToggleReactionPicker(String),
    /// Click outside the floating reaction picker / menu.
    ClosePopup,
    /// Chevron / right click: open / close the message menu.
    ToggleMessageMenu(String),
    /// React with an emoji (same emoji again removes the reaction).
    React {
        /// Message id.
        id: String,
        /// Emoji.
        emoji: String,
    },
    /// Reply to a message: reply bar above the composer.
    StartReply(String),
    /// Close the reply bar.
    CancelReply,
    /// Scroll to (loading if needed) a message, e.g. a quoted original.
    JumpTo(String),
    /// Edit one of our messages.
    StartEdit(String),
    /// Leave edit mode.
    CancelEdit,
    /// Open the delete dialog.
    AskDelete(String),
    /// Delete, for everyone or just here.
    ConfirmDelete {
        /// Message id.
        id: String,
        /// "Delete for everyone".
        everyone: bool,
    },
    /// Close the delete dialog / reaction list.
    CloseDialog,
    /// Copy the text of a message to the clipboard.
    CopyText(String),
    /// Show who reacted to a message.
    ShowReactions(String),
    /// Download a video and open it in the system player.
    OpenVideo(String),
    /// The chat list scrolled (avatars load for what is visible).
    SidebarScrolled {
        /// Pixels from the top.
        offset_y: f32,
        /// Viewport height.
        height: f32,
    },
    /// Search icon in the conversation header.
    ToggleChatSearch,
    /// In-chat search box edited.
    ChatSearch(String),
    /// Next (older) / previous (newer) in-chat hit.
    ChatSearchStep {
        /// +1 older, -1 newer.
        delta: i32,
    },
    /// "Jump to latest" after browsing old history.
    JumpLatest,
    /// Settings page from the rail or the header menu.
    OpenSettings,
    /// Back from the settings page.
    CloseSettings,
    /// Flip a background / notification setting.
    Toggle(Setting),
    /// Open a message found by the chat list search.
    OpenHit {
        /// Chat.
        chat: ChatId,
        /// Message id.
        id: String,
    },
}

/// A boolean preference that can be flipped from the settings page (and,
/// for some, the tray menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Closing the window keeps the app in the tray.
    CloseToTray,
    /// Start without a window.
    StartMinimized,
    /// Write / remove the autostart entry.
    Autostart,
    /// No desktop notifications.
    Muted,
}

/// A downloaded blob, already prepared for display where needed.
#[derive(Clone)]
pub enum Fetched {
    /// Raw bytes (images, documents).
    Bytes(Arc<Vec<u8>>),
    /// A decoded sticker frame.
    Rgba {
        /// Width.
        width: u32,
        /// Height.
        height: u32,
        /// RGBA pixels.
        pixels: Arc<Vec<u8>>,
    },
}

impl std::fmt::Debug for Fetched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fetched::Bytes(b) => write!(f, "Bytes({})", b.len()),
            Fetched::Rgba { width, height, .. } => write!(f, "Rgba({width}x{height})"),
        }
    }
}

/// Geometry needed to scroll a message into view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    /// Top of the message relative to the top of the scrollable content.
    pub top: f32,
    /// Height of the message.
    pub height: f32,
    /// Height of the viewport.
    pub viewport: f32,
    /// Height of the whole content.
    pub content: f32,
    /// Top of the message relative to the top of the viewport right now.
    pub screen_top: f32,
}

/// Task completions.
#[derive(Debug, Clone)]
pub enum Done {
    /// The autostart entry was written (`true`) or removed (`false`).
    Autostart(Result<bool, String>),
    /// Messages loaded from the cache.
    Loaded {
        /// Chat.
        chat: ChatId,
        /// Messages, oldest first.
        msgs: Vec<ChatMessage>,
        /// Older page (prepend) vs. initial load.
        older: bool,
        /// More history in the cache.
        has_more: bool,
    },
    /// Text send finished.
    Sent {
        /// Chat.
        chat: ChatId,
        /// Message id.
        id: String,
        /// Error text on failure.
        result: Result<(), String>,
    },
    /// Media send finished.
    MediaSent {
        /// Chat.
        chat: ChatId,
        /// Message id.
        id: String,
        /// Final content.
        result: Result<Content, String>,
    },
    /// Media downloaded.
    Downloaded {
        /// Cache key.
        key: String,
        /// Bytes or error.
        result: Result<Fetched, String>,
    },
    /// A profile picture was fetched (`None` = the contact has none).
    Avatar {
        /// Contact or group.
        jid: String,
        /// Decoded tile.
        result: Result<Option<Tile>, String>,
    },
    /// A reaction we sent finished; on failure the previous one is restored.
    ReactionSent {
        /// Chat.
        chat: ChatId,
        /// Message id.
        id: String,
        /// Reaction before ours (to restore on failure).
        previous: Option<String>,
        /// Error text on failure.
        result: Result<(), String>,
    },
    /// An edit / revoke / delete-for-me request finished.
    ActionDone {
        /// What was attempted.
        what: &'static str,
        /// Error text on failure.
        result: Result<(), String>,
    },
    /// Group receipts folded into aggregate ticks.
    GroupStatus {
        /// Chat.
        chat: ChatId,
        /// Other participants in the group, if known.
        size: Option<usize>,
        /// Messages whose tick changed.
        changed: Vec<(String, net::ReceiptStatus)>,
    },
    /// Result of the chat list message search.
    ListSearch {
        /// Generation the query was issued for.
        generation: u32,
        /// Hits, newest first.
        hits: Vec<SearchHit>,
    },
    /// Result of the in-chat search.
    ChatSearch {
        /// Generation the query was issued for.
        generation: u32,
        /// Chat searched.
        chat: ChatId,
        /// Hits, newest first.
        hits: Vec<SearchHit>,
    },
    /// A window of history around a message was loaded (jump).
    Window {
        /// Chat.
        chat: ChatId,
        /// The message to bring into view.
        anchor: String,
        /// Loaded messages.
        window: Window,
    },
    /// Newer messages after a jump.
    Newer {
        /// Chat.
        chat: ChatId,
        /// Messages, oldest first.
        msgs: Vec<ChatMessage>,
        /// Still more.
        has_newer: bool,
    },
    /// Where a message sits in the conversation scrollable.
    Located(Option<Located>),
    /// Where the row of a message with an open popup is on screen.
    PopupAt {
        /// Message id the popup belongs to.
        id: String,
        /// Row geometry.
        loc: Option<Located>,
    },
    /// The newest remaining message of a chat after a local delete.
    LastMessage {
        /// Chat.
        chat: ChatId,
        /// Newest message, if any.
        last: Option<Box<ChatMessage>>,
    },
    /// A video was saved and handed to the system player.
    VideoOpened(Result<PathBuf, String>),
    /// Voice note downloaded and decoded.
    VoiceReady {
        /// Message id.
        id: String,
        /// PCM or error.
        result: Result<Arc<Pcm>, String>,
    },
    /// File picked and turned into outgoing media (`None` = cancelled).
    Prepared(Option<Result<Box<OutgoingMedia>, String>>),
    /// Document saved to disk (and opened unless it is an executable type).
    DocumentOpened(Result<(PathBuf, bool), String>),
    /// Messages persisted; `new_ids` were not in the cache before.
    MessagesStored {
        /// Messages (addressing normalised).
        messages: Vec<ChatMessage>,
        /// Ids that are new.
        new_ids: Vec<String>,
        /// Where they came from.
        origin: crate::net::Origin,
    },
    /// The audio output thread is ready (or failed); then play `then`.
    PlayerReady {
        /// Player or error.
        result: Result<crate::media::audio::Player, String>,
        /// Message id to play.
        then: String,
    },
    /// Background store write finished.
    Stored(Result<(), String>),
    /// Logout / unlink cleanup finished, with any problems encountered.
    CleanedUp(Vec<String>),
    /// Screenshot captured.
    Screenshot(Box<iced::window::Screenshot>),
    /// Screenshot written.
    ScreenshotSaved(Result<PathBuf, String>),
    /// Nothing to do.
    Nothing,
}
