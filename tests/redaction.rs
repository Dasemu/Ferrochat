//! Formatted log output never contains known secret fixtures.

use std::io::Write;
use std::sync::{Arc, Mutex};

use ferrochat::auth::secrets::Secret;
use ferrochat::error::{MaskedJid, Redacted};
use ferrochat::net::{Content, Event, MediaKind, MediaRef, Message, ReceiptStatus};

const BODY: &str = "my bank PIN is 4711";
const PHONE: &str = "34612345678";
const TOKEN: &str = "s3cr3t-signal-key-0123456789abcdef";
const FILE: &str = "medical-report.pdf";

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn logs_do_not_leak_secrets() {
    let buf = Buf::default();
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let jid = format!("{PHONE}@s.whatsapp.net");
    let msg = Message {
        id: "ID1".into(),
        chat: jid.clone(),
        sender: jid.clone(),
        sender_name: Some("Ana".into()),
        from_me: false,
        timestamp: 1,
        content: Content::Text(BODY.into()),
        status: ReceiptStatus::Read,
    };
    let doc = Content::Document {
        file_name: FILE.into(),
        mimetype: "application/pdf".into(),
        size: 1,
        pages: None,
        caption: Some(BODY.into()),
        media: Some(MediaRef {
            direct_path: "/p".into(),
            media_key: TOKEN.as_bytes().to_vec(),
            file_sha256: vec![],
            file_enc_sha256: vec![],
            file_length: 1,
            kind: MediaKind::Document,
            mimetype: "application/pdf".into(),
        }),
    };
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(body = %Redacted(BODY), chat = %MaskedJid(&jid), "sending");
        tracing::info!(secret = ?Secret::new(TOKEN), "keyring");
        tracing::debug!(content = ?msg.content, doc = ?doc, "converted");
        tracing::debug!(event = ?Event::Messages { messages: vec![msg.clone()], live: true }, "net");
        tracing::debug!(event = ?Event::Qr { code: TOKEN.into(), valid_for_secs: 20 }, "net");
        tracing::debug!(err = %ferrochat::error::Error::Net(TOKEN.into()).user_message(), "user");
    });
    let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(out.contains("sending"), "logging worked: {out}");
    for needle in [BODY, PHONE, TOKEN, FILE, "4711"] {
        assert!(!out.contains(needle), "leaked {needle:?} in:\n{out}");
    }
}
