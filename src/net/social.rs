//! Domain types for statuses, communities, contacts and starred messages.
//!
//! Like [`super::types`] these know nothing about the protocol library; the
//! conversions from protobuf live next to them as pure functions so they can
//! be unit tested with hand-built messages.

use std::fmt;

use serde::{Deserialize, Serialize};
use whatsapp_rust::waproto::whatsapp as wa;

use super::convert::bare_jid;
use super::types::{MediaKind, MediaRef, MsgKey};

/// The pseudo chat statuses travel in. It is never a conversation.
pub const STATUS_JID: &str = "status@broadcast";
/// Statuses disappear 24 hours after they were posted.
pub const STATUS_TTL_SECS: i64 = 24 * 60 * 60;
/// Longest status text WhatsApp accepts when posting.
pub const MAX_STATUS_TEXT: usize = 700;
/// Largest inline status thumbnail kept (they are a few kB in practice).
pub const MAX_STATUS_THUMB: usize = 64 * 1024;
/// How long an image or text status stays on screen.
pub const STATUS_IMAGE_MS: u64 = 5_000;
/// Longest a video status plays before the viewer moves on (WhatsApp caps
/// videos at 30 s; a longer one is a protocol oddity).
pub const MAX_STATUS_VIDEO_MS: u64 = 90_000;

/// True for the status pseudo chat.
pub fn is_status_jid(jid: &str) -> bool {
    jid == STATUS_JID
}

/// What a status shows.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub enum StatusBody {
    /// Text on a coloured background.
    Text {
        /// The text.
        text: String,
        /// Background colour as `0xAARRGGBB`.
        background: u32,
        /// Font id (WhatsApp's `FontType`).
        font: i32,
    },
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
    /// A video.
    Video {
        /// Optional caption.
        caption: Option<String>,
        /// Duration in seconds.
        seconds: u32,
        /// Pixel width, if announced.
        width: u32,
        /// Pixel height, if announced.
        height: u32,
        /// Inline JPEG thumbnail (stored separately in the cache).
        #[serde(skip)]
        thumbnail: Option<Vec<u8>>,
        /// Where to fetch the video.
        media: Option<MediaRef>,
    },
}

impl StatusBody {
    /// Short tag stored in the cache.
    pub fn kind_tag(&self) -> &'static str {
        match self {
            StatusBody::Text { .. } => "text",
            StatusBody::Image { .. } => "image",
            StatusBody::Video { .. } => "video",
        }
    }

    /// Caption (image / video) or the text itself.
    pub fn caption(&self) -> Option<&str> {
        match self {
            StatusBody::Text { .. } => None,
            StatusBody::Image { caption, .. } | StatusBody::Video { caption, .. } => {
                caption.as_deref().filter(|c| !c.is_empty())
            }
        }
    }

    /// The downloadable media, if any.
    pub fn media(&self) -> Option<&MediaRef> {
        match self {
            StatusBody::Text { .. } => None,
            StatusBody::Image { media, .. } | StatusBody::Video { media, .. } => media.as_ref(),
        }
    }

    /// The inline thumbnail, if any.
    pub fn thumbnail(&self) -> Option<&[u8]> {
        match self {
            StatusBody::Text { .. } => None,
            StatusBody::Image { thumbnail, .. } | StatusBody::Video { thumbnail, .. } => {
                thumbnail.as_deref()
            }
        }
    }

    /// One-line description for lists.
    pub fn preview(&self) -> String {
        match self {
            StatusBody::Text { text, .. } => text.lines().next().unwrap_or_default().to_string(),
            StatusBody::Image { caption, .. } => match caption.as_deref() {
                Some(c) if !c.is_empty() => format!("📷 {c}"),
                _ => "📷 Photo".into(),
            },
            StatusBody::Video { caption, .. } => match caption.as_deref() {
                Some(c) if !c.is_empty() => format!("🎥 {c}"),
                _ => "🎥 Video".into(),
            },
        }
    }
}

impl fmt::Debug for StatusBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print text or file names.
        match self {
            StatusBody::Text { text, .. } => write!(f, "Text(len={})", text.chars().count()),
            StatusBody::Image { width, height, .. } => write!(f, "Image({width}x{height})"),
            StatusBody::Video { seconds, .. } => write!(f, "Video({seconds}s)"),
        }
    }
}

