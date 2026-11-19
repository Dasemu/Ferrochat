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
    let cap = |s: &str| -> String {
        let mut out: String = s.chars().take(48).collect();
        if s.chars().count() > 48 {
            out.push('…');
        }
        out
    };
    let summary = match group {
        Some(g) => format!("{} @ {}", cap(sender), cap(g)),
        None => cap(sender),
    };
    let body = match mode {
        NotificationPreview::SenderAndPreview => preview.chars().take(160).collect(),
        NotificationPreview::SenderOnly => "New message".to_string(),
    };
    (escape_markup(&summary), escape_markup(&body))
}

/// One notification for messages drained from the offline queue.
pub fn summary_text(messages: usize, chats: usize) -> (String, String) {
    let body = if chats == 1 {
        format!(
            "{messages} new message{}",
            if messages == 1 { "" } else { "s" }
        )
    } else {
        format!("{messages} new messages in {chats} chats")
    };
    ("Ferrochat".into(), body)
}

/// Callback run when the user clicks a notification.
pub type OnClick = Box<dyn FnOnce() + Send>;

/// Notifications whose click we are still waiting for (each holds a thread).
#[cfg(all(unix, not(target_os = "macos")))]
static WAITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// At most this many click waiters at once; further notifications are shown
/// without a click action.
#[cfg(all(unix, not(target_os = "macos")))]
const MAX_WAITING: usize = 8;

/// Shows a notification. Blocking (D-Bus round trip, and until the
/// notification is dismissed when `on_click` is given): call from
/// `spawn_blocking`. Clicking the notification runs `on_click`. Failures are
/// logged, never fatal.
pub fn show(summary: &str, body: &str, on_click: Option<OnClick>) {
    let mut n = notify_rust::Notification::new();
    n.appname("Ferrochat")
        .summary(summary)
        .body(body)
        .icon("mail-message-new");
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::sync::atomic::Ordering;
        n.hint(notify_rust::Hint::DesktopEntry(APP_ID.into()));
        n.hint(notify_rust::Hint::Category("im.received".into()));
        let wait = on_click.is_some()
            && WAITING
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                    (c < MAX_WAITING).then_some(c + 1)
                })
                .is_ok();
        if wait {
            // "default" is the action servers invoke when the body is clicked.
            n.action("default", "Open");
        }
        match n.show() {
            Ok(handle) if wait => {
                let mut cb = on_click;
                handle.wait_for_action(|action| {
                    if action == "default"
                        && let Some(f) = cb.take()
                    {
                        f();
                    }
                });
                WAITING.fetch_sub(1, Ordering::SeqCst);
            }
            Ok(_) => {}
            Err(e) => {
                if wait {
                    WAITING.fetch_sub(1, Ordering::SeqCst);
                }
                tracing::warn!(error = %e, "notification failed");
            }
        }
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let _ = (APP_ID, on_click);
        if let Err(e) = n.show() {
            tracing::warn!(error = %e, "notification failed");
        }
    }
}

/// Opens a file with the desktop's default application, without a shell:
/// `xdg-open` / `open` with the path as one argument, and `ShellExecuteW`
/// on Windows (the `open` crate's `shellexecute-on-windows` feature), so
/// file names cannot inject commands.
pub fn open_path(path: &Path) -> std::io::Result<()> {
    open::that_detached(path)
}

/// Extensions that run code or render active content when "opened". Such
/// documents are saved but never launched automatically.
pub const DANGEROUS_EXTENSIONS: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "msi",
    "msp",
    "scr",
    "pif",
    "cpl",
    "lnk",
    "url",
    "reg",
    "jar",
    "jnlp",
    "app",
    "command",
    "tool",
    "pkg",
    "dmg",
    "desktop",
    "sh",
    "bash",
    "zsh",
    "csh",
    "ksh",
    "run",
    "bin",
    "appimage",
    "py",
    "pyw",
    "pl",
    "rb",
    "php",
    "html",
    "htm",
    "xhtml",
    "svg",
    "svgz",
    "mht",
    "mhtml",
    "xml",
    "xsl",
    "chm",
    "iso",
    "img",
    "vhd",
    "vhdx",
    "deb",
    "rpm",
    "flatpakref",
    "snap",
];

/// True when opening a file with this name could execute code.
pub fn is_dangerous(file_name: &str) -> bool {
    let lower = file_name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    match lower.rsplit_once('.') {
        Some((_, ext)) => DANGEROUS_EXTENSIONS.contains(&ext),
        // No extension: could be an ELF / script; do not launch it.
        None => true,
    }
}

const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Makes a received file name safe to write into the downloads folder:
/// no separators or control characters, no leading dots, no trailing dots
/// or spaces (Windows strips them, changing the extension), no reserved
/// Windows device names, at most 120 characters.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let mut out: String = cleaned
        .trim()
        .trim_start_matches('.')
        .chars()
        .take(120)
        .collect();
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        return "document".into();
    }
    let stem = out
        .split('.')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    out
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
    fn dangerous_extensions_are_not_launched() {
        for n in [
            "setup.exe",
            "run.BAT",
            "x.cmd",
            "a.ps1",
            "s.lnk",
            "y.scr",
            "j.jar",
            "m.msi",
            "z.command",
            "app.desktop",
            "i.sh",
            "t.AppImage",
            "page.html",
            "img.svg",
            "noext",
            "evil.exe.",
            "evil.exe ",
        ] {
            assert!(is_dangerous(n), "{n}");
        }
        for n in ["report.pdf", "photo.jpg", "notes.txt", "sheet.xlsx"] {
            assert!(!is_dangerous(n), "{n}");
        }
    }

    #[test]
    fn reserved_and_trailing_names() {
        assert_eq!(sanitize_file_name("CON"), "_CON");
        assert_eq!(sanitize_file_name("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_file_name("Lpt1.pdf"), "_Lpt1.pdf");
        assert_eq!(sanitize_file_name("invoice.pdf. . "), "invoice.pdf");
        assert_eq!(sanitize_file_name("console.txt"), "console.txt");
        assert_eq!(sanitize_file_name("a&b^c%d.pdf"), "a&b^c%d.pdf");
    }

    #[test]
    fn file_names_cannot_escape() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("..hidden"), "hidden");
        assert_eq!(sanitize_file_name(""), "document");
        assert_eq!(sanitize_file_name("Report Q3.pdf"), "Report Q3.pdf");
    }
}
