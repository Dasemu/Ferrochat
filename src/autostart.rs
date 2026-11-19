//! Optional "start at login": an XDG autostart entry
//! `<config home>/autostart/dev.ferro.ferrochat.desktop` whose `Exec` line
//! carries `--minimized`, so the app comes up in the tray without a window.
//! Off by default; the settings toggle writes and removes the file.

use std::path::{Path, PathBuf};

use crate::config::APP_ID;

/// File name of the autostart entry.
pub fn file_name() -> String {
    format!("{APP_ID}.desktop")
}

/// `$XDG_CONFIG_HOME` or `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(".config")))
}

/// Where the entry lives under `config_home`.
pub fn entry_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join(file_name())
}

/// Quotes one `Exec` argument per the Desktop Entry spec: arguments with
/// spaces or reserved characters go in double quotes, with `"`, `` ` ``,
/// `$` and `\` backslash-escaped (and `%` doubled so it is not a field code).
pub fn exec_quote(arg: &str) -> String {
    let reserved = " \t\n\"'\\><~|&;$*?#()`";
    if arg.is_empty() || arg.contains(|c| reserved.contains(c)) || arg.contains('%') {
        let mut out = String::from("\"");
        for c in arg.chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '%' => out.push_str("%%"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    } else {
        arg.to_string()
    }
}

/// The text of the autostart entry for the binary at `exe`.
pub fn entry_text(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Ferrochat\n\
         Comment=Unofficial WhatsApp desktop client\n\
         Exec={} --minimized\nTerminal=false\nX-GNOME-Autostart-enabled=true\n\
         StartupNotify=false\n",
        exec_quote(&exe.to_string_lossy())
    )
}

/// Writes the entry under `config_home`.
pub fn enable_in(config_home: &Path, exe: &Path) -> std::io::Result<()> {
    let path = entry_path(config_home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, entry_text(exe))
}

/// Removes the entry (already gone is fine).
pub fn disable_in(config_home: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(entry_path(config_home)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Whether the entry exists under `config_home`.
pub fn is_enabled_in(config_home: &Path) -> bool {
    entry_path(config_home).is_file()
}

fn home() -> std::io::Result<PathBuf> {
    config_home().ok_or_else(|| std::io::Error::other("no config directory"))
}

/// Enables autostart for the running executable.
pub fn enable() -> std::io::Result<()> {
    enable_in(&home()?, &std::env::current_exe()?)
}

/// Disables autostart.
pub fn disable() -> std::io::Result<()> {
    disable_in(&home()?)
}

/// Whether autostart is on.
pub fn is_enabled() -> bool {
    config_home().is_some_and(|h| is_enabled_in(&h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(exec_quote("/usr/bin/ferrochat"), "/usr/bin/ferrochat");
        assert_eq!(
            exec_quote("/opt/My App/ferrochat"),
            "\"/opt/My App/ferrochat\""
        );
        assert_eq!(exec_quote("/a/$b"), "\"/a/\\$b\"");
        assert_eq!(exec_quote("/a/100%"), "\"/a/100%%\"");
    }

    #[test]
    fn entry_has_minimized_exec() {
        let t = entry_text(Path::new("/usr/bin/ferrochat"));
        assert!(t.contains("Exec=/usr/bin/ferrochat --minimized\n"));
        assert!(t.starts_with("[Desktop Entry]\n"));
    }

    #[test]
    fn create_and_remove_in_a_temp_config_home() {
        let home = std::env::temp_dir().join(format!("ferrochat-autostart-{}", std::process::id()));
        assert!(!is_enabled_in(&home));
        enable_in(&home, Path::new("/x/ferrochat")).unwrap();
        assert!(is_enabled_in(&home));
        assert!(entry_path(&home).ends_with("autostart/dev.ferro.ferrochat.desktop"));
        disable_in(&home).unwrap();
        assert!(!is_enabled_in(&home));
        disable_in(&home).unwrap();
        let _ = std::fs::remove_dir_all(home);
    }
}