/// One status update.
#[derive(Clone, PartialEq)]
pub struct StatusItem {
    /// Message id (unique per author).
    pub id: String,
    /// Author (bare JID).
    pub author: String,
    /// Push name carried by the message.
    pub author_name: Option<String>,
    /// Posted by us (from the phone or another device).
    pub from_me: bool,
    /// Unix seconds.
    pub timestamp: i64,
    /// Content.
    pub body: StatusBody,
}

impl fmt::Debug for StatusItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatusItem")
            .field("id", &self.id)
            .field(
                "author",
                &format_args!("{}", crate::error::MaskedJid(&self.author)),
            )
            .field("from_me", &self.from_me)
            .field("timestamp", &self.timestamp)
            .field("body", &self.body)
            .finish()
    }
}

impl StatusItem {
    /// Unix seconds after which the status is gone.
    pub fn expires_at(&self) -> i64 {
        self.timestamp + STATUS_TTL_SECS
    }

    /// Still inside its 24 hours.
    pub fn is_live(&self, now: i64) -> bool {
        self.expires_at() > now
    }

    /// How long the viewer shows it before moving to the next one.
    pub fn duration_ms(&self) -> u64 {
        match &self.body {
            StatusBody::Image { .. } => STATUS_IMAGE_MS,
            StatusBody::Text { text, .. } => {
                // A little longer for long texts, like the official client.
                let extra = (text.chars().count() as u64 / 40).min(10) * 1_000;
                STATUS_IMAGE_MS + extra
            }
            StatusBody::Video { seconds, .. } => {
                let ms = u64::from(*seconds) * 1_000;
                if ms == 0 {
                    STATUS_IMAGE_MS
                } else {
                    ms.min(MAX_STATUS_VIDEO_MS)
                }
            }
        }
    }
}

/// Converts a decrypted status message. `None` for anything that is not a
/// text / photo / video status (reactions, protocol chatter, ...).
pub fn status_from_wa(
    id: &str,
    author: &str,
    push_name: &str,
    from_me: bool,
    timestamp: i64,
    msg: &wa::Message,
) -> Option<StatusItem> {
    let body = status_body(msg)?;
    Some(StatusItem {
        id: id.to_string(),
        author: bare_jid(author),
        author_name: Some(push_name.to_string()).filter(|n| !n.is_empty()),
        from_me,
        timestamp,
        body,
    })
}

fn non_empty(s: Option<&String>) -> Option<String> {
    s.filter(|s| !s.is_empty()).cloned()
}

fn thumb(t: Option<&Vec<u8>>) -> Option<Vec<u8>> {
    t.filter(|t| !t.is_empty() && t.len() <= MAX_STATUS_THUMB)
        .cloned()
}

fn media(
    kind: MediaKind,
    path: Option<&String>,
    key: Option<&Vec<u8>>,
    sha: Option<&Vec<u8>>,
    enc_sha: Option<&Vec<u8>>,
    len: Option<u64>,
    mime: Option<&String>,
) -> Option<MediaRef> {
    Some(MediaRef {
        direct_path: path.filter(|p| !p.is_empty())?.clone(),
        media_key: key.filter(|k| !k.is_empty())?.clone(),
        file_sha256: sha.cloned().unwrap_or_default(),
        file_enc_sha256: enc_sha.cloned().unwrap_or_default(),
        file_length: len.unwrap_or_default(),
        kind,
        mimetype: mime.cloned().unwrap_or_default(),
    })
}

