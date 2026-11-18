//! Domain types shared by `net`, `store` and `app`.
//!
//! These are deliberately independent of the protocol library: the
//! conversion from `whatsapp-rust` / protobuf types lives in
//! [`crate::net::convert`], so the rest of the app (and the fake client) only
//! ever sees these.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A chat JID as a string (`<user>@s.whatsapp.net`, `<id>@g.us`, `<id>@lid`).
pub type ChatId = String;

/// What kind of media a [`MediaRef`] points to (selects the HKDF info string
/// used to decrypt it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaKind {
    /// JPEG/PNG/WebP image.
    Image,
    /// Any file sent as a document.
    Document,
    /// Audio and voice notes (PTT).
    Audio,
}

/// Everything needed to download and decrypt a media blob from the CDN.
/// Not a credential: it is per-message content, like the message body.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaRef {
    /// CDN path.
    pub direct_path: String,
    /// Per-file AES key material.
    pub media_key: Vec<u8>,
    /// SHA-256 of the plaintext.
    pub file_sha256: Vec<u8>,
    /// SHA-256 of the ciphertext.
    pub file_enc_sha256: Vec<u8>,
    /// Plaintext size in bytes.
    pub file_length: u64,
    /// Media kind.
    pub kind: MediaKind,
    /// MIME type as announced by the sender.
    pub mimetype: String,
}

impl MediaRef {
    /// Stable cache key (hex of the plaintext hash, or the path).
    pub fn cache_key(&self) -> String {
        if self.file_sha256.is_empty() {
            self.direct_path.clone()
        } else {
            hex::encode(&self.file_sha256)
        }
    }
}

impl fmt::Debug for MediaRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaRef")
            .field("kind", &self.kind)
            .field("mimetype", &self.mimetype)
            .field("file_length", &self.file_length)
            .field("media_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// The body of a message.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum Content {
    /// Plain or extended text.
    Text(String),
    /// A photo.
    Image {
        /// Optional caption.
        caption: Option<String>,
        /// Pixel width, if announced.
        width: u32,
        /// Pixel height, if announced.
        height: u32,
        /// Inline JPEG thumbnail (stored separately in the cache).
        #[serde(skip)]
        thumbnail: Option<Vec<u8>>,
        /// Where to fetch the full image.
        media: Option<MediaRef>,
    },
    /// A file.
    Document {
        /// File name shown in the bubble.
        file_name: String,
        /// MIME type.
        mimetype: String,
        /// Size in bytes.
        size: u64,
        /// Page count for PDFs.
        pages: Option<u32>,
        /// Optional caption.
        caption: Option<String>,
        /// Where to fetch the file.
        media: Option<MediaRef>,
    },
    /// Audio; `ptt` marks a recorded voice note.
    Voice {
        /// Duration in seconds.
        seconds: u32,
        /// Push-to-talk (voice note) vs. audio file.
        ptt: bool,
        /// 64 amplitude samples (0..=100) for the waveform.
        waveform: Vec<u8>,
        /// Where to fetch the audio.
        media: Option<MediaRef>,
    },
    /// Something we do not render yet (sticker, video, poll, ...).
    Unsupported(String),
    /// "This message was deleted".
    Deleted,
}

impl Content {
    /// One-line preview for the chat list and notifications.
    pub fn preview(&self) -> String {
        match self {
            Content::Text(t) => t.lines().next().unwrap_or_default().to_string(),
            Content::Image { caption, .. } => match caption {
                Some(c) if !c.is_empty() => format!("📷 {c}"),
                _ => "📷 Photo".into(),
            },
            Content::Document { file_name, .. } => format!("📄 {file_name}"),
            Content::Voice { seconds, ptt, .. } => {
                let label = if *ptt { "🎤" } else { "🎵" };
                format!("{label} {}", format_duration(*seconds))
            }
            Content::Unsupported(kind) => format!("Unsupported message: {kind}"),
            Content::Deleted => "🚫 This message was deleted".into(),
        }
    }

    /// The downloadable media of this content, if any.
    pub fn media(&self) -> Option<&MediaRef> {
        match self {
            Content::Image { media, .. }
            | Content::Document { media, .. }
            | Content::Voice { media, .. } => media.as_ref(),
            _ => None,
        }
    }

    /// Short type tag stored in the cache.
    pub fn kind_tag(&self) -> &'static str {
        match self {
            Content::Text(_) => "text",
            Content::Image { .. } => "image",
            Content::Document { .. } => "document",
            Content::Voice { .. } => "voice",
            Content::Unsupported(_) => "unsupported",
            Content::Deleted => "deleted",
        }
    }
}

