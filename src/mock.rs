//! Mock data used by tests, `--mock` mode and screenshot mode.
//!
//! All names, numbers and messages are invented. Media is generated
//! procedurally (no external files).

use std::collections::HashMap;
use std::io::Cursor;

use crate::media::audio;
use crate::net::types::{ChatMeta, Content, MediaKind, MediaRef, Message, ReceiptStatus};

/// Own (fake) JID in mock mode.
pub const OWN_JID: &str = "34600000000@s.whatsapp.net";
/// The chat opened in the `chat` screenshot.
pub const SHOWCASE_CHAT: &str = "34611111111@s.whatsapp.net";

/// Everything the fake protocol client serves.
pub struct MockData {
    /// Own JID.
    pub own_jid: String,
    /// Chats.
    pub chats: Vec<ChatMeta>,
    /// Contact names.
    pub names: Vec<(String, String)>,
    /// Messages, any order.
    pub messages: Vec<Message>,
    /// Downloadable media keyed by `direct_path`.
    pub media: HashMap<String, Vec<u8>>,
}

fn media_ref(path: &str, kind: MediaKind, mimetype: &str, bytes: &[u8]) -> MediaRef {
    MediaRef {
        direct_path: path.into(),
        media_key: vec![0x42; 32],
        file_sha256: path.as_bytes().to_vec(),
        file_enc_sha256: vec![0; 32],
        file_length: bytes.len() as u64,
        kind,
        mimetype: mimetype.into(),
    }
}

/// A procedurally drawn "sunset over the sea" PNG (original artwork).
pub fn sunset_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let fy = y as f32 / h as f32;
        let fx = x as f32 / w as f32;
        let horizon = 0.62;
        let sun = ((fx - 0.5).powi(2) * 2.2 + (fy - 0.55).powi(2) * 4.0).sqrt();
        if fy < horizon {
            let t = fy / horizon;
            let mut r = 255.0 * (0.98 - 0.25 * t);
            let mut g = 120.0 + 80.0 * t;
            let mut b = 90.0 + 60.0 * (1.0 - t);
            if sun < 0.09 {
                r = 255.0;
                g = 236.0;
                b = 170.0;
            } else if sun < 0.2 {
                let k = 1.0 - (sun - 0.09) / 0.11;
                r += (255.0 - r) * k * 0.6;
                g += (210.0 - g) * k * 0.6;
            }
            image::Rgb([r as u8, g.min(255.0) as u8, b.min(255.0) as u8])
        } else {
            let t = (fy - horizon) / (1.0 - horizon);
            let wave = ((fx * 60.0 + fy * 140.0).sin() * 0.5 + 0.5) * 18.0;
            let glow = if (fx - 0.5).abs() < 0.08 * (1.0 - t) + 0.02 {
                60.0
            } else {
                0.0
            };
            image::Rgb([
                (30.0 + 60.0 * (1.0 - t) + glow + wave * 0.5) as u8,
                (50.0 + 50.0 * (1.0 - t) + glow * 0.7 + wave * 0.4) as u8,
                (90.0 + 50.0 * (1.0 - t) + wave) as u8,
            ])
        }
    });
    let mut out = Vec::new();
    let _ = img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

fn thumbnail_of(png: &[u8]) -> Option<Vec<u8>> {
    crate::media::image::probe_and_thumbnail(png)
        .ok()
        .map(|(_, _, t)| t)
}

/// A synthetic, speech-like voice note (amplitude-modulated tones).
pub fn voice_note(seconds: f32) -> (Vec<u8>, Vec<u8>) {
    let n = (audio::OPUS_RATE as f32 * seconds) as usize;
    let rate = audio::OPUS_RATE as f32;
    let pcm: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / rate;
            let syllable = ((t * 3.1).sin() * (t * 1.3).cos()).abs();
            let pitch = 180.0 + 40.0 * (t * 0.7).sin();
            let s = (t * pitch * std::f32::consts::TAU).sin() * 0.6
                + (t * pitch * 2.0 * std::f32::consts::TAU).sin() * 0.25;
            s * syllable * 0.5
        })
        .collect();
    let wf = audio::waveform(&pcm);
    let ogg = audio::encode_ogg_opus(&pcm).unwrap_or_default();
    (ogg, wf)
}

/// A tiny valid PDF.
pub fn sample_pdf() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n".to_vec()
}

struct B {
    msgs: Vec<Message>,
    n: u32,
}

impl B {
    fn push(
        &mut self,
        chat: &str,
        from: Option<(&str, &str)>,
        ts: i64,
        c: Content,
        st: ReceiptStatus,
    ) {
        self.n += 1;
        let (sender, name, from_me) = match from {
            Some((jid, name)) => (jid.to_string(), Some(name.to_string()), false),
            None => (OWN_JID.to_string(), None, true),
        };
        self.msgs.push(Message {
            id: format!("MOCK{:04}", self.n),
            chat: chat.into(),
            sender,
            sender_name: name,
            from_me,
            timestamp: ts,
            content: c,
            status: if from_me { st } else { ReceiptStatus::Read },
            expires_at: None,
        });
    }
}

