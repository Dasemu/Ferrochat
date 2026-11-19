# Lessons

Things learned while building Ferrochat that are worth knowing before the
next change.

## Protocol library

* **`whatsapp-rust` 0.7 is complete enough to skip the Go sidecar.** QR
  pairing, text, media upload/download (image, document, audio/PTT),
  receipts (read, played, read-self), history sync and logout are all native
  APIs. The evaluation took about an hour (read the crate source, build it,
  map events); see `docs/protocol.md` §1. A `whatsmeow` sidecar would have
  added a Go toolchain, IPC versioning and a second process for no gain.
* **Wrap it behind a trait from day one.** `net::WaClient` + `net::Event`
  means the whole app, the screenshots and the integration tests run against
  `net::fake::FakeClient`. No real WhatsApp account or network exists in CI.
* **Choose message ids client-side.** `SendOptions::with_message_id` lets the
  optimistic bubble, the store row and the later receipts share one id, so
  there is no "temporary id → server id" rewrite.
* `Event::Messages` is a batch and is at-least-once; the store insert is
  idempotent and the receipt merge monotonic, so redelivery is harmless.

## Encrypting the Signal store

* The library's SQLite store has a per-connection init hook intended for
  SQLCipher. Linking SQLCipher means: `whatsapp-rust` with
  `default-features = false` (drops its plain bundled SQLite),
  `whatsapp-rust-sqlite-storage` with `default-features = false`, and
  `libsqlite3-sys = { version = "0.37", features = ["bundled-sqlcipher"] }`.
  Only one crate may link `sqlite3`, so `rusqlite` must use the same
  `libsqlite3-sys` major (rusqlite 0.39 ↔ libsqlite3-sys 0.37).
* Use the raw-key form `PRAGMA key = "x'<64 hex>'"`: the key is already
  random, PBKDF2 would only slow every connection open.
* The r2d2 pool retries a failing connection for its whole timeout (30 s).
  Verify the key with a plain `rusqlite` connection first so a wrong key
  fails in milliseconds.
* Our own cache uses the same SQLCipher build without a key; unkeyed
  SQLCipher behaves like stock SQLite.

## iced 0.14

* Edition 2024 makes `impl Trait` return types capture every lifetime in
  scope. Style helpers like `fn bubble(t: &FerroTheme) -> impl Fn(&Theme) -> Style`
  need `+ use<>` or every view borrowing them fails with E0621.
* `scrollable().anchor_bottom()` is the right primitive for a chat, but it
  flips offsets: `RelativeOffset::START` is the newest message and
  `on_scroll`'s `relative_offset().y` approaches 1.0 at the *oldest* message.
  `snap_to_end` therefore scrolls to the top.
* `Subscription::run_with` takes a `fn` pointer, not a closure: put
  everything the stream needs (config, a restart generation, the secret store
  `Arc`) in a `Hash` key and hash only the parts that should restart it.
  Build the stream lazily (`stream::once(async { .. }).flatten()`) so
  `tokio::spawn` runs inside the executor.
* Bubble tails: a tiny `canvas` with one filled path is cheap and looks right.
* Big `Message` variants trip `clippy::large_enum_variant`; box task results.

## Audio

* `symphonia` has no Opus decoder. `ogg` (demux) + `opus` (libopus) is the
  smallest reliable combination; remember `OpusHead` pre-skip on decode and
  granule positions on encode.
* `rodio`'s output stream must live on its own thread; poll the position
  from the UI on a 100 ms tick only while playing.

## Environment

* No Secret Service in the container: the real-mode smoke test reaches the
  keyring and shows the explicit error on the login screen, which is the
  designed behaviour. WhatsApp's servers are not reachable from CI.
* A shared build host fills its disk quickly: debug test binaries with full
  debuginfo reached 16 GB. Remove `target/debug/incremental` when short on
  space.

## Review round 1

* Never delete a session automatically. Only a real logout wipes; a wrong
  key is `SQLITE_NOTADB` and still needs the user's confirmation; I/O and
  migration errors are retryable errors. `StreamReplaced` is not a logout.
* Count unread by "the store did not have this id", not by delivery path:
  the offline drain after reconnect is exactly when users expect badges.
* Compressed media needs output caps: a few hundred kB of Opus packets can
  decode to gigabytes. Cap by declared duration and packet count.
* `pkg-config` will not link system libraries statically; build libopus from
  the bundled source (`LIBOPUS_NO_PKG=1`) to get a static link.
* Version sync must be a real merge of main into development (simulated in
  `.github/scripts/version.sh`), or the second release conflicts.