impl fmt::Debug for Content {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print message bodies or file names.
        match self {
            Content::Text(t) => write!(f, "Text(len={})", t.chars().count()),
            Content::Image { width, height, .. } => write!(f, "Image({width}x{height})"),
            Content::Document { size, .. } => write!(f, "Document(size={size})"),
            Content::Voice { seconds, ptt, .. } => write!(f, "Voice({seconds}s, ptt={ptt})"),
            Content::Unsupported(k) => write!(f, "Unsupported({k})"),
            Content::Deleted => write!(f, "Deleted"),
        }
    }
}

/// `m:ss` like the official client.
pub fn format_duration(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Delivery state of an outgoing message (the tick marks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum ReceiptStatus {
    /// Queued locally, clock icon.
    Pending,
    /// Sending failed, red alert icon.
    Failed,
    /// Server accepted, one grey tick.
    Sent,
    /// Delivered to a device of the recipient, two grey ticks.
    Delivered,
    /// Read, two blue ticks.
    Read,
    /// Voice note played, two blue ticks (blue mic in the bubble).
    Played,
}

impl ReceiptStatus {
    fn rank(self) -> u8 {
        match self {
            ReceiptStatus::Pending => 0,
            ReceiptStatus::Failed => 0,
            ReceiptStatus::Sent => 1,
            ReceiptStatus::Delivered => 2,
            ReceiptStatus::Read => 3,
            ReceiptStatus::Played => 4,
        }
    }

    /// Receipt state machine. Receipts can arrive out of order (a `read` can
    /// overtake a `delivered`), so the state only ever moves forward:
    /// `Pending → Sent → Delivered → Read → Played`. `Failed` is only reachable
    /// from `Pending`, and any server confirmation overrides `Failed` (a retry
    /// succeeded).
    pub fn advance(self, incoming: ReceiptStatus) -> ReceiptStatus {
        match (self, incoming) {
            (ReceiptStatus::Pending, ReceiptStatus::Failed) => ReceiptStatus::Failed,
            (s, ReceiptStatus::Failed) => s,
            (ReceiptStatus::Failed, ReceiptStatus::Pending) => ReceiptStatus::Failed,
            (cur, new) if new.rank() > cur.rank() => new,
            (cur, _) => cur,
        }
    }

    /// Integer stored in SQLite.
    pub fn to_i64(self) -> i64 {
        match self {
            ReceiptStatus::Pending => 0,
            ReceiptStatus::Failed => -1,
            ReceiptStatus::Sent => 1,
            ReceiptStatus::Delivered => 2,
            ReceiptStatus::Read => 3,
            ReceiptStatus::Played => 4,
        }
    }

    /// Inverse of [`Self::to_i64`]; unknown values map to `Sent`.
    pub fn from_i64(v: i64) -> Self {
        match v {
            0 => ReceiptStatus::Pending,
            -1 => ReceiptStatus::Failed,
            2 => ReceiptStatus::Delivered,
            3 => ReceiptStatus::Read,
            4 => ReceiptStatus::Played,
            _ => ReceiptStatus::Sent,
        }
    }
}

/// A message in a chat.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Message id (unique per chat).
    pub id: String,
    /// Chat JID.
    pub chat: ChatId,
    /// Sender JID (own JID for outgoing messages).
    pub sender: String,
    /// Push name of the sender, if the server sent one.
    pub sender_name: Option<String>,
    /// Outgoing?
    pub from_me: bool,
    /// Unix seconds.
    pub timestamp: i64,
    /// Body.
    pub content: Content,
    /// Delivery state (only meaningful when `from_me`).
    pub status: ReceiptStatus,
}

/// Chat metadata (from history sync or derived from messages).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatMeta {
    /// JID.
    pub id: ChatId,
    /// Display name (contact or group subject).
    pub name: Option<String>,
    /// Unread messages count.
    pub unread: u32,
    /// Unix seconds of the last activity.
    pub last_ts: i64,
    /// Pinned to the top.
    pub pinned: bool,
    /// Archived.
    pub archived: bool,
    /// Muted.
    pub muted: bool,
}

/// True for group JIDs.
pub fn is_group_jid(jid: &str) -> bool {
    jid.ends_with("@g.us")
}