/// The body of a status message.
pub fn status_body(msg: &wa::Message) -> Option<StatusBody> {
    let m = super::convert::base_message(msg);
    if let Some(ext) = m.extended_text_message.as_option()
        && let Some(text) = non_empty(ext.text.as_ref())
    {
        return Some(StatusBody::Text {
            text,
            // Opaque default (WhatsApp green) when the sender sent none.
            background: ext.background_argb.unwrap_or(0xFF1E_6E4F) | 0xFF00_0000,
            font: ext.font.map(|f| f as i32).unwrap_or(0),
        });
    }
    if let Some(text) = non_empty(m.conversation.as_ref()) {
        return Some(StatusBody::Text {
            text,
            background: 0xFF1E_6E4F,
            font: 0,
        });
    }
    if let Some(img) = m.image_message.as_option() {
        return Some(StatusBody::Image {
            caption: non_empty(img.caption.as_ref()),
            width: img.width.unwrap_or_default(),
            height: img.height.unwrap_or_default(),
            thumbnail: thumb(img.jpeg_thumbnail.as_ref()),
            media: media(
                MediaKind::Image,
                img.direct_path.as_ref(),
                img.media_key.as_ref(),
                img.file_sha256.as_ref(),
                img.file_enc_sha256.as_ref(),
                img.file_length,
                img.mimetype.as_ref(),
            ),
        });
    }
    if let Some(v) = m.video_message.as_option() {
        return Some(StatusBody::Video {
            caption: non_empty(v.caption.as_ref()),
            seconds: v.seconds.unwrap_or_default(),
            width: v.width.unwrap_or_default(),
            height: v.height.unwrap_or_default(),
            thumbnail: thumb(v.jpeg_thumbnail.as_ref()),
            media: media(
                MediaKind::Video,
                v.direct_path.as_ref(),
                v.media_key.as_ref(),
                v.file_sha256.as_ref(),
                v.file_enc_sha256.as_ref(),
                v.file_length,
                v.mimetype.as_ref(),
            ),
        });
    }
    None
}

/// The id of the status a protocol message revokes.
pub fn status_revoked_target(msg: &wa::Message) -> Option<String> {
    super::convert::revoked_target(msg)
}

/// One row of the "Recent updates" / "Viewed updates" lists: an author and
/// their live statuses, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusGroup {
    /// Author (bare JID).
    pub author: String,
    /// Live statuses, oldest first.
    pub items: Vec<StatusItem>,
    /// How many of `items` the user has already seen.
    pub viewed: usize,
}

impl StatusGroup {
    /// Every status was seen.
    pub fn all_viewed(&self) -> bool {
        self.viewed >= self.items.len()
    }

    /// Time of the newest status.
    pub fn newest(&self) -> i64 {
        self.items.last().map(|i| i.timestamp).unwrap_or_default()
    }

    /// Index of the first unseen status (or 0 when all were seen, so a
    /// replay starts at the beginning like the official client).
    pub fn start_index(&self) -> usize {
        if self.all_viewed() { 0 } else { self.viewed }
    }
}

/// A subgroup of a community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubGroup {
    /// Group JID.
    pub jid: String,
    /// Subject.
    pub name: String,
    /// The community's announcement group.
    pub announcement: bool,
    /// The "General" chat created with the community.
    pub general: bool,
    /// We are a member (so the chat can be opened).
    pub joined: bool,
    /// Participant count, if known.
    pub members: Option<u32>,
}

/// A community: a parent group and its subgroups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunityInfo {
    /// Parent group JID.
    pub jid: String,
    /// Name.
    pub name: String,
    /// Description, if any.
    pub description: Option<String>,
    /// Participant count, if known.
    pub members: Option<u32>,
    /// Announcement group first, then the rest by name.
    pub subgroups: Vec<SubGroup>,
}