## Tray / background mode

* `ksni` 0.3 depends on zbus 5, which the tree already had (via `rfd`/`ashpd`
  and the keyring); `notify-rust` brings zbus 6. Two zbus versions are
  pre-existing, adding the tray did not add a third.
* `ksni::spawn` fails when no StatusNotifierWatcher is on the bus. That result
  is the only trustworthy "is there a tray" signal; the window mode must be
  decided from it, never assumed. Watch `watcher_offline` too and reopen the
  window if the panel dies while hidden.
* `iced::daemon` stays alive with zero windows and drops the compositor when
  the last window closes. Set `exit_on_close_request: false` on the window and
  route `close_requests()` through the state machine.
* After the window is closed glibc keeps the freed pages; `malloc_trim(0)`
  gives them back (a few MB). The font system and the app state dominate the
  remaining resident size.
* zbus proxies cache properties by default and ksni announces changes with
  `New*` signals, not `PropertiesChanged`: build the test proxy with
  `CacheProperties::No`.
* `xdotool windowclose` destroys the window, it does not send
  `WM_DELETE_WINDOW`; to test close requests under Xvfb send the client
  message yourself.

## Release automation

- Bot pushes made with `GITHUB_TOKEN` never trigger `push` workflows, so the release workflow chains off the version workflow with `workflow_run` (conclusion `success`, branch `main`, checkout `ref: main` for the bump commit).


## Round 3: flicker, names, lists, video

* **Hover flicker was a reflow, not a redraw.** The hover buttons (smiley,
  chevron) were inserted into the bubble's row only while the pointer was
  over it. They took about 70 px from the row, so a bubble that was already
  near the pane width re-wrapped its text, the row grew by a line or two, the
  content moved under the pointer and `on_exit` / `on_enter` fired again: the
  message "blinked" while the mouse crossed it. Image and SVG handles were
  *not* the cause here (handles are cached in the app state and icon handles
  are content-hashed). Fix: the buttons' slot is always laid out (same size
  hovered or not) and only its content appears. Rule: **hover must never
  change layout; reserve the space.**
* **A layout regression test without a window.** `iced::Renderer` implements
  `renderer::Headless` (backend `"tiny-skia"`), so a unit test can lay out a
  widget with `Widget::layout` and compare sizes. `ui::chat::layout_tests`
  lays out the same message with and without hover at three pane widths and
  fails on the old code (371.6 px vs 409.3 px high for a long bubble at
  700 px).
* **Run `Task`s headlessly to test the whole app.** `iced_runtime::task::
  into_stream` (dev-dependency, same version iced uses) lets a test run a
  `Task`, collect its `Action::Output` messages and feed them back to
  `App::update`: `app::test_support::Harness` drives the real `App` and
  `Store` against `FakeClient`.
* **One name map was the whole "wrong names" bug.** Push names, history names
  and the address-book name shared one `HashMap`, so the last writer won and
  message push names beat saved names. Names now carry their source and the
  display order is a pure function (`ChatList::resolve`).
* **Address-book names only come through app-state sync** (`contact`
  mutations), and a session that was already synced never replays them: a
  one-time full snapshot request (`MajorSyncTask::AppStateSync { full_sync:
  true }`, marker `appstate_resync` in the cache) is needed for existing
  users.
* **Keep GStreamer out of the main binary.** A hard link against
  `libgstreamer` makes a machine without it fail at load time with a loader
  error instead of a message. A helper process (`ferrochat-player`) isolates
  the dependency, gives a precise failure ("GStreamer missing" vs "codec
  missing"), and freeing memory on close is just ending the process. The
  release workflow asserts with `ldd` that `ferrochat` has no `gst` link.
* `iced_video_player` renders through a wgpu shader only, so nothing shows on
  the tiny-skia fallback; one more reason for the helper design.
* `mouse_area` gives no window position for right clicks: track the cursor in
  an atomic from `listen_with` (returning `None`, so no message per move) and
  read it when the click arrives.
* `Task::perform` + `spawn_blocking` work on a plain tokio test runtime, which
  is all the harness needs.

- `keyring` 4 opens a *blocking* zbus connection: if any dependency turns on zbus `tokio` (ksni default features, `rfd`/`ashpd` `tokio`, a `zbus` dev-dep) it panics ("Cannot start a runtime from within a runtime") when called on a runtime worker. Keep zbus on `async-io` (`ksni` with `default-features = false, features = ["async-io"]`, `rfd` with `async-std`); `tests/real_keyring.rs` guards it against a real Secret Service.