fn text(s: &str) -> Content {
    Content::Text(s.into())
}

/// Builds the mock dataset relative to `now` (unix seconds).
pub fn mock_data(now: i64) -> MockData {
    use ReceiptStatus::*;
    let mut media = HashMap::new();
    let lucia = SHOWCASE_CHAT;
    let family = "120363000000000001@g.us";
    let marcos = "34622222222@s.whatsapp.net";
    let design = "120363000000000002@g.us";
    let carmen = "34633333333@s.whatsapp.net";
    let clinic = "34644444444@s.whatsapp.net";
    let pablo = "34655555555@s.whatsapp.net";
    let climbing = "120363000000000003@g.us";
    let elena = "34666666666@s.whatsapp.net";
    let mum = "34677777777@s.whatsapp.net";

    let names = vec![
        (lucia.to_string(), "Lucía Fernández".to_string()),
        (marcos.to_string(), "Marcos Díaz".to_string()),
        (carmen.to_string(), "Carmen Ruiz".to_string()),
        (clinic.to_string(), "Clínica Dental Sonrisa".to_string()),
        (pablo.to_string(), "Pablo".to_string()),
        (elena.to_string(), "Elena Martín".to_string()),
        (mum.to_string(), "Mamá".to_string()),
    ];

    let png = sunset_png(640, 420);
    let img_ref = media_ref("/mock/sunset.png", MediaKind::Image, "image/png", &png);
    let thumb = thumbnail_of(&png);
    media.insert(img_ref.direct_path.clone(), png);

    let pdf = sample_pdf();
    let pdf_ref = media_ref(
        "/mock/itinerary.pdf",
        MediaKind::Document,
        "application/pdf",
        &pdf,
    );
    let pdf_len = pdf.len() as u64;
    media.insert(pdf_ref.direct_path.clone(), pdf);

    let (ogg_in, wf_in) = voice_note(9.0);
    let voice_in = media_ref(
        "/mock/voice-in.ogg",
        MediaKind::Audio,
        "audio/ogg; codecs=opus",
        &ogg_in,
    );
    media.insert(voice_in.direct_path.clone(), ogg_in);
    let (ogg_out, wf_out) = voice_note(4.0);
    let voice_out = media_ref(
        "/mock/voice-out.ogg",
        MediaKind::Audio,
        "audio/ogg; codecs=opus",
        &ogg_out,
    );
    media.insert(voice_out.direct_path.clone(), ogg_out);

    let today = now - now.rem_euclid(86_400) + 9 * 3600;
    let mut b = B {
        msgs: Vec::new(),
        n: 0,
    };
    let l = Some((lucia, "Lucía"));
    // Showcase conversation: every bubble kind and every tick state.
    b.push(
        lucia,
        l,
        now - 86_400 - 3000,
        text("Hola! ¿Ya tienes los billetes para Lisboa? ✈️"),
        Read,
    );
    b.push(
        lucia,
        None,
        now - 86_400 - 2900,
        text("Sí, salimos el viernes a las 8:15"),
        Read,
    );
    b.push(
        lucia,
        None,
        now - 86_400 - 2890,
        text("Te paso el itinerario"),
        Read,
    );
    b.push(
        lucia,
        None,
        now - 86_400 - 2880,
        Content::Document {
            file_name: "Itinerario Lisboa.pdf".into(),
            mimetype: "application/pdf".into(),
            size: pdf_len.max(184_320),
            pages: Some(3),
            caption: None,
            media: Some(pdf_ref.clone()),
        },
        Read,
    );
    b.push(
        lucia,
        l,
        now - 3600,
        text("Perfecto, ¡gracias! Mira qué atardecer hubo ayer en la playa"),
        Read,
    );
    b.push(
        lucia,
        l,
        now - 3580,
        Content::Image {
            caption: Some("Desde el paseo marítimo 🌅".into()),
            width: 640,
            height: 420,
            thumbnail: thumb.clone(),
            media: Some(img_ref.clone()),
        },
        Read,
    );
    b.push(
        lucia,
        l,
        now - 3500,
        Content::Voice {
            seconds: 9,
            ptt: true,
            waveform: wf_in,
            media: Some(voice_in),
        },
        Read,
    );
    b.push(
        lucia,
        None,
        now - 1800,
        text("¡Qué pasada! Me encanta 😍"),
        Read,
    );
    b.push(
        lucia,
        None,
        now - 1700,
        Content::Voice {
            seconds: 4,
            ptt: true,
            waveform: wf_out,
            media: Some(voice_out),
        },
        Delivered,
    );
    b.push(
        lucia,
        None,
        now - 240,
        text("Nos vemos mañana en la estación, llevo yo el café ☕"),
        Sent,
    );

    let f1 = Some(("34688888888@s.whatsapp.net", "Tía Rosa"));
    let f2 = Some((mum, "Mamá"));
    b.push(
        family,
        f2,
        now - 7200,
        text("¿Quién viene el domingo a comer?"),
        Read,
    );
    b.push(family, None, now - 7000, text("Yo sí 🙋"), Read);
    b.push(family, f1, now - 900, text("Yo llevo el postre"), Read);
    b.push(family, f2, now - 600, text("Genial, hago paella"), Read);
    b.push(family, f1, now - 420, text("🥘🥘🥘"), Read);

    b.push(
        marcos,
        Some((marcos, "Marcos")),
        today - 86_400 * 2,
        text("¿Partido el jueves?"),
        Read,
    );
    b.push(
        marcos,
        None,
        today - 86_400 * 2 + 60,
        text("Claro, a las 20:00"),
        Read,
    );
    b.push(
        marcos,
        Some((marcos, "Marcos")),
        now - 5400,
        Content::Image {
            caption: None,
            width: 640,
            height: 420,
            thumbnail: thumb,
            media: Some(img_ref),
        },
        Read,
    );

    let d1 = Some(("34699999999@s.whatsapp.net", "Irene (UX)"));
    b.push(
        design,
        d1,
        now - 86_400 - 600,
        text("He subido los nuevos mockups al drive"),
        Read,
    );
    b.push(
        design,
        None,
        now - 86_400 - 500,
        text("Los reviso esta tarde 👀"),
        Delivered,
    );

    b.push(
        carmen,
        Some((carmen, "Carmen")),
        now - 86_400 * 3,
        text("Te llamo luego"),
        Read,
    );
    b.push(carmen, None, now - 86_400 * 3 + 30, text("Vale 👍"), Read);

    b.push(
        clinic,
        Some((clinic, "Clínica Dental Sonrisa")),
        now - 86_400 * 4,
        text("Le recordamos su cita el martes a las 10:30."),
        Read,
    );
    b.push(
        pablo,
        None,
        now - 86_400 * 5,
        text("¡Feliz cumpleaños, Pablo! 🎂"),
        Read,
    );
    b.push(
        pablo,
        Some((pablo, "Pablo")),
        now - 86_400 * 5 + 300,
        text("¡Muchas gracias!"),
        Read,
    );
    b.push(
        climbing,
        Some(("34612121212@s.whatsapp.net", "Jorge")),
        now - 86_400 * 6,
        text("Sábado en el rocódromo a las 11"),
        Read,
    );
    b.push(
        elena,
        None,
        now - 86_400 * 8,
        text("Te devuelvo el libro la semana que viene"),
        Read,
    );
    b.push(
        mum,
        Some((mum, "Mamá")),
        now - 86_400 * 9,
        text("Llámame cuando puedas ❤️"),
        Read,
    );

    let last_of = |chat: &str| {
        b.msgs
            .iter()
            .filter(|m| m.chat == chat)
            .map(|m| m.timestamp)
            .max()
            .unwrap_or(now)
    };
    let chat = |id: &str, name: Option<&str>, unread: u32, pinned: bool, muted: bool| ChatMeta {
        id: id.into(),
        name: name.map(Into::into),
        unread,
        last_ts: last_of(id),
        pinned,
        archived: false,
        muted,
    };
    let chats = vec![
        chat(lucia, None, 0, true, false),
        chat(family, Some("Familia 🏡"), 3, false, false),
        chat(marcos, None, 1, false, false),
        chat(design, Some("Equipo de Diseño"), 0, false, true),
        chat(carmen, None, 0, false, false),
        chat(clinic, None, 0, false, false),
        chat(pablo, None, 0, false, false),
        chat(climbing, Some("Escalada los sábados 🧗"), 12, false, true),
        chat(elena, None, 0, false, false),
        chat(mum, None, 0, false, false),
    ];

    MockData {
        own_jid: OWN_JID.into(),
        chats,
        names,
        messages: b.msgs,
        media,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_data_is_consistent() {
        let d = mock_data(1_700_000_000);
        assert_eq!(d.chats.len(), 10);
        // Every message belongs to a known chat; every media ref is downloadable.
        for m in &d.messages {
            assert!(d.chats.iter().any(|c| c.id == m.chat), "{}", m.chat);
            if let Some(r) = m.content.media() {
                assert!(d.media.contains_key(&r.direct_path));
            }
        }
        let showcase: Vec<_> = d
            .messages
            .iter()
            .filter(|m| m.chat == SHOWCASE_CHAT)
            .collect();
        for tag in ["text", "image", "document", "voice"] {
            assert!(
                showcase.iter().any(|m| m.content.kind_tag() == tag),
                "{tag}"
            );
        }
    }

    #[test]
    fn voice_note_decodes() {
        let (ogg, wf) = voice_note(1.0);
        assert_eq!(wf.len(), 64);
        let pcm = audio::decode_ogg_opus(&ogg).unwrap();
        assert!(pcm.duration_ms() >= 950);
    }
}
