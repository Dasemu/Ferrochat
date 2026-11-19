//! Desktop integration: notifications (D-Bus via `notify-rust`) and opening
//! files with the system handler.

use std::path::Path;

use crate::config::{APP_ID, NotificationPreview};

/// Escapes `&`, `<`, `>` (KDE / GNOME interpret a subset of HTML markup in
/// notification text).
pub fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Builds the notification `(summary, body)` following the privacy setting.
/// Both are markup-escaped.
pub fn notification_text(
    sender: &str,
    group: Option<&str>,
    preview: &str,
    mode: NotificationPreview,
) -> (String, String) {
    let summary = match group {
        Some(g) => format!("{sender} @ {g}"),
        None => sender.to_string(),
    };
    let body = match mode {
        NotificationPreview::SenderAndPreview => preview.chars().take(160).collect(),
        NotificationPreview::SenderOnly => "New message".to_string(),
    };
    (escape_markup(&summary), escape_markup(&body))
}

/// Shows a notification. Blocking (D-Bus round trip): call from
/// `spawn_blocking`. Failures are logged, never fatal.
pub fn show(summary: &str, body: &str) {
    let mut n = notify_rust::Notification::new();
    n.appname("Ferrochat")
        .summary(summary)
        .body(body)
        .icon("mail-message-new");
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        n.hint(notify_rust::Hint::DesktopEntry(APP_ID.into()));
        n.hint(notify_rust::Hint::Category("im.received".into()));
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = APP_ID;
    if let Err(e) = n.show() {
        tracing::warn!(error = %e, "notification failed");
    }
}

/// Opens a file with the desktop's default application.
pub fn open_path(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path).spawn().map(|_| ())
}

/// Makes a received file name safe to write into the downloads folder.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').to_string();
    if trimmed.is_empty() {
        "document".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_modes() {
        let (s, b) = notification_text("Ana", None, "secret plan", NotificationPreview::SenderOnly);
        assert_eq!(s, "Ana");
        assert!(!b.contains("secret"));
        let (s, b) = notification_text(
            "Ana",
            Some("Family"),
            "hello",
            NotificationPreview::SenderAndPreview,
        );
        assert_eq!(s, "Ana @ Family");
        assert_eq!(b, "hello");
    }

    #[test]
    fn markup_is_escaped() {
        let (s, b) = notification_text(
            "<b>Eve</b>",
            None,
            "a & b <a href=x>click</a>",
            NotificationPreview::SenderAndPreview,
        );
        assert_eq!(s, "&lt;b&gt;Eve&lt;/b&gt;");
        assert_eq!(b, "a &amp; b &lt;a href=x&gt;click&lt;/a&gt;");
    }

    #[test]
    fn file_names_cannot_escape() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("..hidden"), "hidden");
        assert_eq!(sanitize_file_name(""), "document");
        assert_eq!(sanitize_file_name("Report Q3.pdf"), "Report Q3.pdf");
    }
}
