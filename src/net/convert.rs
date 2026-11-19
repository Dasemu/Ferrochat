//! Conversion between `whatsapp-rust` / protobuf types and domain types.
//!
//! Everything here is pure and unit-tested; the live client in
//! [`crate::net::whatsapp`] only glues these functions to the library.

use whatsapp_rust::wacore::types::presence::ReceiptType;
use whatsapp_rust::waproto::whatsapp as wa;

use super::types::{
    ChatMeta, Content, MediaKind, MediaRef, Message, OutgoingKind, OutgoingMedia, ReceiptStatus,
};

/// Unwraps ephemeral / view-once / edited / caption wrappers (up to a few
/// levels; WhatsApp nests them).
pub fn base_message(msg: &wa::Message) -> &wa::Message {
    let mut cur = msg;
    for _ in 0..4 {
        let inner = cur
            .ephemeral_message
            .as_option()
            .or(cur.view_once_message.as_option())
            .or(cur.document_with_caption_message.as_option())
            .and_then(|f| f.message.as_option());
        match inner {
            Some(m) => cur = m,
            None => break,
        }
    }
    cur
}

fn non_empty(s: Option<&String>) -> Option<String> {
    s.filter(|s| !s.is_empty()).cloned()
}

fn media_ref(
    kind: MediaKind,
    direct_path: Option<&String>,
    media_key: Option<&Vec<u8>>,
    file_sha256: Option<&Vec<u8>>,
    file_enc_sha256: Option<&Vec<u8>>,
    file_length: Option<u64>,
    mimetype: Option<&String>,
) -> Option<MediaRef> {
    let direct_path = direct_path.filter(|p| !p.is_empty())?;
    let media_key = media_key.filter(|k| !k.is_empty())?;
    Some(MediaRef {
        direct_path: direct_path.clone(),
        media_key: media_key.clone(),
        file_sha256: file_sha256.cloned().unwrap_or_default(),
        file_enc_sha256: file_enc_sha256.cloned().unwrap_or_default(),
        file_length: file_length.unwrap_or_default(),
        kind,
        mimetype: mimetype.cloned().unwrap_or_default(),
    })
}

/// True when the message (or any wrapper) is a view-once message.
pub fn is_view_once(msg: &wa::Message) -> bool {
    let mut cur = msg;
    for _ in 0..4 {
        if cur.view_once_message.is_set() {
            return true;
        }
        if cur.image_message.as_option().and_then(|m| m.view_once) == Some(true)
            || cur.video_message.as_option().and_then(|m| m.view_once) == Some(true)
            || cur.audio_message.as_option().and_then(|m| m.view_once) == Some(true)
        {
            return true;
        }
        match cur
            .ephemeral_message
            .as_option()
            .or(cur.document_with_caption_message.as_option())
            .and_then(|f| f.message.as_option())
        {
            Some(m) => cur = m,
            None => return false,
        }
    }
    false
}

