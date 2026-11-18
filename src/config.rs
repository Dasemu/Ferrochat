//! Paths, command-line flags and environment overrides.

use std::path::PathBuf;

/// Application id used for the keyring service, the Wayland `app_id` and
/// desktop notifications.
pub const APP_ID: &str = "dev.ferro.ferrochat";

/// Which screen `--screenshot` renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenshotTarget {
    /// QR pairing screen.
    Login,
    /// Chat list with no conversation open.
    ChatList,
    /// Chat list plus an open conversation with every bubble kind.
    Chat,
}

impl ScreenshotTarget {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "login" | "qr" => Some(Self::Login),
            "chats" | "chat-list" => Some(Self::ChatList),
            "chat" | "conversation" => Some(Self::Chat),
            _ => None,
        }
    }
}

/// What the notification body shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationPreview {
    /// Sender and message preview (official client default).
    SenderAndPreview,
    /// Only the sender name.
    SenderOnly,
}

/// Runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Config {
    /// `$XDG_DATA_HOME/ferrochat`: cache database and the encrypted Signal store.
    pub data_dir: PathBuf,
    /// `$XDG_STATE_HOME/ferrochat`: log file.
    pub state_dir: PathBuf,
    /// Where documents are saved when opened.
    pub downloads_dir: PathBuf,
    /// Run against the in-process fake protocol with mock data.
    pub mock: bool,
    /// Render one screen to a PNG and exit.
    pub screenshot: Option<(ScreenshotTarget, PathBuf)>,
    /// Light theme instead of the default dark one.
    pub light_theme: bool,
    /// Desktop notification privacy level.
    pub notification_preview: NotificationPreview,
    /// Log `first_frame_ms` at startup.
    pub trace_startup: bool,
}

/// Result of parsing the command line.
#[derive(Debug)]
pub enum Cli {
    /// Run the app with this configuration.
    Run(Box<Config>),
    /// Print help and exit.
    Help,
    /// Bad arguments.
    Invalid(String),
}

/// Usage text for `--help`.
pub const USAGE: &str = "\
ferrochat - unofficial native WhatsApp desktop client

USAGE:
    ferrochat [--mock] [--screenshot <login|chats|chat> <out.png>]

OPTIONS:
    --mock                      Run with mock data and a fake protocol (no network)
    --screenshot <screen> <png> Render a screen (implies --mock), save it and exit
    -h, --help                  Show this help

ENVIRONMENT:
    RUST_LOG                    Log filter (default: info)
    FERRO_THEME=light|dark      Colour scheme (default: dark)
    FERRO_TRACE_STARTUP=1       Log first_frame_ms
    FERROCHAT_DATA_DIR          Override the data directory
    FERROCHAT_NOTIFY=sender     Notifications show the sender only
";

impl Config {
    /// Parses `args` (without the program name) and the environment.
    pub fn from_args<I: IntoIterator<Item = String>>(args: I) -> Cli {
        let mut mock = false;
        let mut screenshot = None;
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--mock" => mock = true,
                "-h" | "--help" => return Cli::Help,
                "--screenshot" => {
                    let (Some(target), Some(out)) = (it.next(), it.next()) else {
                        return Cli::Invalid("--screenshot needs <screen> <out.png>".into());
                    };
                    let Some(target) = ScreenshotTarget::parse(&target) else {
                        return Cli::Invalid(format!("unknown screen '{target}'"));
                    };
                    screenshot = Some((target, PathBuf::from(out)));
                    mock = true;
                }
                other => return Cli::Invalid(format!("unknown argument '{other}'")),
            }
        }
        let mut cfg = Self::defaults();
        cfg.mock = mock;
        cfg.screenshot = screenshot;
        Cli::Run(Box::new(cfg))
    }

    /// Default paths plus environment overrides.
    pub fn defaults() -> Self {
        let dirs = directories::ProjectDirs::from("dev", "ferro", "ferrochat");
        let data_dir = std::env::var_os("FERROCHAT_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs.as_ref().map(|d| d.data_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from(".ferrochat"));
        let state_dir = dirs
            .as_ref()
            .and_then(|d| d.state_dir().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| data_dir.join("state"));
        let downloads_dir = directories::UserDirs::new()
            .and_then(|u| u.download_dir().map(|p| p.to_path_buf()))
            .unwrap_or_else(std::env::temp_dir);
        Self {
            data_dir,
            state_dir,
            downloads_dir,
            mock: false,
            screenshot: None,
            light_theme: std::env::var("FERRO_THEME").is_ok_and(|v| v == "light"),
            notification_preview: if std::env::var("FERROCHAT_NOTIFY").is_ok_and(|v| v == "sender")
            {
                NotificationPreview::SenderOnly
            } else {
                NotificationPreview::SenderAndPreview
            },
            trace_startup: std::env::var("FERRO_TRACE_STARTUP").is_ok_and(|v| v == "1"),
        }
    }

    /// The SQLite message cache.
    pub fn cache_db(&self) -> PathBuf {
        self.data_dir.join("cache.sqlite3")
    }

    /// The SQLCipher-encrypted Signal/session store used by the protocol library.
    pub fn signal_db(&self) -> PathBuf {
        self.data_dir.join("session.sqlcipher")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_screenshot_and_implies_mock() {
        match Config::from_args(args(&["--screenshot", "chat", "/tmp/x.png"])) {
            Cli::Run(cfg) => {
                assert!(cfg.mock);
                assert_eq!(
                    cfg.screenshot,
                    Some((ScreenshotTarget::Chat, PathBuf::from("/tmp/x.png")))
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown() {
        assert!(matches!(
            Config::from_args(args(&["--nope"])),
            Cli::Invalid(_)
        ));
        assert!(matches!(
            Config::from_args(args(&["--screenshot", "foo", "x"])),
            Cli::Invalid(_)
        ));
        assert!(matches!(Config::from_args(args(&["-h"])), Cli::Help));
    }
}
