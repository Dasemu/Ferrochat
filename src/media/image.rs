//! Image probing and JPEG thumbnails for outgoing photos, MIME sniffing.

use std::io::Cursor;
use std::path::Path;

use crate::error::{Error, Result};
use crate::net::types::{OutgoingKind, OutgoingMedia};

/// Longest side of the inline thumbnail sent with a photo.
pub const THUMB_SIDE: u32 = 96;

fn media_err(e: impl std::fmt::Display) -> Error {
    Error::Media(e.to_string())
}

/// Reads only the header and rejects images whose decoded size would exceed
/// [`crate::media::MAX_IMAGE_PIXELS`] (decompression bombs).
pub fn check_image_bounds(bytes: &[u8]) -> Result<(u32, u32)> {
    let (w, h) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(media_err)?
        .into_dimensions()
        .map_err(media_err)?;
    if w as u64 * h as u64 > crate::media::MAX_IMAGE_PIXELS {
        return Err(Error::Media(format!("image too large ({w}x{h})")));
    }
    Ok((w, h))
}

/// Decodes with allocation limits.
pub fn decode_limited(bytes: &[u8]) -> Result<image::DynamicImage> {
    check_image_bounds(bytes)?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(media_err)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::media::MAX_IMAGE_PIXELS * 4 + 16 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(media_err)
}

/// Decodes `bytes`, returns `(width, height, jpeg_thumbnail)`.
pub fn probe_and_thumbnail(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let img = decode_limited(bytes)?;
    let (w, h) = (img.width(), img.height());
    let thumb = img.thumbnail(THUMB_SIDE, THUMB_SIDE).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut out), 70)
        .encode_image(&thumb)
        .map_err(|e| Error::Media(e.to_string()))?;
    Ok((w, h, out))
}

/// MIME type from the file extension.
pub fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "zip" => "application/zip",
        "ogg" | "opus" => "audio/ogg; codecs=opus",
        "mp3" => "audio/mpeg",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "odt" => "application/vnd.oasis.opendocument.text",
        _ => "application/octet-stream",
    }
}

/// Turns a picked file into an [`OutgoingMedia`]: photos (JPEG/PNG/WebP that
/// decode) become images, everything else a document.
pub fn outgoing_from_file(path: &Path, bytes: Vec<u8>, caption: Option<String>) -> OutgoingMedia {
    let mimetype = mime_for_path(path);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    if mimetype.starts_with("image/")
        && let Ok((width, height, thumbnail)) = probe_and_thumbnail(&bytes)
    {
        // WhatsApp only renders JPEG photos reliably; others go as-is.
        return OutgoingMedia {
            kind: OutgoingKind::Image {
                width,
                height,
                thumbnail,
            },
            bytes,
            file_name,
            mimetype: mimetype.into(),
            caption,
        };
    }
    OutgoingMedia {
        kind: OutgoingKind::Document,
        bytes,
        file_name,
        mimetype: mimetype.into(),
        caption,
    }
}

/// Human-readable size (`1.2 MB`, `340 kB`).
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1000.0;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} kB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 128]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn thumbnails_keep_aspect() {
        let (w, h, thumb) = probe_and_thumbnail(&png(400, 200)).unwrap();
        assert_eq!((w, h), (400, 200));
        let t = image::load_from_memory(&thumb).unwrap();
        assert_eq!(t.width(), THUMB_SIDE);
        assert_eq!(t.height(), THUMB_SIDE / 2);
    }

    #[test]
    fn picks_kind_from_file() {
        let img = outgoing_from_file(Path::new("/x/Cat.PNG"), png(10, 10), None);
        assert!(matches!(img.kind, OutgoingKind::Image { width: 10, .. }));
        let doc = outgoing_from_file(Path::new("/x/report.pdf"), b"%PDF-1.4".to_vec(), None);
        assert_eq!(doc.kind, OutgoingKind::Document);
        assert_eq!(doc.mimetype, "application/pdf");
        assert_eq!(doc.file_name, "report.pdf");
        // A broken "png" falls back to a document instead of failing.
        let bad = outgoing_from_file(Path::new("/x/b.png"), b"nope".to_vec(), None);
        assert_eq!(bad.kind, OutgoingKind::Document);
    }

    #[test]
    fn rejects_decompression_bombs() {
        // A valid PNG header claiming 20000x20000 pixels.
        let mut out = Vec::new();
        let enc = image::codecs::png::PngEncoder::new(&mut out);
        use image::ImageEncoder;
        // Encode a tiny image, then patch the IHDR dimensions.
        enc.write_image(&[0u8; 3], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        out[16..20].copy_from_slice(&20_000u32.to_be_bytes());
        out[20..24].copy_from_slice(&20_000u32.to_be_bytes());
        assert!(check_image_bounds(&out).is_err());
        assert!(check_image_bounds(&png(10, 10)).is_ok());
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(34_000), "34 kB");
        assert_eq!(human_size(1_260_000), "1.3 MB");
    }
}