/// Converts a decrypted protobuf message body into [`Content`].
///
/// Returns `None` for messages that are not shown as bubbles (reactions,
/// key distribution, protocol messages other than "delete for everyone", ...).
pub fn content_from_wa(msg: &wa::Message) -> Option<Content> {
    if is_view_once(msg) {
        // Never expose the media reference: not downloaded, not cached.
        return Some(Content::ViewOnce);
    }
    let m = base_message(msg);

    if let Some(text) = non_empty(m.conversation.as_ref()) {
        return Some(Content::Text(text));
    }
    if let Some(ext) = m.extended_text_message.as_option()
        && let Some(text) = non_empty(ext.text.as_ref())
    {
        return Some(Content::Text(text));
    }
    if let Some(img) = m.image_message.as_option() {
        return Some(Content::Image {
            caption: non_empty(img.caption.as_ref()),
            width: img.width.unwrap_or_default(),
            height: img.height.unwrap_or_default(),
            thumbnail: img.jpeg_thumbnail.clone().filter(|t| !t.is_empty()),
            media: media_ref(
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
    if let Some(doc) = m.document_message.as_option() {
        let file_name = non_empty(doc.file_name.as_ref())
            .or_else(|| non_empty(doc.title.as_ref()))
            .unwrap_or_else(|| "Document".into());
        return Some(Content::Document {
            file_name,
            mimetype: doc.mimetype.clone().unwrap_or_default(),
            size: doc.file_length.unwrap_or_default(),
            pages: doc.page_count.filter(|p| *p > 0),
            caption: non_empty(doc.caption.as_ref()),
            media: media_ref(
                MediaKind::Document,
                doc.direct_path.as_ref(),
                doc.media_key.as_ref(),
                doc.file_sha256.as_ref(),
                doc.file_enc_sha256.as_ref(),
                doc.file_length,
                doc.mimetype.as_ref(),
            ),
        });
    }
    if let Some(audio) = m.audio_message.as_option() {
        return Some(Content::Voice {
            seconds: audio.seconds.unwrap_or_default(),
            ptt: audio.ptt.unwrap_or(false),
            waveform: audio
                .waveform
                .iter()
                .flatten()
                .take(64)
                .map(|v| (*v).min(100))
                .collect(),
            media: media_ref(
                MediaKind::Audio,
                audio.direct_path.as_ref(),
                audio.media_key.as_ref(),
                audio.file_sha256.as_ref(),
                audio.file_enc_sha256.as_ref(),
                audio.file_length,
                audio.mimetype.as_ref(),
            ),
        });
    }
    if let Some(p) = m.protocol_message.as_option() {
        use wa::message::protocol_message::Type;
        return match p.r#type {
            Some(Type::REVOKE) => Some(Content::Deleted),
            _ => None,
        };
    }
    if m.video_message.is_set() || m.ptv_message.is_set() {
        return Some(Content::Unsupported("video".into()));
    }
    if m.sticker_message.is_set() {
        return Some(Content::Unsupported("sticker".into()));
    }
    if m.location_message.is_set() || m.live_location_message.is_set() {
        return Some(Content::Unsupported("location".into()));
    }
    if m.contact_message.is_set() || m.contacts_array_message.is_set() {
        return Some(Content::Unsupported("contact card".into()));
    }
    if m.poll_creation_message.is_set() {
        return Some(Content::Unsupported("poll".into()));
    }
    None
}

/// The id of the message a "delete for everyone" refers to.
pub fn revoked_target(msg: &wa::Message) -> Option<String> {
    let m = base_message(msg);
    let p = m.protocol_message.as_option()?;
    if p.r#type != Some(wa::message::protocol_message::Type::REVOKE) {
        return None;
    }
    p.key.as_option()?.id.clone()
}

/// Strips the agent and device parts from a JID
/// (`123.0:4@s.whatsapp.net` → `123@s.whatsapp.net`). Group ids keep their
/// shape (they contain neither).
pub fn bare_jid(jid: &str) -> String {
    match jid.split_once('@') {
        Some((user, server)) => {
            let user = user.split(':').next().unwrap_or(user);
            let user = user.split('.').next().unwrap_or(user);
            format!("{user}@{server}")
        }
        None => jid.to_string(),
    }
}

/// Learns `(lid, pn)` from a chat/sender and its alternate addressing.
pub fn lid_pair(primary: &str, alt: Option<&str>) -> Option<(String, String)> {
    let (p, a) = (bare_jid(primary), bare_jid(alt?));
    match (p.ends_with("@lid"), a.ends_with("@s.whatsapp.net")) {
        (true, true) => Some((p, a)),
        _ if a.ends_with("@lid") && p.ends_with("@s.whatsapp.net") => Some((a, p)),
        _ => None,
    }
}

/// Fields of a live message envelope the converter needs. Decoupled from
/// `whatsapp_rust::types::message::MessageInfo` so fixtures can build it.
#[derive(Debug, Clone)]
pub struct Envelope {
    /// Message id.
    pub id: String,
    /// Chat JID.
    pub chat: String,
    /// Sender JID.
    pub sender: String,
    /// Outgoing (sent from another of our devices)?
    pub from_me: bool,
    /// Push name.
    pub push_name: String,
    /// Unix seconds.
    pub timestamp: i64,
    /// Alternate addressing of the sender (PN for a LID sender or vice versa).
    pub sender_alt: Option<String>,
    /// Alternate addressing of the recipient (our own outgoing DMs).
    pub recipient_alt: Option<String>,
    /// Disappearing-message timer in seconds, if any.
    pub ephemeral_secs: Option<u32>,
}

impl Envelope {
    /// LID ↔ PN pairs this envelope reveals.
    pub fn lid_pairs(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        out.extend(lid_pair(&self.sender, self.sender_alt.as_deref()));
        if self.from_me {
            out.extend(lid_pair(&self.chat, self.recipient_alt.as_deref()));
        } else if !self.chat.ends_with("@g.us") {
            out.extend(lid_pair(&self.chat, self.sender_alt.as_deref()));
        }
        out
    }
}

/// Live inbound message → domain message.
pub fn message_from_envelope(env: &Envelope, msg: &wa::Message) -> Option<Message> {
    let content = content_from_wa(msg)?;
    Some(Message {
        id: env.id.clone(),
        chat: bare_jid(&env.chat),
        sender: bare_jid(&env.sender),
        sender_name: Some(env.push_name.clone()).filter(|n| !n.is_empty()),
        from_me: env.from_me,
        timestamp: env.timestamp,
        content,
        // Messages from our other devices already reached the server.
        status: if env.from_me {
            ReceiptStatus::Sent
        } else {
            ReceiptStatus::Read
        },
        expires_at: env
            .ephemeral_secs
            .filter(|s| *s > 0)
            .map(|s| env.timestamp + s as i64),
    })
}

/// History-sync status → receipt status.
pub fn status_from_web(status: Option<wa::web_message_info::Status>) -> ReceiptStatus {
    use wa::web_message_info::Status;
    match status {
        Some(Status::ERROR) => ReceiptStatus::Failed,
        Some(Status::PENDING) => ReceiptStatus::Pending,
        Some(Status::SERVER_ACK) | None => ReceiptStatus::Sent,
        Some(Status::DELIVERY_ACK) => ReceiptStatus::Delivered,
        Some(Status::READ) => ReceiptStatus::Read,
        Some(Status::PLAYED) => ReceiptStatus::Played,
    }
}

/// History-sync message → domain message. `ephemeral_secs` is the chat's
/// disappearing-messages timer.
pub fn message_from_web_info(
    chat: &str,
    wmi: &wa::WebMessageInfo,
    ephemeral_secs: Option<u32>,
) -> Option<Message> {
    let key = wmi.key.as_option()?;
    let id = key.id.clone()?;
    let content = content_from_wa(wmi.message.as_option()?)?;
    let from_me = key.from_me.unwrap_or(false);
    let sender = if from_me {
        String::new()
    } else {
        wmi.participant
            .clone()
            .or_else(|| key.participant.clone())
            .unwrap_or_else(|| chat.to_string())
    };
    let timestamp = wmi.message_timestamp.unwrap_or_default() as i64;
    Some(Message {
        id,
        chat: bare_jid(chat),
        sender: bare_jid(&sender),
        sender_name: non_empty(wmi.push_name.as_ref()),
        from_me,
        timestamp,
        content,
        status: if from_me {
            status_from_web(wmi.status)
        } else {
            ReceiptStatus::Read
        },
        expires_at: ephemeral_secs
            .filter(|s| *s > 0)
            .map(|s| timestamp + s as i64),
    })
}

/// History-sync conversation → chat metadata.
pub fn chat_from_conversation(conv: &wa::Conversation, now: i64) -> ChatMeta {
    let last_ts = conv
        .conversation_timestamp
        .or(conv.last_msg_timestamp)
        .unwrap_or_default() as i64;
    ChatMeta {
        id: bare_jid(&conv.id),
        name: non_empty(conv.name.as_ref()),
        unread: conv.unread_count.unwrap_or_default(),
        last_ts,
        pinned: conv.pinned.unwrap_or_default() > 0,
        archived: conv.archived.unwrap_or(false),
        muted: conv.mute_end_time.is_some_and(|t| t as i64 > now),
    }
}

/// Everything a history-sync blob carries that the app uses.
#[derive(Debug, Default)]
pub struct HistoryParts {
    /// Chats.
    pub chats: Vec<ChatMeta>,
    /// Messages.
    pub messages: Vec<Message>,
    /// Push names `(jid, name)`.
    pub names: Vec<(String, String)>,
    /// LID → PN pairs.
    pub lid_pairs: Vec<(String, String)>,
}

/// Converts a full history-sync blob into chats, messages, names and LID
/// mappings.
pub fn from_history(hs: &wa::HistorySync, now: i64) -> HistoryParts {
    let mut parts = HistoryParts::default();
    for m in &hs.phone_number_to_lid_mappings {
        if let (Some(pn), Some(lid)) = (&m.pn_jid, &m.lid_jid)
            && let Some(pair) = lid_pair(lid, Some(pn))
        {
            parts.lid_pairs.push(pair);
        }
    }
    for conv in &hs.conversations {
        if conv.id.ends_with("@broadcast") || conv.id.ends_with("@newsletter") {
            continue;
        }
        let alt = conv.pn_jid.as_deref().or(conv.lid_jid.as_deref());
        if let Some(pair) = lid_pair(&conv.id, alt) {
            parts.lid_pairs.push(pair);
        }
        parts.chats.push(chat_from_conversation(conv, now));
        for hm in &conv.messages {
            if let Some(m) = hm
                .message
                .as_option()
                .and_then(|w| message_from_web_info(&conv.id, w, conv.ephemeral_expiration))
            {
                parts.messages.push(m);
            }
        }
    }
    parts.names = hs
        .pushnames
        .iter()
        .filter_map(|p| Some((bare_jid(p.id.as_ref()?), non_empty(p.pushname.as_ref())?)))
        .collect();
    parts
}

/// Receipt type → tick state. `None` for receipts that do not change ticks.
pub fn receipt_status(t: &ReceiptType) -> Option<ReceiptStatus> {
    match t {
        ReceiptType::Delivered => Some(ReceiptStatus::Delivered),
        ReceiptType::Sent => Some(ReceiptStatus::Sent),
        ReceiptType::Read => Some(ReceiptStatus::Read),
        ReceiptType::Played => Some(ReceiptStatus::Played),
        _ => None,
    }
}

/// True for receipts meaning "we read this chat on another device".
pub fn is_self_read(t: &ReceiptType) -> bool {
    matches!(t, ReceiptType::ReadSelf | ReceiptType::PlayedSelf)
}

/// Builds a plain text message.
pub fn build_text(text: &str) -> wa::Message {
    wa::Message {
        conversation: Some(text.to_string()),
        ..Default::default()
    }
}

/// CDN result of an upload, decoupled from the library type for tests.
#[derive(Debug, Clone)]
pub struct Uploaded {
    /// Full URL.
    pub url: String,
    /// CDN path.
    pub direct_path: String,
    /// Media key.
    pub media_key: Vec<u8>,
    /// SHA-256 of the plaintext.
    pub file_sha256: Vec<u8>,
    /// SHA-256 of the ciphertext.
    pub file_enc_sha256: Vec<u8>,
    /// Plaintext length.
    pub file_length: u64,
    /// Key timestamp.
    pub media_key_timestamp: i64,
    /// Streaming sidecar for audio.
    pub streaming_sidecar: Option<Vec<u8>>,
}

/// Builds the protobuf for uploaded media plus the matching local content.
pub fn build_media(media: &OutgoingMedia, up: &Uploaded) -> (wa::Message, Content) {
    let kind = match media.kind {
        OutgoingKind::Image { .. } => MediaKind::Image,
        OutgoingKind::Document => MediaKind::Document,
        OutgoingKind::Voice { .. } => MediaKind::Audio,
    };
    let mref = MediaRef {
        direct_path: up.direct_path.clone(),
        media_key: up.media_key.clone(),
        file_sha256: up.file_sha256.clone(),
        file_enc_sha256: up.file_enc_sha256.clone(),
        file_length: up.file_length,
        kind,
        mimetype: media.mimetype.clone(),
    };
    let content = media.local_content(Some(mref));
    let msg = match &media.kind {
        OutgoingKind::Image {
            width,
            height,
            thumbnail,
        } => wa::Message {
            image_message: buffa_some(wa::message::ImageMessage {
                url: Some(up.url.clone()),
                mimetype: Some(media.mimetype.clone()),
                caption: media.caption.clone(),
                file_sha256: Some(up.file_sha256.clone()),
                file_length: Some(up.file_length),
                height: Some(*height),
                width: Some(*width),
                media_key: Some(up.media_key.clone()),
                file_enc_sha256: Some(up.file_enc_sha256.clone()),
                direct_path: Some(up.direct_path.clone()),
                media_key_timestamp: Some(up.media_key_timestamp),
                jpeg_thumbnail: Some(thumbnail.clone()),
                ..Default::default()
            }),
            ..Default::default()
        },
        OutgoingKind::Document => wa::Message {
            document_message: buffa_some(wa::message::DocumentMessage {
                url: Some(up.url.clone()),
                mimetype: Some(media.mimetype.clone()),
                title: Some(media.file_name.clone()),
                file_name: Some(media.file_name.clone()),
                file_sha256: Some(up.file_sha256.clone()),
                file_length: Some(up.file_length),
                media_key: Some(up.media_key.clone()),
                file_enc_sha256: Some(up.file_enc_sha256.clone()),
                direct_path: Some(up.direct_path.clone()),
                media_key_timestamp: Some(up.media_key_timestamp),
                caption: media.caption.clone(),
                ..Default::default()
            }),
            ..Default::default()
        },
        OutgoingKind::Voice { seconds, waveform } => wa::Message {
            audio_message: buffa_some(wa::message::AudioMessage {
                url: Some(up.url.clone()),
                mimetype: Some(media.mimetype.clone()),
                file_sha256: Some(up.file_sha256.clone()),
                file_length: Some(up.file_length),
                seconds: Some(*seconds),
                ptt: Some(true),
                media_key: Some(up.media_key.clone()),
                file_enc_sha256: Some(up.file_enc_sha256.clone()),
                direct_path: Some(up.direct_path.clone()),
                media_key_timestamp: Some(up.media_key_timestamp),
                streaming_sidecar: up.streaming_sidecar.clone(),
                waveform: Some(waveform.clone()),
                ..Default::default()
            }),
            ..Default::default()
        },
    };
    (msg, content)
}

fn buffa_some<T: Default>(v: T) -> whatsapp_rust::waproto::buffa::MessageField<T> {
    whatsapp_rust::waproto::buffa::MessageField::some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(from_me: bool) -> Envelope {
        Envelope {
            id: "ABC".into(),
            chat: "34600111222@s.whatsapp.net".into(),
            sender: "34600111222:3@s.whatsapp.net".into(),
            from_me,
            push_name: "Ana".into(),
            timestamp: 1_700_000_000,
            sender_alt: None,
            recipient_alt: None,
            ephemeral_secs: None,
        }
    }

    #[test]
    fn text_and_extended_text() {
        let m = build_text("hola");
        assert_eq!(content_from_wa(&m), Some(Content::Text("hola".into())));
        let ext = wa::Message {
            extended_text_message: buffa_some(wa::message::ExtendedTextMessage {
                text: Some("link https://x".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            content_from_wa(&ext),
            Some(Content::Text("link https://x".into()))
        );
    }

    #[test]
    fn unwraps_ephemeral() {
        let inner = build_text("disappearing");
        let m = wa::Message {
            ephemeral_message: buffa_some(wa::message::FutureProofMessage {
                message: buffa_some(inner),
            }),
            ..Default::default()
        };
        assert_eq!(
            content_from_wa(&m),
            Some(Content::Text("disappearing".into()))
        );
    }

    #[test]
    fn envelope_conversion_strips_device() {
        let m = message_from_envelope(&env(false), &build_text("hi")).unwrap();
        assert_eq!(m.sender, "34600111222@s.whatsapp.net");
        assert_eq!(m.sender_name.as_deref(), Some("Ana"));
        assert!(!m.from_me);
        let mine = message_from_envelope(&env(true), &build_text("hi")).unwrap();
        assert_eq!(mine.status, ReceiptStatus::Sent);
    }

    #[test]
    fn reactions_are_not_bubbles() {
        let m = wa::Message {
            reaction_message: buffa_some(wa::message::ReactionMessage::default()),
            ..Default::default()
        };
        assert_eq!(content_from_wa(&m), None);
    }

    #[test]
    fn revoke_becomes_deleted() {
        let m = wa::Message {
            protocol_message: buffa_some(wa::message::ProtocolMessage {
                key: buffa_some(wa::MessageKey {
                    id: Some("TARGET".into()),
                    ..Default::default()
                }),
                r#type: Some(wa::message::protocol_message::Type::REVOKE),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(content_from_wa(&m), Some(Content::Deleted));
        assert_eq!(revoked_target(&m).as_deref(), Some("TARGET"));
    }

    #[test]
    fn receipt_mapping() {
        assert_eq!(
            receipt_status(&ReceiptType::Read),
            Some(ReceiptStatus::Read)
        );
        assert_eq!(
            receipt_status(&ReceiptType::Delivered),
            Some(ReceiptStatus::Delivered)
        );
        assert_eq!(receipt_status(&ReceiptType::Retry), None);
        assert!(is_self_read(&ReceiptType::ReadSelf));
    }

    #[test]
    fn media_roundtrip_build_then_parse() {
        let media = OutgoingMedia {
            kind: OutgoingKind::Voice {
                seconds: 7,
                waveform: vec![10; 64],
            },
            bytes: vec![1, 2, 3],
            file_name: "voice.ogg".into(),
            mimetype: "audio/ogg; codecs=opus".into(),
            caption: None,
        };
        let up = Uploaded {
            url: "https://mmg.whatsapp.net/x".into(),
            direct_path: "/v/t62/x".into(),
            media_key: vec![7; 32],
            file_sha256: vec![1; 32],
            file_enc_sha256: vec![2; 32],
            file_length: 3,
            media_key_timestamp: 1,
            streaming_sidecar: None,
        };
        let (msg, local) = build_media(&media, &up);
        let parsed = content_from_wa(&msg).unwrap();
        assert_eq!(parsed, local);
        match parsed {
            Content::Voice {
                seconds,
                ptt,
                media,
                ..
            } => {
                assert_eq!(seconds, 7);
                assert!(ptt);
                assert_eq!(media.unwrap().direct_path, "/v/t62/x");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn view_once_is_never_exposed() {
        let m = wa::Message {
            view_once_message: buffa_some(wa::message::FutureProofMessage {
                message: buffa_some(wa::Message {
                    image_message: buffa_some(wa::message::ImageMessage {
                        direct_path: Some("/secret".into()),
                        media_key: Some(vec![1; 32]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        assert_eq!(content_from_wa(&m), Some(Content::ViewOnce));
        let flagged = wa::Message {
            image_message: buffa_some(wa::message::ImageMessage {
                view_once: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(content_from_wa(&flagged), Some(Content::ViewOnce));
    }

    #[test]
    fn waveform_values_are_clamped() {
        let m = wa::Message {
            audio_message: buffa_some(wa::message::AudioMessage {
                waveform: Some(vec![250; 80]),
                ..Default::default()
            }),
            ..Default::default()
        };
        match content_from_wa(&m) {
            Some(Content::Voice { waveform, .. }) => {
                assert_eq!(waveform.len(), 64);
                assert!(waveform.iter().all(|v| *v == 100));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lid_pairs_from_envelopes() {
        let mut e = env(false);
        e.chat = "111@lid".into();
        e.sender = "111:2@lid".into();
        e.sender_alt = Some("34600111222:2@s.whatsapp.net".into());
        let pairs = e.lid_pairs();
        assert!(pairs.contains(&("111@lid".into(), "34600111222@s.whatsapp.net".into())));
        let mut mine = env(true);
        mine.chat = "222@lid".into();
        mine.sender = "34600000000@s.whatsapp.net".into();
        mine.recipient_alt = Some("34600333444@s.whatsapp.net".into());
        assert!(
            mine.lid_pairs()
                .contains(&("222@lid".into(), "34600333444@s.whatsapp.net".into()))
        );
        assert_eq!(lid_pair("1@g.us", Some("x@lid")), None);
    }

    #[test]
    fn ephemeral_expiry_is_computed() {
        let mut e = env(false);
        e.ephemeral_secs = Some(86_400);
        let m = message_from_envelope(&e, &build_text("x")).unwrap();
        assert_eq!(m.expires_at, Some(1_700_000_000 + 86_400));
    }

    #[test]
    fn bare_jid_variants() {
        assert_eq!(bare_jid("1:2@s.whatsapp.net"), "1@s.whatsapp.net");
        assert_eq!(bare_jid("1.0:2@s.whatsapp.net"), "1@s.whatsapp.net");
        assert_eq!(bare_jid("123@g.us"), "123@g.us");
        assert_eq!(bare_jid("weird"), "weird");
    }
}