/// Fallback display name for a JID: `+<number>` for phone JIDs.
pub fn fallback_name(jid: &str) -> String {
    let user = jid.split('@').next().unwrap_or(jid);
    let user = user.split([':', '.']).next().unwrap_or(user);
    if jid.ends_with("@s.whatsapp.net") && user.chars().all(|c| c.is_ascii_digit()) {
        format!("+{user}")
    } else if is_group_jid(jid) {
        "Group".into()
    } else {
        user.to_string()
    }
}

/// Media the user wants to send.
#[derive(Clone)]
pub struct OutgoingMedia {
    /// What to send it as.
    pub kind: OutgoingKind,
    /// Plaintext bytes.
    pub bytes: Vec<u8>,
    /// File name (documents).
    pub file_name: String,
    /// MIME type.
    pub mimetype: String,
    /// Optional caption.
    pub caption: Option<String>,
}

impl fmt::Debug for OutgoingMedia {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingMedia")
            .field("kind", &self.kind)
            .field("len", &self.bytes.len())
            .field("mimetype", &self.mimetype)
            .finish_non_exhaustive()
    }
}

/// Kind-specific data of an [`OutgoingMedia`].
#[derive(Debug, Clone, PartialEq)]
pub enum OutgoingKind {
    /// Photo with dimensions and a JPEG thumbnail.
    Image {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// Small JPEG thumbnail.
        thumbnail: Vec<u8>,
    },
    /// Any file.
    Document,
    /// Voice note (Ogg/Opus).
    Voice {
        /// Duration in seconds.
        seconds: u32,
        /// 64 samples, 0..=100.
        waveform: Vec<u8>,
    },
}

impl OutgoingMedia {
    /// The [`Content`] shown locally for this media before/after upload.
    pub fn local_content(&self, media: Option<MediaRef>) -> Content {
        match &self.kind {
            OutgoingKind::Image {
                width,
                height,
                thumbnail,
            } => Content::Image {
                caption: self.caption.clone(),
                width: *width,
                height: *height,
                thumbnail: Some(thumbnail.clone()),
                media,
            },
            OutgoingKind::Document => Content::Document {
                file_name: self.file_name.clone(),
                mimetype: self.mimetype.clone(),
                size: self.bytes.len() as u64,
                pages: None,
                caption: self.caption.clone(),
                media,
            },
            OutgoingKind::Voice { seconds, waveform } => Content::Voice {
                seconds: *seconds,
                ptt: true,
                waveform: waveform.clone(),
                media,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ReceiptStatus::*;

    #[test]
    fn receipts_only_move_forward() {
        assert_eq!(Pending.advance(Sent), Sent);
        assert_eq!(Sent.advance(Delivered), Delivered);
        assert_eq!(Delivered.advance(Read), Read);
        // Out-of-order: read arrives before delivered.
        assert_eq!(Sent.advance(Read).advance(Delivered), Read);
        assert_eq!(Read.advance(Sent), Read);
        assert_eq!(Read.advance(Played), Played);
        assert_eq!(Played.advance(Read), Played);
    }

    #[test]
    fn failure_rules() {
        assert_eq!(Pending.advance(Failed), Failed);
        assert_eq!(Sent.advance(Failed), Sent);
        assert_eq!(Delivered.advance(Failed), Delivered);
        assert_eq!(Failed.advance(Pending), Failed);
        assert_eq!(Failed.advance(Sent), Sent);
        assert_eq!(Failed.advance(Read), Read);
    }

    #[test]
    fn status_roundtrips_through_i64() {
        for s in [Pending, Failed, Sent, Delivered, Read, Played] {
            assert_eq!(ReceiptStatus::from_i64(s.to_i64()), s);
        }
    }

    #[test]
    fn previews() {
        assert_eq!(Content::Text("hi\nthere".into()).preview(), "hi");
        assert_eq!(
            Content::Voice {
                seconds: 75,
                ptt: true,
                waveform: vec![],
                media: None
            }
            .preview(),
            "🎤 1:15"
        );
        assert!(Content::Deleted.preview().contains("deleted"));
    }

    #[test]
    fn debug_never_prints_bodies() {
        let c = Content::Text("my secret body".into());
        assert!(!format!("{c:?}").contains("secret"));
        let d = Content::Document {
            file_name: "tax-return.pdf".into(),
            mimetype: "application/pdf".into(),
            size: 10,
            pages: None,
            caption: None,
            media: None,
        };
        assert!(!format!("{d:?}").contains("tax"));
    }

    #[test]
    fn fallback_names() {
        assert_eq!(fallback_name("34600111222@s.whatsapp.net"), "+34600111222");
        assert_eq!(fallback_name("1203630@g.us"), "Group");
        assert!(is_group_jid("1203630@g.us"));
    }
}
