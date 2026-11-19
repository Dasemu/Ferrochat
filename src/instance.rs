//! Single-instance hand-off. The OS file lock (`config::acquire_instance_lock`)
//! decides who is first; the first instance also listens on a Unix socket in
//! the (0700) data directory. A second launch connects, sends `show`, and
//! exits: the first instance then shows and focuses its window, like the
//! official apps. The socket takes no other input.

use std::path::{Path, PathBuf};

/// `<data_dir>/ferrochat.sock`.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ferrochat.sock")
}

/// The only message the socket understands.
pub const SHOW: &[u8] = b"show\n";

/// Whether a received line is the "show" request.
pub fn is_show(line: &[u8]) -> bool {
    line.trim_ascii() == b"show"
}

#[cfg(unix)]
pub use unix::{cleanup, notify_running, serve};

#[cfg(not(unix))]
pub fn serve(_data_dir: &Path) -> std::io::Result<()> {
    Ok(())
}
#[cfg(not(unix))]
pub fn notify_running(_data_dir: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("not supported on this platform"))
}
#[cfg(not(unix))]
pub fn cleanup(_data_dir: &Path) {}

#[cfg(unix)]
mod unix {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::time::Duration;

    use super::{SHOW, is_show, socket_path};
    use crate::ext::{self, External};

    /// Starts listening (call only while holding the instance lock, so any
    /// existing socket file is stale). Runs on its own thread.
    pub fn serve(data_dir: &Path) -> std::io::Result<()> {
        let path = socket_path(data_dir);
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        std::thread::Builder::new()
            .name("instance-socket".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    handle(stream);
                }
            })?;
        Ok(())
    }

    fn handle(mut stream: UnixStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let mut buf = [0u8; 16];
        let mut len = 0;
        // Read one short line; anything longer or silent is dropped.
        while len < buf.len() {
            match stream.read(&mut buf[len..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    len += n;
                    if buf[..len].contains(&b'\n') {
                        break;
                    }
                }
            }
        }
        if is_show(&buf[..len]) {
            ext::send(External::Show { chat: None });
        }
    }

    /// Asks the running instance to show its window. Retries briefly: the
    /// first instance may still be binding its socket.
    pub fn notify_running(data_dir: &Path) -> std::io::Result<()> {
        let path = socket_path(data_dir);
        let mut last = std::io::Error::other("no socket");
        for _ in 0..10 {
            match UnixStream::connect(&path) {
                Ok(mut s) => {
                    s.set_write_timeout(Some(Duration::from_secs(1)))?;
                    return s.write_all(SHOW);
                }
                Err(e) => last = e,
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Err(last)
    }

    /// Removes the socket file on exit.
    pub fn cleanup(data_dir: &Path) {
        let _ = std::fs::remove_file(socket_path(data_dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_line_parsing() {
        assert!(is_show(b"show\n"));
        assert!(is_show(b" show \r\n"));
        assert!(!is_show(b"quit\n"));
        assert!(!is_show(b""));
    }
}
