//! The video helper against real GStreamer (the mock clip is a 3 s VP8 WebM)
//! and the app-side `Player` against scripted fake helpers. No network, no
//! window: frames are just bytes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ferrochat::media::video::{self, Event, Failure, Player, spawn_with};
use ferrochat::media::video_proto::{Cmd, PlayState};
use tokio::sync::mpsc::UnboundedReceiver;

const HELPER: &str = env!("CARGO_BIN_EXE_ferrochat-player");
const WAIT: Duration = Duration::from_secs(20);

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ferrochat-vt-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn clip() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/mock-clip.webm")
}

async fn next(rx: &mut UnboundedReceiver<Event>) -> Option<Event> {
    tokio::time::timeout(WAIT, rx.recv()).await.ok().flatten()
}

#[tokio::test]
async fn the_helper_plays_a_real_clip_and_reports_everything() {
    let (player, mut rx) = spawn_with(Path::new(HELPER), &clip()).expect("spawn");
    let mut ready = None;
    let mut frames = 0;
    let mut last_pos = 0;
    let mut ended = false;
    while let Some(ev) = next(&mut rx).await {
        match ev {
            Event::Ready {
                width,
                height,
                duration_ms,
            } => ready = Some((width, height, duration_ms)),
            Event::Frame {
                width,
                height,
                rgba,
            } => {
                assert_eq!((width, height), (320, 180));
                assert_eq!(rgba.len(), 320 * 180 * 4);
                frames += 1;
            }
            Event::Time {
                position_ms, state, ..
            } => {
                last_pos = last_pos.max(position_ms);
                if state == PlayState::Ended {
                    ended = true;
                    break;
                }
            }
            Event::Failed(f) => panic!("playback failed: {f:?}"),
        }
    }
    let (w, h, dur) = ready.expect("ready message");
    assert_eq!((w, h), (320, 180));
    assert!((2500..=3500).contains(&dur), "duration {dur}");
    assert!(frames >= 10, "only {frames} frames");
    assert!(ended, "never reached the end");
    assert!(last_pos >= 2500, "position stopped at {last_pos}");

    // Replaying from the end works, and a seek lands where asked.
    player.send(Cmd::Play);
    let mut replayed = false;
    while let Some(ev) = next(&mut rx).await {
        if let Event::Time {
            position_ms, state, ..
        } = ev
            && state == PlayState::Playing
            && position_ms < 1500
        {
            replayed = true;
            break;
        }
    }
    assert!(replayed, "Play after the end did not restart");
    player.send(Cmd::Pause);
    player.send(Cmd::Seek(2000));
    let mut sought = false;
    while let Some(ev) = next(&mut rx).await {
        if let Event::Time {
            position_ms, state, ..
        } = ev
            && state == PlayState::Paused
            && (1700..=2300).contains(&position_ms)
        {
            sought = true;
            break;
        }
    }
    assert!(sought, "seek did not land near 2 s");

    // Closing frees the decoder: the helper exits and the channel closes.
    drop(player);
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    assert!(closed.is_ok(), "the helper outlived its player");
}

#[tokio::test]
async fn a_broken_file_fails_cleanly() {
    let dir = tmp("broken");
    let file = dir.join("broken.mp4");
    std::fs::write(
        &file,
        b"this is not a video at all, just text padding".repeat(8),
    )
    .unwrap();
    let (_player, mut rx) = spawn_with(Path::new(HELPER), &file).expect("spawn");
    match next(&mut rx).await {
        Some(Event::Failed(Failure::Broken(_) | Failure::MissingCodec(_))) => {}
        other => panic!("expected a clean failure, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_missing_file_fails_cleanly() {
    let (_player, mut rx) =
        spawn_with(Path::new(HELPER), Path::new("/nonexistent/clip.mp4")).expect("spawn");
    assert!(matches!(next(&mut rx).await, Some(Event::Failed(_))));
}

/// Writes an executable shell script acting as the helper.
fn script(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("helper.sh");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[tokio::test]
async fn player_parses_helper_output_and_delivers_commands() {
    let dir = tmp("script");
    // Ready(2x1, 1000 ms), a 2x1 frame, a time message, then record stdin.
    let helper = script(
        &dir,
        r#"printf 'R\002\000\000\000\001\000\000\000\350\003\000\000\000\000\000\000'
printf 'F\002\000\000\000\001\000\000\000\001\002\003\004\005\006\007\010'
printf 'T\364\001\000\000\000\000\000\000\350\003\000\000\000\000\000\000\001'
cat > "$1.cmds""#,
    );
    let target = dir.join("clip.mp4");
    let (player, mut rx) = spawn_with(&helper, &target).expect("spawn");
    assert_eq!(
        next(&mut rx).await,
        Some(Event::Ready {
            width: 2,
            height: 1,
            duration_ms: 1000
        })
    );
    assert_eq!(
        next(&mut rx).await,
        Some(Event::Frame {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8]
        })
    );
    assert_eq!(
        next(&mut rx).await,
        Some(Event::Time {
            position_ms: 500,
            duration_ms: 1000,
            state: PlayState::Playing
        })
    );
    player.send(Cmd::Pause);
    player.send(Cmd::Seek(750));
    player.send(Cmd::Volume(40));
    player.send(Cmd::Mute(true));
    drop(player);
    // The script exits once stdin closes, which ends the stream.
    assert!(next(&mut rx).await.is_none());
    let cmds = std::fs::read_to_string(dir.join("clip.mp4.cmds")).unwrap();
    assert_eq!(cmds, "pause\nseek 750\nvolume 40\nmute 1\nquit\n");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_helper_that_cannot_load_gstreamer_is_reported_as_unavailable() {
    let dir = tmp("noloader");
    let helper = script(
        &dir,
        "echo 'ferrochat-player: error while loading shared libraries: libgstreamer-1.0.so.0: cannot open shared object file' >&2; exit 127",
    );
    let (_player, mut rx) = spawn_with(&helper, &dir.join("x.mp4")).expect("spawn");
    match next(&mut rx).await {
        Some(Event::Failed(Failure::Unavailable(line))) => {
            assert!(line.contains("libgstreamer"), "{line}");
        }
        other => panic!("{other:?}"),
    }
    let msg = Failure::Unavailable(String::new()).user_message();
    assert!(msg.contains("GStreamer") && msg.contains("Open externally"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_helper_is_found_next_to_the_app() {
    // `cargo test` builds the helper into target/debug, one level above the
    // test executable's deps directory.
    assert!(video::helper_path().is_some());
}

#[allow(dead_code)]
fn _player_is_send(p: Player) -> impl Send {
    p
}