impl CommunityInfo {
    /// Orders the subgroups: announcement group, general chat, then by name.
    pub fn sort(&mut self) {
        self.subgroups.sort_by(|a, b| {
            b.announcement
                .cmp(&a.announcement)
                .then(b.general.cmp(&a.general))
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
}

/// Answer of the "is this number on WhatsApp" check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneLookup {
    /// Registered: chat with this JID.
    OnWhatsApp {
        /// Phone JID to chat with.
        jid: String,
        /// A business account.
        business: bool,
    },
    /// Valid number, no WhatsApp account.
    NotOnWhatsApp,
    /// Not a phone number.
    Invalid,
}

/// Digits of a typed phone number, `None` when it cannot be one (E.164 has
/// 8 to 15 digits). Accepts `+`, spaces, dashes, dots and parentheses.
pub fn parse_phone(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    let mut digits = String::new();
    for (i, c) in t.chars().enumerate() {
        match c {
            '0'..='9' => digits.push(c),
            '+' if i == 0 => {}
            ' ' | '-' | '.' | '(' | ')' | '\u{a0}' => {}
            _ => return None,
        }
    }
    // International "00" dialling prefix.
    let digits = digits
        .strip_prefix("00")
        .map(str::to_string)
        .unwrap_or(digits);
    (8..=15).contains(&digits.len()).then_some(digits)
}

/// Phone JID of typed digits.
pub fn phone_jid(digits: &str) -> String {
    format!("{digits}@s.whatsapp.net")
}

/// A starred message, for the "Starred messages" list.
#[derive(Debug, Clone, PartialEq)]
pub struct StarredMessage {
    /// The message.
    pub message: super::types::Message,
    /// When it was starred (unix seconds).
    pub starred_at: i64,
}

/// A star change (ours or from another device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarChange {
    /// The message.
    pub key: MsgKey,
    /// Now starred.
    pub starred: bool,
    /// Unix seconds of the change.
    pub ts: i64,
}

/// Longest group subject the protocol accepts.
pub const MAX_GROUP_SUBJECT: usize = 100;
/// Most people one group can be created with from the picker (WhatsApp
/// allows 1024 members; creating with a huge list in one go is not a flow
/// the official client offers).
pub const MAX_NEW_GROUP_PICK: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;
    use whatsapp_rust::waproto::buffa::MessageField;

