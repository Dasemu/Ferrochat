//! Regenerates `tests/fixtures/*.json` (hand-written payloads in the
//! protobuf-JSON shape `waproto` serialises to). Run with
//! `cargo test --test gen_fixtures -- --ignored`.

use whatsapp_rust::waproto::buffa::MessageField;
use whatsapp_rust::waproto::whatsapp as wa;

fn some<T: Default>(v: T) -> MessageField<T> {
    MessageField::some(v)
}

fn strip(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, x)| !x.is_null() && x.as_array().is_none_or(|a| !a.is_empty()))
                .map(|(k, x)| (k, strip(x)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(strip).collect()),
        other => other,
    }
}

fn write(name: &str, v: &impl serde::Serialize) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let value = strip(serde_json::to_value(v).unwrap());
    std::fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

#[test]
#[ignore]
fn regenerate() {
    let image = wa::Message {
        image_message: some(wa::message::ImageMessage {
            url: Some("https://mmg.whatsapp.net/v/t62.7118-24/fixture.enc".into()),
            mimetype: Some("image/jpeg".into()),
            caption: Some("Sunset".into()),
            file_sha256: Some(vec![0xaa; 32]),
            file_length: Some(48213),
            height: Some(1080),
            width: Some(1920),
            media_key: Some(vec![0x11; 32]),
            file_enc_sha256: Some(vec![0xbb; 32]),
            direct_path: Some("/v/t62.7118-24/fixture.enc".into()),
            media_key_timestamp: Some(1_700_000_000),
            jpeg_thumbnail: Some(vec![0xff, 0xd8, 0xff, 0xd9]),
            ..Default::default()
        }),
        ..Default::default()
    };
    write("image_message.json", &image);
    let voice = wa::Message {
        ephemeral_message: some(wa::message::FutureProofMessage {
            message: some(wa::Message {
                audio_message: some(wa::message::AudioMessage {
                    mimetype: Some("audio/ogg; codecs=opus".into()),
                    file_sha256: Some(vec![0xcc; 32]),
                    file_length: Some(12000),
                    seconds: Some(9),
                    ptt: Some(true),
                    media_key: Some(vec![0x22; 32]),
                    file_enc_sha256: Some(vec![0xdd; 32]),
                    direct_path: Some("/v/t62.7117-24/voice.enc".into()),
                    waveform: Some((0..64).map(|i| (i * 3 % 100) as u8).collect()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        }),
        ..Default::default()
    };
    write("voice_note_ephemeral.json", &voice);
    let doc = wa::Message {
        document_with_caption_message: some(wa::message::FutureProofMessage {
            message: some(wa::Message {
                document_message: some(wa::message::DocumentMessage {
                    mimetype: Some("application/pdf".into()),
                    title: Some("Invoice".into()),
                    file_name: Some("invoice-2026.pdf".into()),
                    file_length: Some(184320),
                    page_count: Some(3),
                    caption: Some("Here you go".into()),
                    media_key: Some(vec![0x33; 32]),
                    file_sha256: Some(vec![0xee; 32]),
                    file_enc_sha256: Some(vec![0x44; 32]),
                    direct_path: Some("/v/t62.7119-24/doc.enc".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        }),
        ..Default::default()
    };
    write("document_with_caption.json", &doc);
    let hs = wa::HistorySync {
        sync_type: wa::history_sync::HistorySyncType::INITIAL_BOOTSTRAP,
        progress: Some(40),
        conversations: vec![
            wa::Conversation {
                id: "34600111222@s.whatsapp.net".into(),
                unread_count: Some(2),
                conversation_timestamp: Some(1_700_000_500),
                pinned: Some(1_700_000_000),
                messages: vec![
                    wa::HistorySyncMsg {
                        message: some(wa::WebMessageInfo {
                            key: some(wa::MessageKey {
                                remote_jid: Some("34600111222@s.whatsapp.net".into()),
                                from_me: Some(true),
                                id: Some("3EB0HIST0001".into()),
                                ..Default::default()
                            }),
                            message: some(wa::Message {
                                conversation: Some("See you tomorrow".into()),
                                ..Default::default()
                            }),
                            message_timestamp: Some(1_700_000_400),
                            status: Some(wa::web_message_info::Status::READ),
                            ..Default::default()
                        }),
                        msg_order_id: Some(1),
                    },
                    wa::HistorySyncMsg {
                        message: some(wa::WebMessageInfo {
                            key: some(wa::MessageKey {
                                remote_jid: Some("34600111222@s.whatsapp.net".into()),
                                from_me: Some(false),
                                id: Some("3EB0HIST0002".into()),
                                ..Default::default()
                            }),
                            message: some(wa::Message {
                                conversation: Some("Great!".into()),
                                ..Default::default()
                            }),
                            message_timestamp: Some(1_700_000_500),
                            push_name: Some("Ana".into()),
                            ..Default::default()
                        }),
                        msg_order_id: Some(2),
                    },
                ],
                ..Default::default()
            },
            wa::Conversation {
                id: "120363000000000009@g.us".into(),
                name: Some("Book club".into()),
                archived: Some(true),
                mute_end_time: Some(4_000_000_000),
                ..Default::default()
            },
            wa::Conversation {
                id: "status@broadcast".into(),
                ..Default::default()
            },
        ],
        pushnames: vec![wa::Pushname {
            id: Some("34600111222@s.whatsapp.net".into()),
            pushname: Some("Ana".into()),
        }],
        ..Default::default()
    };
    write("history_sync.json", &hs);
}
