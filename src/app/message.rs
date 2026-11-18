//! The `Message` enum: UI events, protocol events and task completions.

use std::path::PathBuf;
use std::sync::Arc;

use crate::app::state::Filter;
use crate::media::audio::Pcm;
use crate::net::{self, ChatId, Content, Message as ChatMessage, OutgoingMedia};

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
    /// Window focus changed.
    Focus(bool),
    /// Screenshot mode: capture now.
    CaptureScreenshot,
    /// First frame has been presented (startup trace).
    FirstFrame,
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
}

/// Task completions.
#[derive(Debug, Clone)]
pub enum Done {
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
        result: Result<Arc<Vec<u8>>, String>,
    },
    /// Voice note downloaded and decoded.
    VoiceReady {
        /// Message id.
        id: String,
        /// PCM or error.
        result: Result<Arc<Pcm>, String>,
    },
    /// File picked and turned into outgoing media (`None` = cancelled).
    Prepared(Option<Result<Box<OutgoingMedia>, String>>),
    /// Document saved to disk and opened.
    DocumentOpened(Result<PathBuf, String>),
    /// Background store write finished.
    Stored(Result<(), String>),
    /// Logout / unlink cleanup finished.
    CleanedUp,
    /// Screenshot captured.
    Screenshot(Box<iced::window::Screenshot>),
    /// Screenshot written.
    ScreenshotSaved(Result<PathBuf, String>),
    /// Nothing to do.
    Nothing,
}