    fn text_status(text: &str, bg: Option<u32>) -> wa::Message {
        wa::Message {
            extended_text_message: MessageField::some(wa::message::ExtendedTextMessage {
                text: Some(text.into()),
                background_argb: bg,
                font: Some(wa::message::extended_text_message::FontType::SYSTEM_BOLD),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn text_status_keeps_colour_and_font() {
        let it = status_from_wa(
            "A1",
            "34600111222:7@s.whatsapp.net",
            "Lu",
            false,
            100,
            &text_status("hola", Some(0x001E_6E4F)),
        )
        .expect("status");
        assert_eq!(it.author, "34600111222@s.whatsapp.net");
        assert_eq!(it.author_name.as_deref(), Some("Lu"));
        match it.body {
            StatusBody::Text {
                text,
                background,
                font,
            } => {
                assert_eq!(text, "hola");
                // The alpha channel is forced opaque.
                assert_eq!(background, 0xFF1E_6E4F);
                assert_eq!(font, 6);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn image_status_needs_nothing_but_keeps_the_media_when_complete() {
        let msg = wa::Message {
            image_message: MessageField::some(wa::message::ImageMessage {
                caption: Some("sunset".into()),
                direct_path: Some("/v/p".into()),
                media_key: Some(vec![1; 32]),
                file_sha256: Some(vec![2; 32]),
                file_enc_sha256: Some(vec![3; 32]),
                file_length: Some(10),
                mimetype: Some("image/jpeg".into()),
                jpeg_thumbnail: Some(vec![9; 10]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let it = status_from_wa("A2", "x@s.whatsapp.net", "", false, 1, &msg).expect("status");
        assert_eq!(it.author_name, None);
        assert_eq!(it.body.caption(), Some("sunset"));
        assert!(it.body.media().is_some());
        assert_eq!(it.body.thumbnail().map(<[u8]>::len), Some(10));
        assert_eq!(it.duration_ms(), STATUS_IMAGE_MS);
    }

    #[test]
    fn video_status_lasts_as_long_as_the_video() {
        let msg = wa::Message {
            video_message: MessageField::some(wa::message::VideoMessage {
                seconds: Some(12),
                ..Default::default()
            }),
            ..Default::default()
        };
        let it = status_from_wa("A3", "x@s.whatsapp.net", "", false, 1, &msg).expect("status");
        assert_eq!(it.duration_ms(), 12_000);
        assert!(it.body.media().is_none(), "no CDN path: not downloadable");
    }

    #[test]
    fn hostile_status_values_are_bounded() {
        let msg = wa::Message {
            video_message: MessageField::some(wa::message::VideoMessage {
                seconds: Some(u32::MAX),
                jpeg_thumbnail: Some(vec![0; MAX_STATUS_THUMB + 1]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let it = status_from_wa("A4", "x@s.whatsapp.net", "", false, 1, &msg).expect("status");
        assert_eq!(it.duration_ms(), MAX_STATUS_VIDEO_MS);
        assert!(it.body.thumbnail().is_none(), "oversized thumbnail dropped");
    }

    #[test]
    fn protocol_chatter_is_not_a_status() {
        assert!(
            status_from_wa(
                "A5",
                "x@s.whatsapp.net",
                "",
                false,
                1,
                &wa::Message::default()
            )
            .is_none()
        );
    }

    #[test]
    fn statuses_live_exactly_24_hours() {
        let it = status_from_wa(
            "A",
            "x@s.whatsapp.net",
            "",
            false,
            1_000,
            &text_status("t", None),
        )
        .expect("status");
        assert!(it.is_live(1_000 + STATUS_TTL_SECS - 1));
        assert!(!it.is_live(1_000 + STATUS_TTL_SECS));
    }

    #[test]
    fn long_text_stays_longer_but_not_forever() {
        let short = StatusBody::Text {
            text: "hi".into(),
            background: 0,
            font: 0,
        };
        let long = StatusBody::Text {
            text: "x".repeat(10_000),
            background: 0,
            font: 0,
        };
        let mk = |body| StatusItem {
            id: "i".into(),
            author: "a".into(),
            author_name: None,
            from_me: false,
            timestamp: 0,
            body,
        };
        assert_eq!(mk(short).duration_ms(), STATUS_IMAGE_MS);
        assert_eq!(mk(long).duration_ms(), STATUS_IMAGE_MS + 10_000);
    }

    #[test]
    fn phone_numbers_are_parsed_leniently_but_not_blindly() {
        assert_eq!(
            parse_phone("+34 600 111 222").as_deref(),
            Some("34600111222")
        );
        assert_eq!(
            parse_phone("0034-600.111.222").as_deref(),
            Some("34600111222")
        );
        assert_eq!(
            parse_phone("(1) 555 0100 123").as_deref(),
            Some("15550100123")
        );
        assert_eq!(parse_phone(""), None);
        assert_eq!(parse_phone("12345"), None, "too short");
        assert_eq!(parse_phone("1234567890123456"), None, "too long");
        assert_eq!(parse_phone("600abc111"), None);
        assert_eq!(parse_phone("6+00111222"), None, "plus only leads");
    }

    #[test]
    fn community_subgroups_sort_announcement_first() {
        let sg = |name: &str, ann: bool, gen_: bool| SubGroup {
            jid: name.into(),
            name: name.into(),
            announcement: ann,
            general: gen_,
            joined: true,
            members: None,
        };
        let mut c = CommunityInfo {
            jid: "c".into(),
            name: "C".into(),
            description: None,
            members: None,
            subgroups: vec![
                sg("zeta", false, false),
                sg("General", false, true),
                sg("alpha", false, false),
                sg("Announcements", true, false),
            ],
        };
        c.sort();
        let names: Vec<_> = c.subgroups.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Announcements", "General", "alpha", "zeta"]);
    }

    #[test]
    fn status_group_replays_from_the_start_once_everything_was_seen() {
        let it = |id: &str, ts| StatusItem {
            id: id.into(),
            author: "a".into(),
            author_name: None,
            from_me: false,
            timestamp: ts,
            body: StatusBody::Text {
                text: "t".into(),
                background: 0,
                font: 0,
            },
        };
        let mut g = StatusGroup {
            author: "a".into(),
            items: vec![it("1", 1), it("2", 2), it("3", 3)],
            viewed: 1,
        };
        assert!(!g.all_viewed());
        assert_eq!(g.start_index(), 1);
        g.viewed = 3;
        assert!(g.all_viewed());
        assert_eq!(g.start_index(), 0);
        assert_eq!(g.newest(), 3);
    }

    #[test]
    fn status_debug_hides_text() {
        let it = StatusItem {
            id: "i".into(),
            author: "34600111222@s.whatsapp.net".into(),
            author_name: Some("Lu".into()),
            from_me: false,
            timestamp: 0,
            body: StatusBody::Text {
                text: "secret words".into(),
                background: 0,
                font: 0,
            },
        };
        let s = format!("{it:?}");
        assert!(!s.contains("secret") && !s.contains("600111222") && !s.contains("Lu"));
    }
}
